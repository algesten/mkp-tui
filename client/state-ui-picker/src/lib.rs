//! User-decision source: playlist-picker selection + "create +
//! add" deferred breadcrumb.
//!
//! The `selected` index drives the playlist column overlay cursor
//! while the picker is open. When the user reaches "+ New…" and
//! types a name, `pending_create_add` is populated so the next
//! `PlaylistCreated` broadcast can fire the deferred AddToPlaylist.
//!
//! `last_add_playlist` is a persisted-per-backend hint loaded at
//! connect time so the picker opens with the user's last choice
//! pre-selected. Dispatch updates it on every successful add.

use mkpclient_state_ui_screen::ActionItem;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiPicker {
    /// Picker overlay cursor index (0..N for playlists, N for
    /// "+ New…"). Only meaningful while the picker is the active
    /// screen.
    pub selected: usize,
    /// Last playlist id added to (per backend). Used as the picker's
    /// initial cursor target.
    pub last_add_playlist: Option<std::sync::Arc<str>>,
    /// Deferred AddToPlaylist when the user creates a new playlist
    /// from inside the picker. Held until the server confirms the
    /// new playlist and the deferred add fires.
    pub pending_create_add: Option<PendingCreateAdd>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCreateAdd {
    pub name: std::sync::Arc<str>,
    pub item: ActionItem,
}
