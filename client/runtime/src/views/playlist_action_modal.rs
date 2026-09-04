//! View model for the per-playlist action chooser
//! (`Screen::PlaylistAction`).
//!
//! Two-row menu: Rename / Delete. Memoising the row list keeps it
//! one source of truth for both the renderer and the dispatch
//! handler — they agree on the hot-key set without sharing a const.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistActionRow {
    pub key: String,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistActionModalModel {
    pub rows: Vec<PlaylistActionRow>,
    pub selected: usize,
}

#[derive(drv::Input)]
pub struct PlaylistActionModalInput {
    pub selected: usize,
    pub rename_key: String,
    pub delete_key: String,
}

impl PlaylistActionModalInput {
    pub fn new(selected: usize, keys: &mkpclient_state_ui_keybindings::Keybindings) -> Self {
        use mkpclient_state_ui_keybindings::{Action, KeyContext};
        Self {
            selected,
            rename_key: keys.hint_for(
                KeyContext::PlaylistActionModal,
                Action::PlaylistActionRename,
            ),
            delete_key: keys.hint_for(
                KeyContext::PlaylistActionModal,
                Action::PlaylistActionDelete,
            ),
        }
    }
}

#[drv::memo(single)]
pub fn playlist_action_modal_model(input: PlaylistActionModalInput) -> PlaylistActionModalModel {
    PlaylistActionModalModel {
        rows: vec![
            PlaylistActionRow {
                key: input.rename_key,
                label: "Rename Playlist",
            },
            PlaylistActionRow {
                key: input.delete_key,
                label: "Delete Playlist",
            },
        ],
        selected: input.selected,
    }
}
