//! External-fact source: `addr -> fingerprint` cache from TOFU
//! probes. A probe is a short TLS handshake that captures the
//! server's cert, closes, and produces a fingerprint. The runtime
//! fires one before a fresh connect so it can match the right
//! stored credential — the old mDNS host may have had its cert
//! rotated, and multiple paired sessions can share a hostname.

use imbl::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probes {
    pub by_addr: HashMap<String, ProbeOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Probe in flight. Execute phase won't re-fire while this is set.
    InFlight,
    /// Server cert observed; fingerprint is hex-encoded SHA-256.
    Fingerprint(String),
    /// Probe failed; the runtime surfaces the message on the UI.
    /// Held so the probe isn't re-fired in a tight loop.
    Failed(String),
}

impl Probes {
    pub fn mark_in_flight(&mut self, addr: String) {
        self.by_addr.insert(addr, ProbeOutcome::InFlight);
    }

    pub fn set_fingerprint(&mut self, addr: String, fingerprint: String) {
        self.by_addr
            .insert(addr, ProbeOutcome::Fingerprint(fingerprint));
    }

    pub fn set_failed(&mut self, addr: String, message: String) {
        self.by_addr.insert(addr, ProbeOutcome::Failed(message));
    }

    pub fn get(&self, addr: &str) -> Option<&ProbeOutcome> {
        self.by_addr.get(addr)
    }

    /// Drop a cached outcome so the next execute-phase probe re-fires.
    pub fn invalidate(&mut self, addr: &str) {
        self.by_addr.remove(addr);
    }
}
