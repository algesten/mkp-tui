//! Filesystem-backed credentials native.
//!
//! Storage layout (same shape the legacy tokio client used, so
//! existing pairings survive the cutover):
//!
//! ```text
//! ~/.config/mkp/pairing/{fingerprint}/
//!   server_cert.pem
//!   client_cert.pem
//!   client_key.pem
//!   host.txt           (informational; the fingerprint dir name is
//!                       the stable key)
//! ```

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use log::{debug, warn};

use mkpclient_core::Notifier;
use mkpclient_driver_credentials_core::{CredCmd, CredDriver, CredEvent, Trace};
use mkpclient_state_credentials::PairingEntry;

pub struct CredNative {
    _marker: (),
}

pub fn spawn(trace: Arc<dyn Trace>, notify: Notifier) -> (CredDriver, CredNative) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<CredCmd>();
    let (event_tx, event_rx) = mpsc::channel::<CredEvent>();

    thread::Builder::new()
        .name("mkp-credentials".into())
        .spawn(move || worker_loop(cmd_rx, event_tx, notify))
        .expect("spawning credentials worker should succeed");

    let driver = CredDriver::new(cmd_tx, event_rx, trace);
    (driver, CredNative { _marker: () })
}

fn worker_loop(rx: Receiver<CredCmd>, tx: Sender<CredEvent>, notify: Notifier) {
    while let Ok(cmd) = rx.recv() {
        let event = match cmd {
            CredCmd::Load => match load_all() {
                Ok(entries) => CredEvent::Loaded(entries),
                Err(e) => CredEvent::Error {
                    op: "load",
                    message: e,
                },
            },
            CredCmd::Save(entry) => match save_one(&entry) {
                Ok(()) => CredEvent::Saved(entry),
                Err(e) => CredEvent::Error {
                    op: "save",
                    message: e,
                },
            },
            CredCmd::Delete { fingerprint } => {
                delete_one(&fingerprint);
                CredEvent::Deleted { fingerprint }
            }
        };

        if tx.send(event).is_err() {
            return;
        }
        notify.notify();
    }
}

fn config_dir() -> Option<PathBuf> {
    if let Some(config) = std::env::var_os("MKP_CONFIG_HOME") {
        return Some(PathBuf::from(config));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("mkp"));
    }
    // Matches the legacy TUI's expectation: `~/.config/mkp/` on
    // Linux/macOS. We deliberately don't use the `dirs` crate so
    // the fs native stays dep-light.
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("mkp"))
}

fn pairing_root() -> Option<PathBuf> {
    config_dir().map(|d| d.join("pairing"))
}

fn pairing_dir(fingerprint: &str) -> Option<PathBuf> {
    pairing_root().map(|r| r.join(fingerprint))
}

fn load_all() -> Result<Vec<PairingEntry>, String> {
    let root = match pairing_root() {
        Some(r) => r,
        None => return Ok(Vec::new()),
    };
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let iter = fs::read_dir(&root).map_err(|e| format!("read_dir {}: {}", root.display(), e))?;
    for entry in iter.flatten() {
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let fingerprint = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        match load_one(&fingerprint, &dir) {
            Some(e) => out.push(e),
            None => warn!(
                "credentials: skipping incomplete pairing dir at {}",
                dir.display()
            ),
        }
    }
    Ok(out)
}

fn load_one(fingerprint: &str, dir: &std::path::Path) -> Option<PairingEntry> {
    let server_cert_pem = fs::read_to_string(dir.join("server_cert.pem")).ok()?;
    let client_cert_pem = fs::read_to_string(dir.join("client_cert.pem")).ok()?;
    let client_key_pem = fs::read_to_string(dir.join("client_key.pem")).ok()?;
    let host = fs::read_to_string(dir.join("host.txt"))
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(PairingEntry {
        fingerprint: fingerprint.to_string(),
        host,
        server_cert_pem,
        client_cert_pem,
        client_key_pem,
    })
}

fn save_one(entry: &PairingEntry) -> Result<(), String> {
    let dir = pairing_dir(&entry.fingerprint).ok_or("cannot determine config dir")?;

    // 0o700 on Unix; on other targets this is a no-op.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("create {}: {}", dir.display(), e))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    }

    let host = entry.host.strip_suffix(".local").unwrap_or(&entry.host);

    fs::write(dir.join("server_cert.pem"), &entry.server_cert_pem)
        .map_err(|e| format!("write server_cert.pem: {}", e))?;
    fs::write(dir.join("client_cert.pem"), &entry.client_cert_pem)
        .map_err(|e| format!("write client_cert.pem: {}", e))?;
    fs::write(dir.join("client_key.pem"), &entry.client_key_pem)
        .map_err(|e| format!("write client_key.pem: {}", e))?;
    fs::write(dir.join("host.txt"), host).map_err(|e| format!("write host.txt: {}", e))?;

    debug!(
        "credentials: saved pairing for {} (fingerprint {})",
        host, entry.fingerprint
    );
    Ok(())
}

fn delete_one(fingerprint: &str) {
    if let Some(dir) = pairing_dir(fingerprint) {
        let _ = fs::remove_dir_all(&dir);
    }
    debug!("credentials: deleted fingerprint {}", fingerprint);
}
