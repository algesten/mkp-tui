//! View model for the bulk-action modal (`Screen::SelectionActionModal`).
//!
//! Materialises the selection-action menu rows + selected count so
//! the renderer is a pure painter. The same row list backs the
//! dispatch handler's hot-key parsing — keeping both off one memo
//! avoids drift between the renderer's labels and the handler's
//! key-set.

use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_selection::SelectionContext;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SelectionActionRow {
    pub key: String,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SelectionActionModalModel {
    pub rows: Vec<SelectionActionRow>,
    pub count: usize,
    pub selected: usize,
}

#[derive(drv::Input)]
pub struct SelectionActionModalInput {
    pub ctx_queue: bool,
    pub ctx_middle: bool,
    pub middle_is_playlist_songs: bool,
    pub count: usize,
    pub selected: usize,
    pub keys: Vec<String>,
}

impl SelectionActionModalInput {
    pub fn new(
        ctx: Option<SelectionContext>,
        middle_mode: &MiddleMode,
        count: usize,
        selected: usize,
        keys: &mkpclient_state_ui_keybindings::Keybindings,
    ) -> Self {
        Self {
            ctx_queue: matches!(ctx, Some(SelectionContext::Queue)),
            ctx_middle: matches!(ctx, Some(SelectionContext::Middle)),
            middle_is_playlist_songs: matches!(middle_mode, MiddleMode::PlaylistSongs),
            count,
            selected,
            keys: [
                mkpclient_state_ui_keybindings::Action::SelectionPlayNext,
                mkpclient_state_ui_keybindings::Action::SelectionPlayLast,
                mkpclient_state_ui_keybindings::Action::SelectionAddToPlaylist,
                mkpclient_state_ui_keybindings::Action::SelectionDelete,
            ]
            .into_iter()
            .map(|action| {
                keys.hint_for(
                    mkpclient_state_ui_keybindings::KeyContext::SelectionActionModal,
                    action,
                )
            })
            .collect(),
        }
    }
}

#[drv::memo(single)]
pub fn selection_action_modal_model(input: SelectionActionModalInput) -> SelectionActionModalModel {
    let mut rows: Vec<SelectionActionRow> = vec![
        SelectionActionRow {
            key: input.keys[0].clone(),
            label: "Play Next",
        },
        SelectionActionRow {
            key: input.keys[1].clone(),
            label: "Play Last",
        },
        SelectionActionRow {
            key: input.keys[2].clone(),
            label: "Add to Playlist",
        },
    ];
    if input.ctx_queue {
        rows.push(SelectionActionRow {
            key: input.keys[3].clone(),
            label: "Delete from Queue",
        });
    } else if input.ctx_middle && input.middle_is_playlist_songs {
        rows.push(SelectionActionRow {
            key: input.keys[3].clone(),
            label: "Remove from Playlist",
        });
    }
    SelectionActionModalModel {
        rows,
        count: input.count,
        selected: input.selected,
    }
}
