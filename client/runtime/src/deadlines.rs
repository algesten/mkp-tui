//! Centralized wake-up scheduling.
//!
//! The doc's "Centralize deadline management" section: every UI
//! element that needs the loop to wake at a wall-clock instant
//! folds its candidate deadline into one min-fold. The main loop
//! reads the result and calls `recv_timeout(deadline - now)`.
//!
//! Why this is a free function and not a `drv` memo: inputs change
//! every tick (every `Instant::now()` is a new value when used as
//! "current time"), the fold is a handful of comparisons, and we
//! never want a stale cached deadline to keep the loop asleep past
//! a real expiry. See `EXAMPLE-ARCH.md` "Wake on event, don't
//! spin" + the led-rewrite `nearest_deadline` for the same call.
//!
//! Add a new candidate by calling `consider(...)` in the body
//! below — never plumb a separate timeout into the loop. As the
//! list grows, projecting the relevant inputs into a real
//! `#[derive(drv::Input)]` becomes worthwhile and the inputs
//! struct moves up alongside the view-model memos.

use std::time::{Duration, Instant};

use crate::sources::Sources;

/// Cadence for the braille spinner. ~8 Hz reads as a smooth spin
/// without pinning the loop.
const SPIN_INTERVAL: Duration = Duration::from_millis(120);

/// Min-fold over every wake-deadline source the runtime currently
/// knows about. `None` means "no pending deadline; the loop only
/// needs to wake on driver / input notify".
pub fn nearest_deadline(sources: &Sources) -> Option<Instant> {
    let mut soonest: Option<Instant> = None;
    let consider = |soonest: &mut Option<Instant>, candidate: Option<Instant>| {
        if let Some(t) = candidate {
            *soonest = match *soonest {
                Some(cur) if cur < t => Some(cur),
                _ => Some(t),
            };
        }
    };

    // Spinner animation. Without a spinner-driven element on
    // screen the cadence is None and the loop only wakes on
    // events. Mirrors led-rewrite's `lsp_status.any_busy()` →
    // schedule-wake-in-80 ms idiom.
    if any_spinner_active(sources) {
        consider(&mut soonest, Some(sources.clock.now + SPIN_INTERVAL));
    }

    // Hover-preview expiry — the bar reverts to now-playing once
    // the preview's TTL elapses, so the loop must wake at exactly
    // that instant for the renderer to redraw without a stale-
    // looking lag.
    consider(&mut soonest, sources.preview.nearest_deadline());

    // Toast expiry — same shape; the yellow message disappears
    // once `expires_at` passes.
    consider(&mut soonest, sources.toast.nearest_deadline());

    // Stale-task GC — wake at `started_at + TTL` for the oldest
    // unacknowledged peer activity so it falls off the bar at the
    // right moment instead of lingering until the next user input.
    consider(
        &mut soonest,
        sources
            .activity
            .next_reap_at(mkpclient_state_activity::STALE_TASK_TTL),
    );

    // Reconnect backoff — the loop must be awake at the instant the
    // next attempt becomes legal, otherwise a dropped link would sit
    // until some unrelated event happened to wake it. This is what
    // lets the retry be event-driven instead of a fixed-interval
    // poll (`EXAMPLE-ARCH.md` § "Wake on event, don't spin").
    consider(&mut soonest, sources.link.retry_at);

    soonest
}

/// True iff any UI element that draws a braille spinner glyph is
/// currently visible. Add new sources here as they're added to
/// `render` — the doc's "what should be true" pattern: nobody
/// imperatively starts/stops the spinner, the deadline simply
/// re-derives from sources each tick.
fn any_spinner_active(sources: &Sources) -> bool {
    use mkpclient_state_link::LinkPhase;
    if sources.link.phase != LinkPhase::Connected {
        return true;
    }
    if !sources.playlists.loaded {
        return true;
    }
    let pt = &sources.playlist_tracks;
    if pt.playlist_id.is_some() && !pt.is_ready() && pt.songs.is_empty() {
        return true;
    }
    let s = &sources.search;
    if s.task_id.is_some() && !s.first_page_received {
        return true;
    }
    if sources.activity.most_recent().is_some() {
        return true;
    }
    // Optimistic mutations animate spinner glyphs in the left
    // column (creating / renaming) and the middle pane (adding)
    // until the matching reply lands. `removing` is purely a
    // filter — no spinner — so it doesn't need to wake the loop.
    let pending = &sources.pending_playlists;
    if !pending.creating.is_empty() || !pending.renaming.is_empty() || !pending.adding.is_empty() {
        return true;
    }
    false
}
