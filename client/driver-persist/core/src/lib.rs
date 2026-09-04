//! Sync core of the persist driver.
//!
//! `Persist` is the in-flight source the runtime tracks to dedupe
//! redundant load requests (the legacy TUI happened to call
//! `load_view` exactly once because `auto_restore` short-circuited
//! after the first hit; the driver version separates the wish from
//! the response, so a guard is needed).
//!
//! Saves are fire-and-forget. The worker reports `SaveFailed` for any
//! that error so the runtime can surface a toast.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use imbl::HashSet;
use serde::{Deserialize, Serialize};

// ─── Saved data shapes ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, drv::Input)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedView {
    Search {
        query: String,
        search_type: String, // "song" | "album" | "artist"
        selected: usize,
        offset: usize,
        selected_id: String,
    },
    AlbumDetail {
        album_id: String,
        album_name: String,
        selected: usize,
        offset: usize,
        selected_id: String,
    },
    ArtistDetail {
        artist_id: String,
        artist_name: String,
        selected: usize,
        offset: usize,
    },
    Playlist {
        playlist_id: String,
        selected: usize,
        offset: usize,
        selected_id: String,
    },
}

/// Identity of a `SavedView` excluding cursor / offset / song-id —
/// the bits that distinguish "which view is this" from "where is the
/// cursor inside it." The view-persist lifecycle diffs by this key so
/// cursor moves don't trigger a save on every key press, only mode
/// changes do.
#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum SavedViewKey {
    Search { query: String, search_type: String },
    AlbumDetail { album_id: String },
    ArtistDetail { artist_id: String },
    Playlist { playlist_id: String },
}

impl SavedView {
    pub fn key(&self) -> SavedViewKey {
        match self {
            SavedView::Search {
                query, search_type, ..
            } => SavedViewKey::Search {
                query: query.clone(),
                search_type: search_type.clone(),
            },
            SavedView::AlbumDetail { album_id, .. } => SavedViewKey::AlbumDetail {
                album_id: album_id.clone(),
            },
            SavedView::ArtistDetail { artist_id, .. } => SavedViewKey::ArtistDetail {
                artist_id: artist_id.clone(),
            },
            SavedView::Playlist { playlist_id, .. } => SavedViewKey::Playlist {
                playlist_id: playlist_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchHistory {
    pub items: Vec<SearchHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistoryItem {
    pub query: String,
    pub search_type: String, // "song" | "album" | "artist"
    pub ts: i64,
}

pub const SEARCH_HISTORY_LIMIT: usize = 10;

// ─── ABI ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum PersistCmd {
    LoadKeybindings,
    SaveKeybindings {
        keybindings: mkpclient_state_ui_keybindings::Keybindings,
    },
    LoadLastServer,
    SaveLastServer {
        name: String,
    },
    LoadView {
        backend: String,
    },
    SaveView {
        backend: String,
        view: SavedView,
    },
    ClearView {
        backend: String,
    },
    LoadSearchHistory {
        backend: String,
    },
    PushSearchHistory {
        backend: String,
        query: String,
        search_type: String,
    },
    LoadLastAddPlaylist {
        backend: String,
    },
    SaveLastAddPlaylist {
        backend: String,
        playlist_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum PersistEvent {
    KeybindingsSaved {
        keybindings: mkpclient_state_ui_keybindings::Keybindings,
    },
    KeybindingsLoaded {
        keybindings: mkpclient_state_ui_keybindings::Keybindings,
    },
    LastServerLoaded {
        name: Option<String>,
    },
    ViewLoaded {
        backend: String,
        view: Option<SavedView>,
    },
    SearchHistoryLoaded {
        backend: String,
        history: SearchHistory,
    },
    LastAddPlaylistLoaded {
        backend: String,
        id: Option<String>,
    },
    /// Any save-side error the runtime might want to surface as a toast.
    SaveFailed {
        op: &'static str,
        err: String,
    },
}

// ─── In-flight source ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LoadKey {
    Keybindings,
    LastServer,
    View(String),
    SearchHistory(String),
    LastAddPlaylist(String),
}

#[derive(Debug, Clone, Default)]
pub struct Persist {
    /// Set of loads the runtime has issued but the worker hasn't
    /// responded to yet. Insert before dispatching a `Load*` cmd;
    /// remove on the matching `*Loaded` event.
    pub loads_in_flight: HashSet<LoadKey>,
    /// Saves we sent but haven't yet seen the worker drain. Currently
    /// only useful for clean shutdown — the driver decrements on send,
    /// and a future shutdown path would block until 0.
    pub writes_pending: usize,
    /// Most recent `ViewLoaded` event the worker reported. The
    /// runtime's `lifecycle::restore` memo pair reads this and the
    /// trampoline clears it after applying so the next `LoadView`
    /// has a fresh slate.
    pub last_view_load: Option<ViewLoadResult>,
    /// Identity of the `SavedView` we most recently shipped via
    /// `SaveView` for the active backend. The view-persist lifecycle
    /// diffs `desired_view_key` against this to decide whether to
    /// fire a save. Reset to `None` when `backend_name` changes so
    /// the next backend's first save fires unconditionally.
    pub last_view_saved_key: Option<SavedViewKey>,
    /// Playlist id we most recently shipped via
    /// `SaveLastAddPlaylist` for the active backend. The
    /// last-add-persist lifecycle diffs `picker.last_add_playlist`
    /// against this. Reset to `None` when `backend_name` changes.
    pub last_add_playlist_saved: Option<String>,
    /// Task id of the last `Search::begin()` we shipped a
    /// `PushSearchHistory` for. Reset to `None` on backend change so
    /// the next backend's first search pushes unconditionally.
    pub last_pushed_search_task: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub struct ViewLoadResult {
    pub backend: String,
    pub view: Option<SavedView>,
}

impl Persist {
    pub fn is_loading(&self, key: &LoadKey) -> bool {
        self.loads_in_flight.contains(key)
    }
}

// ─── Trace ──────────────────────────────────────────────────────────

pub trait Trace: Send + Sync {
    fn persist_load(&self, _key: &LoadKey) {}
    fn persist_loaded(&self, _key: &LoadKey) {}
    fn persist_save(&self, _op: &'static str) {}
    fn persist_error(&self, _op: &'static str, _err: &str) {}
}

pub struct NoopTrace;
impl Trace for NoopTrace {}

// ─── Driver handle ──────────────────────────────────────────────────

pub struct PersistDriver {
    cmd_tx: Sender<PersistCmd>,
    event_rx: Receiver<PersistEvent>,
    trace: Arc<dyn Trace>,
}

impl PersistDriver {
    pub fn new(
        cmd_tx: Sender<PersistCmd>,
        event_rx: Receiver<PersistEvent>,
        trace: Arc<dyn Trace>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            trace,
        }
    }

    /// Ship commands to the worker. Silently no-ops if the worker
    /// hung up.
    pub fn execute<'a, I>(&self, cmds: I)
    where
        I: IntoIterator<Item = &'a PersistCmd>,
    {
        for cmd in cmds {
            match cmd {
                PersistCmd::LoadKeybindings => self.trace.persist_load(&LoadKey::Keybindings),
                PersistCmd::LoadLastServer => self.trace.persist_load(&LoadKey::LastServer),
                PersistCmd::LoadView { backend } => {
                    self.trace.persist_load(&LoadKey::View(backend.clone()))
                }
                PersistCmd::LoadSearchHistory { backend } => self
                    .trace
                    .persist_load(&LoadKey::SearchHistory(backend.clone())),
                PersistCmd::LoadLastAddPlaylist { backend } => self
                    .trace
                    .persist_load(&LoadKey::LastAddPlaylist(backend.clone())),
                PersistCmd::SaveLastServer { .. } => self.trace.persist_save("save_last_server"),
                PersistCmd::SaveKeybindings { .. } => self.trace.persist_save("save_keybindings"),
                PersistCmd::SaveView { .. } => self.trace.persist_save("save_view"),
                PersistCmd::ClearView { .. } => self.trace.persist_save("clear_view"),
                PersistCmd::PushSearchHistory { .. } => {
                    self.trace.persist_save("push_search_history")
                }
                PersistCmd::SaveLastAddPlaylist { .. } => {
                    self.trace.persist_save("save_last_add_playlist")
                }
            }
            if self.cmd_tx.send(cmd.clone()).is_err() {
                return;
            }
        }
    }

    pub fn process(&self) -> Vec<PersistEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                PersistEvent::KeybindingsLoaded { .. } => {
                    self.trace.persist_loaded(&LoadKey::Keybindings)
                }
                PersistEvent::KeybindingsSaved { .. } => {}
                PersistEvent::LastServerLoaded { .. } => {
                    self.trace.persist_loaded(&LoadKey::LastServer)
                }
                PersistEvent::ViewLoaded { backend, .. } => {
                    self.trace.persist_loaded(&LoadKey::View(backend.clone()))
                }
                PersistEvent::SearchHistoryLoaded { backend, .. } => self
                    .trace
                    .persist_loaded(&LoadKey::SearchHistory(backend.clone())),
                PersistEvent::LastAddPlaylistLoaded { backend, .. } => self
                    .trace
                    .persist_loaded(&LoadKey::LastAddPlaylist(backend.clone())),
                PersistEvent::SaveFailed { op, err } => self.trace.persist_error(op, err),
            }
            out.push(ev);
        }
        out
    }
}
