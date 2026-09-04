//! UI-loop adapter between the runtime and the TUI's render layer.
//!
//! All dispatch / lifecycle logic lives in `mkpclient-runtime`. What
//! remains here is render-scratch: the spinner tick and the per-pane
//! scroll-offset `Cell`s the renderer mutates through `&AppState`.

use std::cell::Cell;

#[derive(Debug, Default)]
pub struct AppState {
    /// Set by Ctrl-Z / the configured Suspend action and consumed by
    /// the outer loop after it has drained the current input batch.
    pub suspend_requested: bool,
    /// Frame counter used to advance spinner animation. Bumped each
    /// render tick by the main loop.
    pub tick: u32,
    /// Persistent scroll offset for the left / middle / queue panes.
    /// `Cell` so the render path can update through `&AppState`
    /// without needing `&mut`.
    pub left_offset: Cell<usize>,
    pub middle_offset: Cell<usize>,
    pub queue_offset: Cell<usize>,
    /// Parallel stacks for the middle pane's scroll offset, mirroring
    /// the runtime's `history.back` / `history.forward`. The input
    /// translator pushes/pops on every history transition so Shift-Left
    /// / Shift-Right restores the previous viewport, not just the
    /// previous cursor row.
    pub middle_offset_back: Vec<usize>,
    pub middle_offset_forward: Vec<usize>,
}
