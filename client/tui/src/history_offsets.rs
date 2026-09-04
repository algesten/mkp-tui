//! Reconcile the middle-pane scroll-offset stacks with `history.back`
//! / `history.forward` transitions.
//!
//! The live `middle_offset` is render scratch (ratatui's `list_state`
//! reads + writes it every frame), so it stays in `AppState`. The
//! parallel `middle_offset_back` / `middle_offset_forward` stacks
//! mirror the runtime's `history.back` / `history.forward` so
//! Shift-Left / Shift-Right restore the previous viewport, not just
//! the previous cursor row.
//!
//! The previous design had every key handler that mutated history
//! also reach into `AppState` and manipulate the stacks inline. That
//! was reactive ("after dispatch, look at how `history.back` changed
//! and react") and brittle: any new code path that mutated
//! `history.back` differently silently broke the stacks. This module
//! replaces all that with a single reconciliation: snapshot
//! `history.transition_seq` before the dispatch, run the handlers,
//! then call [`reconcile`] which reads `history.last_transition` to
//! apply the matching stack transition. One code path covers drill /
//! back / forward.

use mkpclient_runtime::Runtime;
use mkpclient_state_ui_history::{HistoryTransition, UiHistory};

use crate::app::AppState;

/// Snapshot of the runtime's `history.transition_seq`. Compared
/// against the post-dispatch value to detect that a navigation
/// happened this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLens {
    pub transition_seq: u64,
}

impl HistoryLens {
    pub fn from_runtime(rt: &Runtime) -> Self {
        Self::from_history(&rt.sources.history)
    }

    pub fn from_history(h: &UiHistory) -> Self {
        Self {
            transition_seq: h.transition_seq,
        }
    }
}

/// Apply the offset-stack transition that matches the navigation the
/// dispatch handlers performed this tick. No-op if no transition
/// happened (`transition_seq` unchanged) or if `last_transition` is
/// `None` (history hasn't been touched yet at all).
pub fn reconcile(before: HistoryLens, rt: &Runtime, app: &mut AppState) {
    let history = &rt.sources.history;
    if history.transition_seq == before.transition_seq {
        return;
    }
    let Some(kind) = history.last_transition else {
        return;
    };
    match kind {
        HistoryTransition::Drill => {
            // Archive live offset to back, drop redo offsets, reset
            // live offset for the new (cursor=0) view.
            let cur = app.middle_offset.get();
            app.middle_offset_back.push(cur);
            app.middle_offset_forward.clear();
            app.middle_offset.set(0);
        }
        HistoryTransition::Back => {
            // Live offset → forward stack; pop back into live.
            let cur = app.middle_offset.get();
            app.middle_offset_forward.push(cur);
            let next = app.middle_offset_back.pop().unwrap_or(0);
            app.middle_offset.set(next);
        }
        HistoryTransition::Forward => {
            // Live offset → back stack; pop forward into live.
            let cur = app.middle_offset.get();
            app.middle_offset_back.push(cur);
            let next = app.middle_offset_forward.pop().unwrap_or(0);
            app.middle_offset.set(next);
        }
    }
}
