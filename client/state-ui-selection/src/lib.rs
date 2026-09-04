//! User-decision source: multi-select bookkeeping.
//!
//! The user enters selection mode against a particular pane
//! (`Middle` or `Queue`) by hitting `m`. While active, row
//! activations toggle membership in `selected` instead of opening
//! the action menu. `range_anchor` records the row where Shift-Up/
//! Shift-Down ranges originated.

use imbl::OrdSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionContext {
    Middle,
    Queue,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiSelection {
    pub context: Option<SelectionContext>,
    pub selected: OrdSet<usize>,
    pub range_anchor: Option<usize>,
    /// Indices that belong to the *current* anchor→cursor range
    /// (legacy `range_selected`). Tracked separately from `selected`
    /// so that moving the cursor back across the anchor unselects
    /// only the rows we range-added, leaving prior explicit toggles
    /// alone. Empty when no anchor is active.
    pub range_selected: OrdSet<usize>,
}

impl UiSelection {
    pub fn is_active(&self) -> bool {
        self.context.is_some()
    }

    pub fn begin(&mut self, context: SelectionContext) {
        self.context = Some(context);
        self.selected.clear();
        self.range_anchor = None;
        self.range_selected.clear();
    }

    pub fn clear(&mut self) {
        self.context = None;
        self.selected.clear();
        self.range_anchor = None;
        self.range_selected.clear();
    }

    pub fn add(&mut self, idx: usize) {
        self.selected.insert(idx);
    }

    pub fn remove(&mut self, idx: usize) {
        self.selected.remove(&idx);
    }
}
