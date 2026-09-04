//! Filesystem-backed persist native.
//!
//! Storage layout (same shape the legacy mkpclient used so existing
//! state survives the cutover):
//!
//! ```text
//! ~/.config/mkp/
//!   last_server                    plain text — preferred server NAME
//!   {backend_name}/
//!     last_view                    TOML — SavedView (per-backend)
//!     search_history               TOML — SearchHistory
//!     last_add_playlist            plain text — playlist id
//! ```
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
    PersistCmd, PersistDriver, PersistEvent, SearchHistory, SearchHistoryItem, Trace,
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
        PersistCmd::LoadView { backend } => Some(PersistEvent::ViewLoaded {
            view: load_view(&backend),
            backend,
        }),
        PersistCmd::SaveView { backend, view } => match save_view(&backend, &view) {
            Ok(()) => None,
            Err(err) => Some(PersistEvent::SaveFailed {
                op: "save_view",
                err,
            }),
        },
        PersistCmd::ClearView { backend } => {
            clear_view(&backend);
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

fn load_view(backend: &str) -> Option<mkpclient_driver_persist_core::SavedView> {
    let p = backend_dir(backend)?.join("last_view");
    let text = std::fs::read_to_string(p).ok()?;
    toml::from_str(&text).ok()
}

fn save_view(backend: &str, view: &mkpclient_driver_persist_core::SavedView) -> Result<(), String> {
    let Some(p) = backend_dir(backend) else {
        return Err("no config dir".into());
    };
    let p = p.join("last_view");
    let text = toml::to_string(view).map_err(|e| e.to_string())?;
    atomic_write(&p, text.as_bytes()).map_err(|e| e.to_string())
}

fn clear_view(backend: &str) {
    if let Some(p) = backend_dir(backend) {
        let _ = std::fs::remove_file(p.join("last_view"));
    }
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

#[cfg(test)]
mod keybinding_tests {
    use super::*;
    use mkpclient_state_ui_keybindings::{Action, KeyChord, KeyContext, Keybindings};

    #[test]
    fn save_then_reload_preserves_binding_and_writes_merged_defaults() {
        let dir = tempfile::tempdir().unwrap();
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
    }
}
