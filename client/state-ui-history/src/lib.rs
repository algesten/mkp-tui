//! User-decision source: middle-pane navigation history.
//!
//! `mode` is what the middle pane currently shows; `back` is the
//! pre-rewrite "Shift-Left" stack, `forward` is the redo stack
//! (cleared on a fresh navigation, populated by `back`).

use std::sync::Arc;

use mkproto::{SearchType, TaskId};

/// What the middle pane is currently showing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum MiddleMode {
    #[default]
    PlaylistSongs,
    SearchResults {
        term: String,
        search_type: SearchType,
        /// Task id correlating Search reply + streamed SearchMore
        /// pages. `None` until the request has been issued.
        task_id: Option<TaskId>,
    },
    AlbumDetail {
        album_id: String,
        album_title: String,
        awaiting_seq: Option<u64>,
    },
    ArtistDetail {
        artist_id: String,
        artist_name: String,
        awaiting_seq: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryFrame {
    pub mode: MiddleMode,
    pub filter: Arc<str>,
    pub selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryTransition {
    Drill,
    Back,
    Forward,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiHistory {
    pub mode: MiddleMode,
    pub back: Vec<HistoryFrame>,
    pub forward: Vec<HistoryFrame>,
    /// What kind of transition produced the current `mode` /
    /// `back` / `forward` state. Set by `history_drill`,
    /// `history_back`, `history_forward`. Consumers (e.g. the TUI's
    /// scroll-offset reconciler) pair it with `transition_seq` to
    /// detect "a transition happened this tick" without having to
    /// diff stack contents.
    pub last_transition: Option<HistoryTransition>,
    /// Monotonically increments on every drill / back / forward.
    /// Used by consumers to detect that a transition has occurred
    /// (an unchanged seq means no transition this tick, even if
    /// `last_transition` is set from an earlier one).
    pub transition_seq: u64,
}
