//! User-decision source: which screen / modal is active.
//!
//! `Screen` is the "what is the user looking at right now?" answer.
//! Modals own their own ephemeral input state (filter text, search
//! query, action-menu cursor, action-item payload, etc.) on the
//! same enum so the decision and its inputs can't get out of sync.

use std::sync::Arc;

use imbl::Vector;
use mkpclient_state_ui_filter::FilterTarget;
use mkpclient_state_ui_keybindings::Keybindings;
use mkproto::{MediaKind, SearchType};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    NowPlaying,
    /// Full-screen search input overlay.
    SearchInput(SearchState),
    /// Context menu on a song / album / artist row.
    ActionModal(ActionModalState),
    /// Live-filter text input for the focused pane (middle / queue).
    FilterInput(FilterState),
    /// Key cheat-sheet overlay (scrollable).
    HelpOverlay {
        scroll: u16,
    },
    KeybindingsEditor(KeybindingsEditorState),
    /// New-playlist name input. `add_item` is the song/album that
    /// the user was trying to add when they hit "+ New playlist"
    /// in the picker — once the server confirms the new playlist,
    /// the deferred add fires.
    CreatePlaylist {
        input: Arc<str>,
        add_item: Option<ActionItem>,
    },
    /// Rename-playlist name input; `id` is the target.
    RenamePlaylist {
        id: Arc<str>,
        original: Arc<str>,
        input: Arc<str>,
    },
    /// Per-playlist context menu (Rename / Delete).
    PlaylistAction {
        playlist_id: Arc<str>,
        playlist_name: Arc<str>,
        selected: usize,
    },
    /// "Add to playlist" picker — lists playlists + "+ New…".
    PlaylistPicker {
        item: ActionItem,
        selected: usize,
    },
    /// Confirm remove-song-from-playlist.
    ConfirmRemoveFromPlaylist {
        playlist_id: Arc<str>,
        song_index: usize,
        song_title: Arc<str>,
    },
    /// Type-to-confirm delete — user must re-type the playlist name.
    ConfirmDeletePlaylist {
        id: Arc<str>,
        name: Arc<str>,
        input: Arc<str>,
    },
    /// Bulk action menu shown when the user presses Tab while
    /// selection mode is active.
    SelectionActionModal {
        selected: usize,
    },
    /// Server-error modal — shown when the server replies with
    /// `ServerMsg::Error` to a request. Esc closes; `c` copies.
    ErrorModal {
        message: Arc<str>,
    },
    /// Connection-lost modal — shown when the link drops while we
    /// had an active backend, then auto-clears once the link
    /// reconnects.
    ServerLostModal {
        server: Arc<str>,
    },
    /// Server-picker modal opened from pressing Enter on the
    /// connected-server row in the left pane. Layered on top of
    /// the main view so the queue / playlist tracks stay visible
    /// while the user picks a different server. Selecting the
    /// already-connected server just closes the modal; selecting a
    /// different server triggers a disconnect + reconnect to the
    /// new target.
    ServerPicker {
        selected: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingsEditorState {
    pub help_scroll: u16,
    pub selected_context: usize,
    pub selected_binding: usize,
    pub listening: bool,
    pub adding: bool,
    pub focus_right: bool,
    pub draft: Keybindings,
}

impl KeybindingsEditorState {
    pub fn new(draft: Keybindings, help_scroll: u16) -> Self {
        Self {
            help_scroll,
            selected_context: 0,
            selected_binding: 0,
            listening: false,
            adding: false,
            focus_right: false,
            draft,
        }
    }
}

/// TUI-local kind tag for the action menu. The wire-protocol
/// `MediaKind` only knows Song/Album/Playlist; we need an Artist
/// variant for the action menu (Copy Link on artist rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Song,
    Album,
    Artist,
}

impl ActionKind {
    pub fn to_media(self) -> Option<MediaKind> {
        match self {
            ActionKind::Song => Some(MediaKind::Song),
            ActionKind::Album => Some(MediaKind::Album),
            ActionKind::Artist => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOrigin {
    /// Middle pane showing playlist songs (Remove → from playlist).
    PlaylistSongs,
    /// Middle pane showing search/album/artist (no remove).
    OtherMiddle,
    /// Queue pane (Remove → from queue).
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub id: Arc<str>,
    pub kind: ActionKind,
    pub label: Arc<str>,
    /// Streaming URL (used for Copy Link).
    pub url: Option<Arc<str>>,
    /// For songs: the album id (if known) used by Go-to-Album.
    pub album_id: Option<Arc<str>>,
    /// For songs/albums: the artist id (if known) used by Go-to-Artist.
    pub artist_id: Option<Arc<str>>,
    /// Album name for title bar when drilling via Go-to-Album.
    pub album_title: Option<Arc<str>>,
    /// Artist name (same idea).
    pub artist_label: Option<Arc<str>>,
    /// For PlaylistSongs mode — the playlist id this song belongs
    /// to, so Remove can dispatch the right message.
    pub playlist_id: Option<Arc<str>>,
    /// Index in the current view (used by Remove to identify the
    /// row in the server-side playlist).
    pub view_index: Option<usize>,
    /// Origin pane — needed so the menu can show "Remove from queue"
    /// only on Queue rows.
    pub origin: ActionOrigin,
}

impl ActionItem {
    pub fn new(id: String, kind: ActionKind, label: String) -> Self {
        Self {
            id: Arc::from(id),
            kind,
            label: Arc::from(label),
            url: None,
            album_id: None,
            artist_id: None,
            album_title: None,
            artist_label: None,
            playlist_id: None,
            view_index: None,
            origin: ActionOrigin::OtherMiddle,
        }
    }

    pub fn with_playlist(mut self, playlist_id: String, view_index: usize) -> Self {
        self.playlist_id = Some(Arc::from(playlist_id));
        self.view_index = Some(view_index);
        self.origin = ActionOrigin::PlaylistSongs;
        self
    }

    pub fn with_origin(mut self, origin: ActionOrigin) -> Self {
        self.origin = origin;
        self
    }

    pub fn with_url(mut self, url: Option<String>) -> Self {
        self.url = url.map(Arc::from);
        self
    }

    pub fn with_album(mut self, album_id: Option<String>, album_title: Option<String>) -> Self {
        self.album_id = album_id.map(Arc::from);
        self.album_title = album_title.map(Arc::from);
        self
    }

    pub fn with_artist(mut self, artist_id: Option<String>, artist_label: Option<String>) -> Self {
        self.artist_id = artist_id.map(Arc::from);
        self.artist_label = artist_label.map(Arc::from);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionModalState {
    pub item: ActionItem,
    pub selected: usize,
}

impl ActionModalState {
    pub fn menu(&self) -> Vec<(char, &'static str)> {
        let has_url = self.item.url.is_some();
        // Artist-only menu: just Copy Link if we have a URL.
        if self.item.kind == ActionKind::Artist {
            let mut v = Vec::new();
            if has_url {
                v.push(('c', "Copy Link"));
            }
            return v;
        }
        let mut items: Vec<(char, &'static str)> = vec![
            ('q', "Go to Artist"),
            ('w', "Go to Album"),
            ('n', "Play Next"),
            ('e', "Play Last"),
            ('a', "Add to Playlist"),
        ];
        if has_url {
            items.push(('c', "Copy Link"));
        }
        match self.item.origin {
            ActionOrigin::PlaylistSongs => items.push(('d', "Remove from Playlist")),
            ActionOrigin::Queue => items.push(('d', "Remove from Queue")),
            ActionOrigin::OtherMiddle => {}
        }
        items
    }

    pub fn len(&self) -> usize {
        self.menu().len()
    }

    pub fn is_empty(&self) -> bool {
        self.menu().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterState {
    pub target: FilterTarget,
    pub input: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchState {
    pub input: Arc<str>,
    pub last_type: SearchType,
    /// Persisted search history, loaded once when the modal opens.
    pub history: Vector<SearchHistoryItem>,
    /// Cursor index into the persisted search history. `None` means
    /// the input field owns focus; arrow keys move us into the list.
    pub history_selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHistoryItem {
    pub query: Arc<str>,
    pub search_type: Arc<str>,
    pub ts: i64,
}
