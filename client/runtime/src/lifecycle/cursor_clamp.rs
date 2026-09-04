//! Cursor-clamp lifecycle: keep each column's cursor (and the action
//! modal's selection) inside the row range it actually has.
//!
//! Spec §5 / §6: the question is "given the live row count + this
//! cursor, what cursor index is valid?" That's a desired-state memo.
//! Without it, an external mutation that shrinks the list (e.g. a
//! `PlaylistMutation::SongRemoved` broadcast taking the row out from
//! under the cursor, a `PlaylistMutation::Deleted` shrinking the left
//! pane, a `RemoveFromQueue` shrinking the queue, or a permission
//! flip changing the action-modal menu shape) leaves the cursor past
//! the end and the painter renders no cursor at all.
//!
//! One action memo handles every site by construction: inputs are a
//! precomputed row count + the cursor index. Per call site, a thin
//! trampoline reads the right row-count helper, calls the memo, and
//! writes back. Spec parity: a query, a diff, a write — no
//! transition handlers.

use mkpclient_state_ui_screen::Screen;

use crate::queries;
use crate::sources::Sources;
use crate::views::{action_modal_model, ActionModalInput};

// ─── inputs ─────────────────────────────────────────────────────────

/// Cursor projection for the column being clamped. The same input
/// type serves all three columns — the trampoline plucks the right
/// field off `Cursor` before calling the memo.
#[derive(drv::Input)]
pub struct ColumnCursorInput {
    pub cursor: usize,
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorClampAction {
    Noop,
    /// Write `cursor = row`.
    Snap {
        row: usize,
    },
}

#[drv::memo(single)]
pub fn cursor_clamp_action(row_count: usize, input: ColumnCursorInput) -> CursorClampAction {
    if row_count == 0 {
        if input.cursor == 0 {
            CursorClampAction::Noop
        } else {
            CursorClampAction::Snap { row: 0 }
        }
    } else if input.cursor >= row_count {
        CursorClampAction::Snap { row: row_count - 1 }
    } else {
        CursorClampAction::Noop
    }
}

// ─── trampolines ────────────────────────────────────────────────────

pub fn apply_middle_cursor_clamp(sources: &mut Sources) {
    let row_count = queries::middle_row_count(sources);
    let action = cursor_clamp_action(
        row_count,
        ColumnCursorInput {
            cursor: sources.cursor.middle,
        },
    );
    if let CursorClampAction::Snap { row } = action {
        sources.cursor.middle = row;
    }
}

pub fn apply_queue_cursor_clamp(sources: &mut Sources) {
    let row_count = queries::queue_filtered_indices(sources).len();
    let action = cursor_clamp_action(
        row_count,
        ColumnCursorInput {
            cursor: sources.cursor.queue,
        },
    );
    if let CursorClampAction::Snap { row } = action {
        sources.cursor.queue = row;
    }
}

pub fn apply_left_cursor_clamp(sources: &mut Sources) {
    // Left column = [server][playlists…][+ New]: filtered playlist
    // count + 2 fixed rows. Mirrors `dispatch::left_n_rows`.
    let row_count = queries::filtered_playlist_count(sources, &sources.filter.playlist) + 2;
    let action = cursor_clamp_action(
        row_count,
        ColumnCursorInput {
            cursor: sources.cursor.left,
        },
    );
    if let CursorClampAction::Snap { row } = action {
        sources.cursor.left = row;
    }
}

pub fn apply_action_modal_clamp(sources: &mut Sources) {
    let Screen::ActionModal(state) = &sources.screen else {
        return;
    };
    // Single source of truth for the menu shape: the view memo. The
    // source's `selected` index must be valid against this same shape
    // — otherwise dispatch's modulo-len cycling and the painter's
    // selection styling disagree on out-of-bounds values.
    let model = action_modal_model(ActionModalInput::new(state, &sources.keybindings));
    let action = cursor_clamp_action(
        model.rows.len(),
        ColumnCursorInput {
            cursor: state.selected,
        },
    );
    if let CursorClampAction::Snap { row } = action {
        if let Screen::ActionModal(state) = &mut sources.screen {
            state.selected = row;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(cursor: usize) -> ColumnCursorInput {
        ColumnCursorInput { cursor }
    }

    #[test]
    fn noop_when_in_bounds() {
        assert_eq!(cursor_clamp_action(3, input(1)), CursorClampAction::Noop);
    }

    #[test]
    fn clamps_to_last_when_past_end() {
        assert_eq!(
            cursor_clamp_action(3, input(5)),
            CursorClampAction::Snap { row: 2 }
        );
    }

    #[test]
    fn snaps_to_zero_when_list_empty_and_cursor_nonzero() {
        assert_eq!(
            cursor_clamp_action(0, input(4)),
            CursorClampAction::Snap { row: 0 }
        );
    }

    #[test]
    fn noop_when_list_empty_and_cursor_zero() {
        assert_eq!(cursor_clamp_action(0, input(0)), CursorClampAction::Noop);
    }

    #[test]
    fn noop_at_exact_last() {
        assert_eq!(cursor_clamp_action(3, input(2)), CursorClampAction::Noop);
    }
}
