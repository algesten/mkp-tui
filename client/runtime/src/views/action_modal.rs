//! View model for the per-row action modal (`Screen::ActionModal`).
//!
//! Per spec §4 every view is a `#[drv::memo]`. The action menu's
//! contents are derived from `ActionItem.kind` + `origin` —
//! pre-rendering the (key, label) list here keeps the renderer a
//! pure painter, and means the menu's hot-key set is one source of
//! truth for both the renderer and the dispatch handler.

use mkpclient_state_ui_keybindings::{Action, KeyContext, Keybindings};
use mkpclient_state_ui_screen::{ActionItem, ActionKind, ActionModalState, ActionOrigin};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ActionModalRow {
    pub key: String,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ActionModalModel {
    pub rows: Vec<ActionModalRow>,
    pub selected: usize,
}

#[derive(drv::Input)]
pub struct ActionModalInput {
    pub kind_artist: bool,
    pub has_url: bool,
    pub origin_playlist_songs: bool,
    pub origin_queue: bool,
    pub selected: usize,
    pub keys: Vec<String>,
}

impl ActionModalInput {
    pub fn new(state: &ActionModalState, keybindings: &Keybindings) -> Self {
        let item: &ActionItem = &state.item;
        Self {
            kind_artist: matches!(item.kind, ActionKind::Artist),
            has_url: item.url.is_some(),
            origin_playlist_songs: matches!(item.origin, ActionOrigin::PlaylistSongs),
            origin_queue: matches!(item.origin, ActionOrigin::Queue),
            selected: state.selected,
            keys: [
                Action::ActionGoToArtist,
                Action::ActionGoToAlbum,
                Action::ActionPlayNext,
                Action::ActionPlayLast,
                Action::ActionAddToPlaylist,
                Action::ActionCopyLink,
                Action::ActionRemove,
            ]
            .into_iter()
            .map(|action| keybindings.hint_for(KeyContext::ActionModal, action))
            .collect(),
        }
    }
}

#[drv::memo(single)]
pub fn action_modal_model(input: ActionModalInput) -> ActionModalModel {
    let mut rows: Vec<ActionModalRow> = Vec::new();
    if input.kind_artist {
        if input.has_url {
            rows.push(ActionModalRow {
                key: input.keys[5].clone(),
                label: "Copy Link",
            });
        }
    } else {
        rows.push(ActionModalRow {
            key: input.keys[0].clone(),
            label: "Go to Artist",
        });
        rows.push(ActionModalRow {
            key: input.keys[1].clone(),
            label: "Go to Album",
        });
        rows.push(ActionModalRow {
            key: input.keys[2].clone(),
            label: "Play Next",
        });
        rows.push(ActionModalRow {
            key: input.keys[3].clone(),
            label: "Play Last",
        });
        rows.push(ActionModalRow {
            key: input.keys[4].clone(),
            label: "Add to Playlist",
        });
        if input.has_url {
            rows.push(ActionModalRow {
                key: input.keys[5].clone(),
                label: "Copy Link",
            });
        }
        if input.origin_playlist_songs {
            rows.push(ActionModalRow {
                key: input.keys[6].clone(),
                label: "Remove from Playlist",
            });
        } else if input.origin_queue {
            rows.push(ActionModalRow {
                key: input.keys[6].clone(),
                label: "Remove from Queue",
            });
        }
    }
    ActionModalModel {
        rows,
        selected: input.selected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkpclient_state_ui_screen::ActionKind;

    fn build(kind: ActionKind, origin: ActionOrigin, has_url: bool) -> ActionModalState {
        let mut item = ActionItem::new("id".into(), kind, "label".into());
        if has_url {
            item = item.with_url(Some("https://example".into()));
        }
        item = item.with_origin(origin);
        ActionModalState { item, selected: 0 }
    }

    #[test]
    fn artist_with_url_offers_only_copy_link() {
        let s = build(ActionKind::Artist, ActionOrigin::OtherMiddle, true);
        let m = action_modal_model(ActionModalInput::new(&s, &Keybindings::defaults()));
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].key, "c");
    }

    #[test]
    fn artist_without_url_has_empty_menu() {
        let s = build(ActionKind::Artist, ActionOrigin::OtherMiddle, false);
        let m = action_modal_model(ActionModalInput::new(&s, &Keybindings::defaults()));
        assert!(m.rows.is_empty());
    }

    #[test]
    fn song_in_playlist_offers_remove() {
        let s = build(ActionKind::Song, ActionOrigin::PlaylistSongs, false);
        let m = action_modal_model(ActionModalInput::new(&s, &Keybindings::defaults()));
        assert_eq!(m.rows.last().map(|r| r.key.as_str()), Some("d"));
        assert_eq!(m.rows.last().map(|r| r.label), Some("Remove from Playlist"));
    }

    #[test]
    fn song_in_queue_offers_remove_from_queue() {
        let s = build(ActionKind::Song, ActionOrigin::Queue, true);
        let m = action_modal_model(ActionModalInput::new(&s, &Keybindings::defaults()));
        assert_eq!(m.rows.last().map(|r| r.label), Some("Remove from Queue"));
    }

    #[test]
    fn other_middle_has_no_remove() {
        let s = build(ActionKind::Album, ActionOrigin::OtherMiddle, true);
        let m = action_modal_model(ActionModalInput::new(&s, &Keybindings::defaults()));
        assert!(!m.rows.iter().any(|r| r.key == "d"));
    }
}
