//! Per-view UI bridge driver — the "pure output driver" of
//! EXAMPLE-ARCH.md §"Pure output drivers".
//!
//! ## Shape
//!
//! - Source: [`UiBridgeState`] holds the last payload pushed for
//!   each view (the spec's *in-flight artifact*). The driver dedups
//!   on this; the runtime mutates it via `execute`.
//! - Cmd ABI: [`BridgeCmd::Push`] carries `(ViewKind, Vec<u8>)`.
//!   The native worker receives one and ships the bytes to the
//!   platform (e.g. fires the iOS C ABI callback).
//! - Event ABI: empty for now. Neither SwiftUI nor ratatui acks
//!   paint, so there's no `last_acked` to track. Reserved if a
//!   future platform exposes paint completion.
//! - Trace: per-driver narrow trait, adapted by the runtime's
//!   unified trace sink.
//!
//! ## Spec compliance
//!
//! Per EXAMPLE-ARCH.md §"Pure output drivers":
//!
//! > The point isn't tracking frames for their own sake — it's that
//! > the same "write intent sync, fire async, diff is Noop until
//! > Done" pattern every other driver uses applies here too.
//!
//! `execute` writes intent into `state.in_flight` synchronously,
//! then ships `BridgeCmd::Push` to the worker asynchronously. The
//! sync write closes the loop: a re-call with the same bytes in the
//! same tick is a no-op.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Identifier for each view memo whose output the bridge tracks.
/// `repr(u32)` so the discriminant can cross the C ABI as a stable
/// integer. Variants are append-only — the integer values are part
/// of the wire contract with the Swift side.
///
/// Modal views (the `*Modal` variants) ship `Option<Model>` as their
/// payload: `Some(model)` while the modal is active, `None`
/// otherwise. SwiftUI binds visibility to payload presence so a
/// `.sheet(isPresented:)` flips closed when the runtime emits a
/// `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ViewKind {
    // ── Always-on content views ──────────────────────────────────
    NowPlaying = 0,
    LeftColumn = 1,
    MiddleHeader = 2,
    Queue = 3,
    PlaylistTracks = 4,
    SearchResults = 5,
    AlbumDetail = 6,
    ArtistDetail = 7,
    SelectionBar = 8,
    PreConnect = 9,

    // ── Modals (Option<Model> payload) ───────────────────────────
    ActionModal = 10,
    ConfirmDeletePlaylist = 11,
    ConfirmRemove = 12,
    ErrorModal = 13,
    FilterInput = 14,
    HelpOverlay = 15,
    InputModal = 16,
    PairingModal = 17,
    PlaylistActionModal = 18,
    PlaylistPickerHint = 19,
    SearchInputModal = 20,
    SelectionActionModal = 21,
    ServerLostModal = 22,
    ServerPickerModal = 23,

    // ── Shell: which top-level surface is up ─────────────────────
    Shell = 24,
}

impl ViewKind {
    pub fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Driver-owned source: the last payload shipped for each view.
/// The runtime never mutates this directly — only `UiBridgeDriver::execute`
/// does, mirroring how every other driver writes its in-flight state
/// inside its own execute method.
#[derive(Debug, Default)]
pub struct UiBridgeState {
    pub in_flight: HashMap<ViewKind, Vec<u8>>,
}

/// Async commands the driver ships to its native worker.
#[derive(Clone, Debug)]
pub enum BridgeCmd {
    Push { kind: ViewKind, payload: Vec<u8> },
}

/// `--golden-trace` hook for view pushes.
pub trait Trace: Send + Sync {
    fn bridge_push(&self, _kind: ViewKind, _bytes: usize) {}
    fn bridge_skip(&self, _kind: ViewKind) {}
}

pub struct NoopTrace;
impl Trace for NoopTrace {}

/// Sync handle the runtime calls into. Held in the per-platform
/// `Drivers` bundle.
pub struct UiBridgeDriver {
    cmd_tx: Sender<BridgeCmd>,
    trace: Arc<dyn Trace>,
}

impl UiBridgeDriver {
    pub fn new(cmd_tx: Sender<BridgeCmd>, trace: Arc<dyn Trace>) -> Self {
        Self { cmd_tx, trace }
    }

    /// Execute pattern: dedup against in-flight, then write intent
    /// synchronously and ship the push asynchronously.
    pub fn execute(&self, kind: ViewKind, payload: Vec<u8>, state: &mut UiBridgeState) {
        if state.in_flight.get(&kind) == Some(&payload) {
            self.trace.bridge_skip(kind);
            return;
        }
        // Sync: prevent re-firing if the runtime calls execute again
        // within the same tick (the spec's "next query returns Noop"
        // invariant — without acks we collapse it to "next call with
        // identical bytes is Noop").
        state.in_flight.insert(kind, payload.clone());
        self.trace.bridge_push(kind, payload.len());
        // Async: ship to the native worker.
        if self.cmd_tx.send(BridgeCmd::Push { kind, payload }).is_err() {
            log::debug!("ui-bridge: cmd channel closed; native worker has exited");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn first_push_writes_intent_and_emits_cmd() {
        let (tx, rx) = mpsc::channel::<BridgeCmd>();
        let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
        let mut state = UiBridgeState::default();

        driver.execute(ViewKind::NowPlaying, vec![1, 2, 3], &mut state);

        assert_eq!(
            state.in_flight.get(&ViewKind::NowPlaying),
            Some(&vec![1, 2, 3])
        );
        match rx.try_recv() {
            Ok(BridgeCmd::Push { kind, payload }) => {
                assert_eq!(kind, ViewKind::NowPlaying);
                assert_eq!(payload, vec![1, 2, 3]);
            }
            other => panic!("expected one Push, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_push_is_noop() {
        let (tx, rx) = mpsc::channel::<BridgeCmd>();
        let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
        let mut state = UiBridgeState::default();

        driver.execute(ViewKind::NowPlaying, vec![1, 2, 3], &mut state);
        driver.execute(ViewKind::NowPlaying, vec![1, 2, 3], &mut state);

        // First call shipped; second was deduped.
        let _ = rx.try_recv().expect("first push");
        assert!(rx.try_recv().is_err(), "duplicate push should not fire");
    }

    #[test]
    fn changed_payload_replaces_in_flight() {
        let (tx, rx) = mpsc::channel::<BridgeCmd>();
        let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
        let mut state = UiBridgeState::default();

        driver.execute(ViewKind::NowPlaying, vec![1, 2, 3], &mut state);
        driver.execute(ViewKind::NowPlaying, vec![4, 5, 6], &mut state);

        assert_eq!(
            state.in_flight.get(&ViewKind::NowPlaying),
            Some(&vec![4, 5, 6])
        );
        let _ = rx.try_recv().expect("first push");
        let _ = rx.try_recv().expect("second push");
    }
}
