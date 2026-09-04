//! Driver bundle: every sync handle the runtime calls into, plus a
//! type-erased slot for the native-worker lifecycle markers held for
//! the life of the runtime (drop order: sync handles first — they
//! close cmd channels — natives after).
//!
//! The bundle is platform-neutral. A per-platform "runtime-*" crate
//! (e.g. `mkpclient-runtime-desktop`, `mkpclient-runtime-ios`) spawns
//! the right natives and assembles the bundle via
//! [`Drivers::from_handles`].
//!
//! The unified `RuntimeTrace` is split into per-driver adapter
//! structs so each driver's narrower `Trace` trait sees only its
//! own events.

use std::any::Any;
use std::sync::Arc;

use mkpclient_driver_clipboard_core::{self as clipboard, ClipboardDriver};
use mkpclient_driver_credentials_core::{self as cred, CredDriver};
use mkpclient_driver_discovery_core::{self as disc, DiscoveryDriver, ServerAd};
use mkpclient_driver_link_core::{self as link, LinkDriver, LinkKind};
use mkpclient_driver_persist_core::{self as persist, LoadKey, PersistDriver};
use mkpclient_state_credentials::PairingEntry;

/// Cross-driver trace sink. Caller supplies one impl; the adapters
/// below split it into per-driver impls so each driver keeps a
/// narrower trait.
pub trait Trace: Send + Sync {
    // Discovery
    fn discovery_added(&self, _ad: &ServerAd) {}
    fn discovery_refreshed(&self, _ad: &ServerAd) {}
    fn discovery_removed(&self, _name: &str) {}
    // Credentials
    fn cred_load_start(&self) {}
    fn cred_load_done(&self, _n: usize) {}
    fn cred_save(&self, _fp: &str) {}
    fn cred_delete(&self, _fp: &str) {}
    fn cred_error(&self, _op: &'static str, _msg: &str) {}
    // Link
    fn link_connect(&self, _addr: &str, _kind: LinkKind) {}
    fn link_connected(&self, _kind: LinkKind) {}
    fn link_send(&self, _seq: u64) {}
    fn link_recv(&self, _seq: u64) {}
    fn link_closed(&self, _err: Option<&str>) {}
    fn pairing_ready(&self, _fp: &str, _code: &str) {}
    fn pair_failed(&self, _msg: &str) {}
    fn probe(&self, _addr: &str) {}
    fn probe_result(&self, _addr: &str, _result: &Result<String, String>) {}
    // Persist
    fn persist_load(&self, _key: &LoadKey) {}
    fn persist_loaded(&self, _key: &LoadKey) {}
    fn persist_save(&self, _op: &'static str) {}
    fn persist_error(&self, _op: &'static str, _err: &str) {}
    // Clipboard
    fn clipboard_write(&self, _seq: u64) {}
    fn clipboard_outcome(&self, _seq: u64, _ok: bool) {}
}

pub type RuntimeTrace = Arc<dyn Trace>;

/// Type-erased keep-alive slot for a spawned native marker. The
/// runtime drops these after the sync handles, so workers see a
/// channel hangup and exit cleanly.
pub type NativeMarker = Box<dyn Any + Send>;

pub struct Drivers {
    pub discovery: DiscoveryDriver,
    pub credentials: CredDriver,
    pub link: LinkDriver,
    pub persist: PersistDriver,
    pub clipboard: ClipboardDriver,

    /// Native markers (one per driver). Drop order is *after* the
    /// sync handles above because struct fields drop top-to-bottom;
    /// keeping these last preserves the "close cmd channels first,
    /// then let workers exit" invariant.
    _natives: Vec<NativeMarker>,
}

impl Drivers {
    /// Assemble a `Drivers` bundle from already-spawned sync handles.
    /// Per-platform `runtime-*` crates call this after spawning the
    /// natives appropriate for their target.
    pub fn from_handles(
        discovery: DiscoveryDriver,
        credentials: CredDriver,
        link: LinkDriver,
        persist: PersistDriver,
        clipboard: ClipboardDriver,
        natives: Vec<NativeMarker>,
    ) -> Self {
        Self {
            discovery,
            credentials,
            link,
            persist,
            clipboard,
            _natives: natives,
        }
    }
}

// ─── per-driver trace adapters ──────────────────────────────────────

/// Wrap a `RuntimeTrace` so it can be passed as a `disc::Trace` to
/// `mkpclient-driver-discovery-*::spawn`.
pub fn discovery_trace(trace: RuntimeTrace) -> Arc<dyn disc::Trace> {
    Arc::new(DiscAdapter(trace))
}

/// Wrap a `RuntimeTrace` for the credentials driver.
pub fn credentials_trace(trace: RuntimeTrace) -> Arc<dyn cred::Trace> {
    Arc::new(CredAdapter(trace))
}

/// Wrap a `RuntimeTrace` for the link driver.
pub fn link_trace(trace: RuntimeTrace) -> Arc<dyn link::Trace> {
    Arc::new(LinkAdapter(trace))
}

/// Wrap a `RuntimeTrace` for the persist driver.
pub fn persist_trace(trace: RuntimeTrace) -> Arc<dyn persist::Trace> {
    Arc::new(PersistAdapter(trace))
}

/// Wrap a `RuntimeTrace` for the clipboard driver.
pub fn clipboard_trace(trace: RuntimeTrace) -> Arc<dyn clipboard::Trace> {
    Arc::new(ClipboardAdapter(trace))
}

struct DiscAdapter(RuntimeTrace);
impl disc::Trace for DiscAdapter {
    fn discovery_added(&self, ad: &ServerAd) {
        self.0.discovery_added(ad)
    }
    fn discovery_refreshed(&self, ad: &ServerAd) {
        self.0.discovery_refreshed(ad)
    }
    fn discovery_removed(&self, name: &str) {
        self.0.discovery_removed(name)
    }
}

struct CredAdapter(RuntimeTrace);
impl cred::Trace for CredAdapter {
    fn cred_load_start(&self) {
        self.0.cred_load_start()
    }
    fn cred_load_done(&self, n: usize) {
        self.0.cred_load_done(n)
    }
    fn cred_save(&self, fp: &str) {
        self.0.cred_save(fp)
    }
    fn cred_delete(&self, fp: &str) {
        self.0.cred_delete(fp)
    }
    fn cred_error(&self, op: &'static str, msg: &str) {
        self.0.cred_error(op, msg)
    }
}

struct LinkAdapter(RuntimeTrace);
impl link::Trace for LinkAdapter {
    fn link_connect(&self, addr: &str, kind: LinkKind) {
        self.0.link_connect(addr, kind)
    }
    fn link_connected(&self, kind: LinkKind) {
        self.0.link_connected(kind)
    }
    fn link_send(&self, seq: u64) {
        self.0.link_send(seq)
    }
    fn link_recv(&self, seq: u64) {
        self.0.link_recv(seq)
    }
    fn link_closed(&self, err: Option<&str>) {
        self.0.link_closed(err)
    }
    fn pairing_ready(&self, fp: &str, code: &str) {
        self.0.pairing_ready(fp, code)
    }
    fn pair_failed(&self, msg: &str) {
        self.0.pair_failed(msg)
    }
    fn probe(&self, addr: &str) {
        self.0.probe(addr)
    }
    fn probe_result(&self, addr: &str, result: &Result<String, String>) {
        self.0.probe_result(addr, result)
    }
}

struct ClipboardAdapter(RuntimeTrace);
impl clipboard::Trace for ClipboardAdapter {
    fn clipboard_write(&self, seq: u64) {
        self.0.clipboard_write(seq)
    }
    fn clipboard_outcome(&self, seq: u64, ok: bool) {
        self.0.clipboard_outcome(seq, ok)
    }
}

struct PersistAdapter(RuntimeTrace);
impl persist::Trace for PersistAdapter {
    fn persist_load(&self, key: &LoadKey) {
        self.0.persist_load(key)
    }
    fn persist_loaded(&self, key: &LoadKey) {
        self.0.persist_loaded(key)
    }
    fn persist_save(&self, op: &'static str) {
        self.0.persist_save(op)
    }
    fn persist_error(&self, op: &'static str, err: &str) {
        self.0.persist_error(op, err)
    }
}

/// Silence unused warning when downstream consumers only import
/// `PairingEntry` through the state crate rather than re-exported
/// here.
#[allow(dead_code)]
fn _force_pairing_entry(_: &PairingEntry) {}
