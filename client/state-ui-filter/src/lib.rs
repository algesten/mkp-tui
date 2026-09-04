//! User-decision source: per-pane live filter strings.
//!
//! Three independent slots: the left playlists list filter, the
//! middle pane (tracks / search / detail) filter, and the queue
//! column filter. All start empty; the FilterInput modal commits
//! to one of them depending on which pane was focused at open.
//!
//! Stored as `Arc<str>` so projections borrowing these fields can
//! snap them with an atomic refcount bump (no per-call heap alloc)
//! and `eq_static` short-circuits via `Arc::ptr_eq` between
//! keystrokes — the value only mutates when the user types or the
//! field is cleared.

use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiFilter {
    pub playlist: Arc<str>,
    pub middle: Arc<str>,
    pub queue: Arc<str>,
}

impl Default for UiFilter {
    fn default() -> Self {
        let empty: Arc<str> = Arc::from("");
        Self {
            playlist: empty.clone(),
            middle: empty.clone(),
            queue: empty,
        }
    }
}

/// Which pane the FilterInput modal is editing. Legacy parity:
/// only Middle and Queue can be filtered — Left has no filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FilterTarget {
    Middle,
    Queue,
}
