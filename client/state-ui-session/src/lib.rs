//! User-decision source: per-session breadcrumbs the dispatch layer
//! reads to keep auto-connect, lost-server reconnect, saved-view
//! restore coherent across ticks.
//!
//! These fields don't fit any of the other `state-ui-*` domains —
//! they're plumbing for the connection lifecycle and a couple of
//! one-shot guards, not a UI overlay or focus model. Keeping them on
//! their own source lets dispatch handlers in `runtime` operate on
//! `&mut Sources` without reaching back into a TUI-side god-struct.

use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct UiSession {
    /// Preferred server name loaded once from
    /// `~/.config/mkp/last_server`. Cleared after the auto-connect
    /// path consumes it.
    pub preferred_server: Option<Arc<str>>,
    /// One-shot guard: did we already attempt the preferred
    /// auto-connect path? Reset on disconnect so the runtime keeps
    /// trying after a server-lost event.
    pub auto_connect: bool,
    /// Wallclock when the runtime started — used for the 30 s grace
    /// period that waits for a named preferred server to appear in
    /// mDNS before falling back to the single-server first-run path.
    pub started_at: Instant,
    /// On disconnect, the previously-connected server name is
    /// stashed here so the ServerLostModal flow can wait for it to
    /// reappear in mDNS and reconnect automatically.
    pub lost_server: Option<Arc<str>>,
    /// Guard: once we've auto-restored the saved view for this
    /// session we don't do it again, even after a reconnect.
    pub auto_restored_view: bool,
    /// Backend name (mDNS server name) we're currently connected
    /// to. Set on Connected; cleared on Disconnect. Drives the
    /// per-backend persist scope (last-add-playlist, search history,
    /// saved view).
    pub backend_name: Option<Arc<str>>,
    /// On `restore_view` we know the song id the user was hovering
    /// last; the actual track list lands a tick or three later, so
    /// the auto-restore loop scans for this id and snaps the cursor
    /// once it appears.
    pub pending_cursor_song_id: Option<Arc<str>>,
}

impl Default for UiSession {
    fn default() -> Self {
        Self {
            preferred_server: None,
            auto_connect: true,
            started_at: Instant::now(),
            lost_server: None,
            auto_restored_view: false,
            backend_name: None,
            pending_cursor_song_id: None,
        }
    }
}
