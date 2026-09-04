//! User-decision source: cursor positions + active column focus.
//!
//! `Cursor` is a plain struct (no driver, no async side) — dispatch
//! mutates fields directly. View-model memos project the relevant
//! field via `drv::Input`s declared in the consumer crate.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnFocus {
    #[default]
    Left,
    Middle,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Active pane.
    pub focus: ColumnFocus,
    /// Cursor row in the left (playlists) column. Index 0 is the
    /// "server" row; 1..=N+1 are the playlists / "New…".
    pub left: usize,
    /// Cursor row in the middle pane (mode-dependent).
    pub middle: usize,
    /// Cursor row in the queue column.
    pub queue: usize,
    /// Cursor in the pre-connect server picker.
    pub server_picker: usize,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            focus: ColumnFocus::default(),
            // Default cursor at the first playlist row, not the
            // "server" row at index 0 — matches legacy ergonomics.
            left: 1,
            middle: 0,
            queue: 0,
            server_picker: 0,
        }
    }
}

impl Cursor {
    pub fn cycle_focus_forward(&mut self) {
        self.focus = match self.focus {
            ColumnFocus::Left => ColumnFocus::Middle,
            ColumnFocus::Middle => ColumnFocus::Queue,
            ColumnFocus::Queue => ColumnFocus::Left,
        };
    }

    pub fn cycle_focus_backward(&mut self) {
        self.focus = match self.focus {
            ColumnFocus::Left => ColumnFocus::Queue,
            ColumnFocus::Middle => ColumnFocus::Left,
            ColumnFocus::Queue => ColumnFocus::Middle,
        };
    }
}
