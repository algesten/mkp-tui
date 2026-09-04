//! User-decision source (persisted to disk / Keychain): per-server
//! pairing credentials, keyed by SHA-256 fingerprint of the server
//! cert.
//!
//! Writes go through the credentials driver's `execute` path — this
//! source is mutated by the runtime's ingest phase as driver events
//! (Loaded / Saved / Deleted) arrive. Dispatch code doesn't mutate
//! this directly; it pushes a `Save` / `Delete` cmd and waits for
//! the fold-in.

use imbl::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingEntry {
    /// SHA-256 of the server cert DER, hex-encoded. Primary key.
    pub fingerprint: String,
    /// mDNS hostname at pairing time. Informational — may drift as
    /// the server's `.local` name changes, but the fingerprint is
    /// the stable identifier.
    pub host: String,
    pub server_cert_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Credentials {
    pub entries: HashMap<String, PairingEntry>,
    /// False until the initial `Load` completion arrives. Query code
    /// can use this to avoid "no creds" UI flashes before the store
    /// has been read.
    pub loaded: bool,
}

impl Credentials {
    pub fn insert(&mut self, entry: PairingEntry) {
        self.entries.insert(entry.fingerprint.clone(), entry);
    }

    pub fn remove(&mut self, fingerprint: &str) {
        self.entries.remove(fingerprint);
    }

    pub fn get(&self, fingerprint: &str) -> Option<&PairingEntry> {
        self.entries.get(fingerprint)
    }
}
