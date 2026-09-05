//! Filesystem-backed persist native.
//!
//! Storage layout (same shape the legacy mkpclient used so existing
//! state survives the cutover):
//!
//! ```text
//! ~/.config/mkp/
//!   last_server                    plain text — preferred server NAME
//!   {server_name}/
//!     search_history               TOML — SearchHistory
//!     last_add_playlist            plain text — playlist id
//!     {music_backend}/
//!       last_view                  TOML — SavedView
//! ```
//!
//! The view sits under the music backend because album / artist /
//! playlist ids belong to a catalogue, not to a server: the same
//! server serves a different one after a backend swap. Releases up
//! to 1.0.0 wrote `{server_name}/last_view`, which `load_view`
//! still reads when no backend-qualified view exists yet.
//!
//! Server / backend names are sanitised for filesystem safety
//! (no `/`, `\`, leading `.`). Atomic writes via temp-file-and-rename.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use log::warn;

use mkpclient_core::Notifier;
use mkpclient_driver_persist_core::{
    PersistCmd, PersistDriver, PersistEvent, SearchHistory, SearchHistoryItem, Trace, ViewKey,
    SEARCH_HISTORY_LIMIT,
};

pub struct PersistNative {
    _marker: (),
}

pub fn spawn(trace: Arc<dyn Trace>, notify: Notifier) -> (PersistDriver, PersistNative) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<PersistCmd>();
    let (event_tx, event_rx) = mpsc::channel::<PersistEvent>();

    thread::Builder::new()
        .name("mkp-persist".into())
        .spawn(move || worker_loop(cmd_rx, event_tx, notify))
        .expect("spawning persist worker should succeed");

    let driver = PersistDriver::new(cmd_tx, event_rx, trace);
    (driver, PersistNative { _marker: () })
}

fn worker_loop(rx: Receiver<PersistCmd>, tx: Sender<PersistEvent>, notify: Notifier) {
    while let Ok(cmd) = rx.recv() {
        let event = handle(cmd);
        if let Some(ev) = event {
            if tx.send(ev).is_err() {
                return;
            }
            notify.notify();
        }
    }
}

fn handle(cmd: PersistCmd) -> Option<PersistEvent> {
    match cmd {
        PersistCmd::LoadKeybindings => Some(PersistEvent::KeybindingsLoaded {
            keybindings: load_keybindings(),
        }),
        PersistCmd::SaveKeybindings { keybindings } => match save_keybindings(&keybindings) {
            Ok(()) => Some(PersistEvent::KeybindingsSaved { keybindings }),
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "save_keybindings",
                err,
            }),
        },
        PersistCmd::LoadLastServer => Some(PersistEvent::LastServerLoaded {
            name: load_last_server(),
        }),
        PersistCmd::SaveLastServer { name } => match save_last_server(&name) {
            Ok(()) => None,
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "save_last_server",
                err,
            }),
        },
        PersistCmd::LoadView { key } => Some(PersistEvent::ViewLoaded {
            view: load_view(&key),
            key,
        }),
        PersistCmd::SaveView { key, view } => match save_view(&key, &view) {
            Ok(()) => None,
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "save_view",
                err,
            }),
        },
        PersistCmd::ClearView { key } => {
            clear_view(&key);
            None
        }
        PersistCmd::LoadSearchHistory { backend } => Some(PersistEvent::SearchHistoryLoaded {
            history: load_search_history(&backend),
            backend,
        }),
        PersistCmd::PushSearchHistory {
            backend,
            query,
            search_type,
        } => match push_search_history(&backend, &query, &search_type) {
            Ok(()) => None,
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "push_search_history",
                err,
            }),
        },
        PersistCmd::LoadLastAddPlaylist { backend } => Some(PersistEvent::LastAddPlaylistLoaded {
            id: load_last_add_playlist(&backend),
            backend,
        }),
        PersistCmd::SaveLastAddPlaylist {
            backend,
            playlist_id,
        } => match save_last_add_playlist(&backend, &playlist_id) {
            Ok(()) => None,
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "save_last_add_playlist",
                err,
            }),
        },
    }
}

// ─── path helpers ───────────────────────────────────────────────────

fn config_dir() -> Option<PathBuf> {
    if let Some(config) = std::env::var_os("MKP_CONFIG_HOME") {
        return Some(PathBuf::from(config));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("mkp"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("mkp"))
}

fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c => c,
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string()
}

fn backend_dir(backend: &str) -> Option<PathBuf> {
    Some(config_dir()?.join(sanitise(backend)))
}

fn last_server_path() -> Option<PathBuf> {
    Some(config_dir()?.join("last_server"))
}

fn keybindings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("keybindings.toml"))
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all().ok();
    }
    std::fs::rename(tmp, path)
}

fn load_keybindings() -> mkpclient_state_ui_keybindings::Keybindings {
    let defaults = mkpclient_state_ui_keybindings::Keybindings::defaults();
    let Some(path) = keybindings_path() else {
        return defaults;
    };
    load_keybindings_from(&path)
}

fn load_keybindings_from(path: &std::path::Path) -> mkpclient_state_ui_keybindings::Keybindings {
    let defaults = mkpclient_state_ui_keybindings::Keybindings::defaults();
    if !path.exists() {
        if let Err(err) = atomic_write(path, defaults.to_toml().as_bytes()) {
            warn!("persist: failed to write default keybindings: {err}");
        }
        return defaults;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return defaults;
    };
    let Ok(merged) = mkpclient_state_ui_keybindings::Keybindings::merge_toml(&content) else {
        warn!("persist: failed to parse keybindings TOML");
        return defaults;
    };
    if let Err(err) = atomic_write(path, merged.to_toml().as_bytes()) {
        warn!("persist: failed to write merged keybindings: {err}");
    }
    merged
}

fn save_keybindings(
    keybindings: &mkpclient_state_ui_keybindings::Keybindings,
) -> Result<(), String> {
    let Some(path) = keybindings_path() else {
        return Err("no config dir".into());
    };
    atomic_write(&path, keybindings.to_toml().as_bytes()).map_err(|e| e.to_string())
}

// ─── last_server ────────────────────────────────────────────────────

fn load_last_server() -> Option<String> {
    let p = last_server_path()?;
    let s = std::fs::read_to_string(p).ok()?;
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn save_last_server(name: &str) -> Result<(), String> {
    let Some(p) = last_server_path() else {
        return Err("no config dir".into());
    };
    atomic_write(&p, name.as_bytes()).map_err(|e| e.to_string())
}

// ─── last_view ──────────────────────────────────────────────────────

fn view_path(key: &ViewKey) -> Option<PathBuf> {
    Some(
        backend_dir(&key.server)?
            .join(sanitise(&key.backend))
            .join("last_view"),
    )
}

/// Where releases up to 1.0.0 kept the view: one per server, with no
/// music-backend dimension.
fn legacy_view_path(key: &ViewKey) -> Option<PathBuf> {
    Some(backend_dir(&key.server)?.join("last_view"))
}

fn read_view(p: &std::path::Path) -> Option<mkpclient_driver_persist_core::SavedView> {
    let text = std::fs::read_to_string(p).ok()?;
    toml::from_str(&text).ok()
}

/// Take over a view written by a release up to 1.0.0.
///
/// That file has no backend dimension, so nothing records which
/// catalogue it came from. Exactly one backend may inherit it — the
/// first to ask, which on an upgrade is whichever the server is
/// already on. Moving it into that backend's directory keeps the
/// user's place while stopping the next backend from inheriting ids
/// out of a catalogue it has never heard of.
fn claim_legacy_view(key: &ViewKey) -> Option<mkpclient_driver_persist_core::SavedView> {
    let legacy = legacy_view_path(key)?;
    let view = read_view(&legacy)?;
    if let Some(dest) = view_path(key) {
        match toml::to_string(&view)
            .map_err(|e| e.to_string())
            .and_then(|text| atomic_write(&dest, text.as_bytes()).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                let _ = std::fs::remove_file(&legacy);
            }
            // Return the view anyway — a blank pane helps nobody —
            // and leave the file for the next run to claim.
            Err(e) => warn!("could not migrate the 1.0.0 view for {}: {e}", key.server),
        }
    }
    Some(view)
}

fn load_view(key: &ViewKey) -> Option<mkpclient_driver_persist_core::SavedView> {
    if let Some(p) = view_path(key) {
        if let Some(view) = read_view(&p) {
            return Some(view);
        }
    }
    claim_legacy_view(key)
}

fn save_view(key: &ViewKey, view: &mkpclient_driver_persist_core::SavedView) -> Result<(), String> {
    let Some(p) = view_path(key) else {
        return Err("no config dir".into());
    };
    let text = toml::to_string(view).map_err(|e| e.to_string())?;
    atomic_write(&p, text.as_bytes()).map_err(|e| e.to_string())
}

fn clear_view(key: &ViewKey) {
    if let Some(p) = view_path(key) {
        let _ = std::fs::remove_file(p);
    }
    // The unclaimed 1.0.0 file is deliberately left alone: it is not
    // this backend's to delete, and a later `load_view` returning it
    // is the inheritance rule working, not a resurrection.
}

// ─── search_history ─────────────────────────────────────────────────

fn load_search_history(backend: &str) -> SearchHistory {
    backend_dir(backend)
        .map(|d| d.join("search_history"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn push_search_history(backend: &str, query: &str, search_type: &str) -> Result<(), String> {
    let mut hist = load_search_history(backend);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    hist.items
        .retain(|i| !(i.query == query && i.search_type == search_type));
    hist.items.insert(
        0,
        SearchHistoryItem {
            query: query.to_string(),
            search_type: search_type.to_string(),
            ts: now,
        },
    );
    hist.items.truncate(SEARCH_HISTORY_LIMIT);

    let Some(p) = backend_dir(backend) else {
        return Err("no config dir".into());
    };
    let p = p.join("search_history");
    let text = toml::to_string(&hist).map_err(|e| e.to_string())?;
    atomic_write(&p, text.as_bytes()).map_err(|e| {
        warn!("persist: push_search_history write failed: {e}");
        e.to_string()
    })
}

// ─── last_add_playlist ──────────────────────────────────────────────

fn load_last_add_playlist(backend: &str) -> Option<String> {
    let p = backend_dir(backend)?.join("last_add_playlist");
    let s = std::fs::read_to_string(p).ok()?;
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn save_last_add_playlist(backend: &str, playlist_id: &str) -> Result<(), String> {
    let Some(p) = backend_dir(backend) else {
        return Err("no config dir".into());
    };
    atomic_write(&p.join("last_add_playlist"), playlist_id.as_bytes()).map_err(|e| e.to_string())
}

/// Serialises every test in this binary that touches the process
/// environment. `std::env::set_var` races *any* concurrent read of
/// the environment, not just of the same variable — and
/// `tempfile::tempdir()` reads `TMPDIR` — so the tempdir has to be
/// created inside the critical section too, not before entering it.
#[cfg(test)]
fn with_env_lock<T>(f: impl FnOnce(&tempfile::TempDir) -> T) -> T {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    f(&dir)
}

#[cfg(test)]
mod keybinding_tests {
    use super::*;
    use mkpclient_state_ui_keybindings::{Action, KeyChord, KeyContext, Keybindings};

    #[test]
    fn save_then_reload_preserves_binding_and_writes_merged_defaults() {
        with_env_lock(|dir| {
            let path = dir.path().join("keybindings.toml");
            let mut keys = Keybindings::defaults();
            keys.replace(KeyContext::Global, Action::PlayPause, KeyChord::char('p'));
            atomic_write(&path, keys.to_toml().as_bytes()).unwrap();

            let loaded = load_keybindings_from(&path);
            assert_eq!(
                loaded.keys_for(KeyContext::Global, Action::PlayPause),
                vec![KeyChord::char('p')]
            );
            assert!(std::fs::read_to_string(path).unwrap().contains("move_up"));
        })
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use mkpclient_driver_persist_core::SavedView;

    /// Run `f` with `config_dir()` pointed at a fresh tempdir, under
    /// the binary-wide environment lock.
    fn with_config_root<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        with_env_lock(|dir| {
            std::env::set_var("MKP_CONFIG_HOME", dir.path());
            let out = f(dir.path());
            std::env::remove_var("MKP_CONFIG_HOME");
            out
        })
    }

    fn view(playlist_id: &str) -> SavedView {
        SavedView::Playlist {
            playlist_id: playlist_id.into(),
            selected: 0,
            offset: 0,
            selected_id: String::new(),
        }
    }

    fn playlist_id_of(v: &SavedView) -> &str {
        match v {
            SavedView::Playlist { playlist_id, .. } => playlist_id,
            other => panic!("expected a playlist view, got {other:?}"),
        }
    }

    #[test]
    fn views_of_two_backends_on_one_server_do_not_collide() {
        with_config_root(|root| {
            let mk = ViewKey::new("living-room", "musickit");
            let tidal = ViewKey::new("living-room", "tidal");
            save_view(&mk, &view("mk-playlist")).unwrap();
            save_view(&tidal, &view("tidal-playlist")).unwrap();

            assert_eq!(playlist_id_of(&load_view(&mk).unwrap()), "mk-playlist");
            assert_eq!(
                playlist_id_of(&load_view(&tidal).unwrap()),
                "tidal-playlist"
            );
            assert!(root
                .join("living-room")
                .join("musickit")
                .join("last_view")
                .exists());
        })
    }

    /// Writes a 1.0.0-shaped view: one per server, no backend segment.
    fn write_legacy_view(root: &std::path::Path, server: &str, playlist_id: &str) -> PathBuf {
        let legacy = root.join(server).join("last_view");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, toml::to_string(&view(playlist_id)).unwrap()).unwrap();
        legacy
    }

    #[test]
    fn a_1_0_0_view_is_claimed_by_the_backend_that_reads_it_first() {
        with_config_root(|root| {
            let legacy = write_legacy_view(root, "study", "from-1-0-0");

            let key = ViewKey::new("study", "musickit");
            assert_eq!(
                playlist_id_of(&load_view(&key).unwrap()),
                "from-1-0-0",
                "upgrading must not drop the view the user left behind"
            );
            assert!(
                !legacy.exists(),
                "reading it moves it under the backend that claimed it"
            );
            assert!(root
                .join("study")
                .join("musickit")
                .join("last_view")
                .exists());

            // And it stays that backend's view from then on.
            save_view(&key, &view("current")).unwrap();
            assert_eq!(playlist_id_of(&load_view(&key).unwrap()), "current");
        })
    }

    /// The legacy file has no music-backend dimension, so nothing
    /// records which catalogue it came from. Serving it to *every*
    /// backend that lacks a view of its own re-creates the exact
    /// failure that keying the view by backend exists to prevent: a
    /// MusicKit album / artist id restored under Tidal, which is a
    /// request that could only fail.
    ///
    /// The compat requirement is that the upgrade doesn't lose the
    /// user's view — one backend claims it. Handing the same file to
    /// the next backend as well is not compat, it's the bug.
    #[test]
    fn a_1_0_0_view_is_not_handed_to_a_second_backend() {
        with_config_root(|root| {
            write_legacy_view(root, "hall", "from-1-0-0");

            // Upgrade. The backend that happened to be live reads the
            // legacy view and, on the next navigation, writes it under
            // its own path.
            let mk = ViewKey::new("hall", "musickit");
            let migrated = load_view(&mk).expect("the live backend picks the legacy view up");
            save_view(&mk, &migrated).unwrap();

            // Now the server swaps. Tidal has never had a view here, and
            // the 1.0.0 file holds ids from a catalogue it cannot resolve.
            assert!(
                load_view(&ViewKey::new("hall", "tidal")).is_none(),
                "a legacy view another backend already claimed must not be \
                 restored under a different backend"
            );
        })
    }

    #[test]
    fn clearing_a_view_leaves_the_other_backend_alone() {
        with_config_root(|_root| {
            let mk = ViewKey::new("den", "musickit");
            let tidal = ViewKey::new("den", "tidal");
            save_view(&mk, &view("mk-playlist")).unwrap();
            save_view(&tidal, &view("tidal-playlist")).unwrap();

            clear_view(&mk);
            assert!(load_view(&mk).is_none());
            assert_eq!(
                playlist_id_of(&load_view(&tidal).unwrap()),
                "tidal-playlist"
            );
        })
    }
}
