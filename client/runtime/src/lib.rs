//! Runtime for the drv-based client: sources bundle, driver bundle,
//! ingest + execute phases, and the blocking wake primitive.
//!
//! This crate is platform-neutral. It does **not** spawn native
//! drivers — a per-platform `runtime-*` crate
//! (`mkpclient-runtime-desktop`, `mkpclient-runtime-ios`) wires the
//! right natives and assembles the bundle via
//! [`Drivers::from_handles`] and [`Runtime::from_parts`].
//!
//! The caller (TUI binary, iOS FFI layer) owns the outer render loop
//! and is responsible for:
//!   - invoking [`Runtime::tick`] each time around the loop
//!   - calling [`Runtime::dispatch`] for user events
//!   - blocking on [`Runtime::wait_for_wake`] when idle
//!   - reading [`Runtime::sources`] to render.
//!
//! This split lets one sync main loop drive both the TUI (terminal
//! paint driver) and the iOS bridge (SwiftUI push) without baking
//! either into the runtime crate.

mod bridge;
mod deadlines;
mod dispatch;
mod drivers;
mod execute;
mod ingest;
mod lifecycle;
pub mod queries;
pub mod render;
mod sources;
pub mod views;

pub use bridge::ViewBridge;
pub use deadlines::nearest_deadline;
pub use dispatch::{
    history_back, history_drill, history_forward, DispatchEvent, Dispatcher, JumpTarget,
    SemanticEvent, TuiCursorEvent,
};
pub use drivers::{
    clipboard_trace, credentials_trace, discovery_trace, link_trace, persist_trace, Drivers,
    NativeMarker, RuntimeTrace, Trace,
};
pub use mkpclient_core::Notifier;
pub use mkproto::{ClientMsg, Peer, PlayState, PlaybackState, Song};
pub use sources::Sources;

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use log::debug;

/// Identity the runtime announces on each `Hello` post-connect.
/// Caller supplies `user` (usually `$USER`) and `host` (usually the
/// local mDNS hostname). The runtime keeps it and stamps every
/// Hello with it.
pub type PeerIdentity = Peer;

/// Caller-supplied flags that influence the boot path. Defaults
/// reproduce the historical auto-connect behavior — auto-connect to
/// the previously used server as soon as it shows up in mDNS.
#[derive(Debug, Default, Clone)]
pub struct RuntimeOptions {
    /// Skip the auto-reconnect path: don't load `last_server` from
    /// disk and start with `session.auto_connect = false`. The TUI
    /// uses this for `--pick`, which sends the user straight to the
    /// server picker instead of dialing the previous server.
    pub pick: bool,
}

/// One-stop handle the caller holds for the lifetime of the app.
pub struct Runtime {
    pub sources: Sources,
    pub drivers: Drivers,
    pub peer: PeerIdentity,
    notify: Notifier,
    wake_rx: Receiver<()>,
}

/// Channel pair plus notifier used to wake the runtime loop. A
/// per-platform crate creates one with [`make_wake`] before spawning
/// natives so each driver can be handed a clone of the notifier.
pub struct Wake {
    pub notify: Notifier,
    pub wake_rx: Receiver<()>,
}

/// Build a fresh wake channel + notifier pair. Per-platform crates
/// pass `notify` to every native `spawn(...)` call so workers can
/// signal the loop, and hand the `Wake` to [`Runtime::from_parts`].
pub fn make_wake() -> Wake {
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let notify = Notifier::new(wake_tx);
    Wake { notify, wake_rx }
}

impl Runtime {
    /// Assemble a `Runtime` from already-spawned drivers and an
    /// already-built wake channel. The per-platform crate is
    /// responsible for any boot-time prefetches (credentials load,
    /// last-server load) and for stamping the corresponding
    /// in-flight markers on `sources` before this call.
    pub fn from_parts(sources: Sources, drivers: Drivers, peer: PeerIdentity, wake: Wake) -> Self {
        Self {
            sources,
            drivers,
            peer,
            notify: wake.notify,
            wake_rx: wake.wake_rx,
        }
    }

    /// Clone the runtime's wake handle. External event sources (a
    /// keyboard reader thread, a push-notification channel, …) call
    /// `notify()` on this to unblock `wait_for_wake` — otherwise the
    /// main loop sits on its 250 ms timer and UI inputs feel laggy.
    pub fn notifier(&self) -> Notifier {
        self.notify.clone()
    }

    /// Ingest every pending driver event into sources, then execute
    /// any desired driver commands implied by the new source state.
    pub fn tick(&mut self) {
        // Drop any expired hover-preview / toast before memos look
        // at them, so `*.is_some()` cleanly means "live". Mirrors
        // guideline 7 ("clean up stale user decisions in the ingest
        // phase").
        // EXAMPLE-ARCH §"Time is a source field": stamp clock once
        // per tick so every downstream consumer (dispatch, ingest,
        // memos) reads a consistent "now".
        self.sources.clock.tick(Instant::now());
        let now = self.sources.clock.now;
        self.sources.preview.drop_if_expired(now);
        self.sources.toast.drop_if_expired(now);
        self.sources
            .activity
            .reap_stale(now, mkpclient_state_activity::STALE_TASK_TTL);
        ingest::run(&mut self.sources, &self.drivers, &self.peer);
        execute::run(&mut self.sources, &self.drivers);
    }

    /// Mutate sources in response to a UI event. Accepts any
    /// [`SemanticEvent`] / [`TuiCursorEvent`] / [`DispatchEvent`] —
    /// the `Into<DispatchEvent>` conversion is automatic for the two
    /// inner buckets.
    pub fn dispatch<E: Into<DispatchEvent>>(&mut self, ev: E) {
        dispatch::dispatch(ev, &mut self.sources, &self.drivers);
    }

    /// Block until a driver signals new work, a UI-side wake-up
    /// fires, or the timeout elapses. Returns immediately if a wake
    /// is already queued.
    pub fn wait_for_wake(&self, timeout: Duration) {
        match self.wake_rx.recv_timeout(timeout) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                debug!("runtime: all wake senders dropped");
            }
        }
        // Collapse any wake signals that piled up during the tick or
        // while blocking. Order matters: drain AFTER recv_timeout,
        // never before — work kicked off in the previous tick may
        // complete fast enough to post its event and signal wake
        // before we reach the sleep, and a drain ahead of
        // recv_timeout would swallow it, forcing a wait until either
        // the timeout fires or another wake arrives.
        while self.wake_rx.try_recv().is_ok() {}
    }

    /// How long the loop would block right now. A deadline already
    /// in the past yields zero — wake now — rather than falling into
    /// the no-deadline fallback, which would park the loop for a
    /// minute with work already due and hide every other pending
    /// deadline behind the stale one.
    pub fn next_timeout(&self) -> Duration {
        match nearest_deadline(&self.sources) {
            Some(d) => d.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(60),
        }
    }

    /// Block until something happens, computing the timeout from
    /// `nearest_deadline(&self.sources)`. The single call site every
    /// caller should reach for: anything that needs the loop to wake
    /// at a wall-clock instant (spinner cadence, toast expiry,
    /// preview timeout, reconnect backoff) folds itself into
    /// `nearest_deadline` and the loop stays unchanged.
    pub fn wait_for_next_deadline(&self) {
        self.wait_for_wake(self.next_timeout());
    }
}
