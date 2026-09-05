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

    /// Drop every outcome that is not a usable fingerprint, so the
    /// next execute-phase probe re-fires for those addresses. Called
    /// when the reconnect backoff is armed.
    ///
    /// `Failed` only ever meant "unreachable at that moment", yet
    /// `link_action` treats it as a permanent veto — so the outage
    /// that drops a link would otherwise poison the address the
    /// reconnect needs. `InFlight` is just as stuck: a probe still
    /// outstanding when the network went away has no result coming,
    /// and the execute phase refuses to re-fire while it is set.
    pub fn retry_unresolved(&mut self) {
        self.by_addr
            .retain(|_, v| matches!(v, ProbeOutcome::Fingerprint(_)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_unresolved_keeps_only_usable_fingerprints() {
        let mut p = Probes::default();
        p.set_fingerprint("good:1".into(), "abc".into());
        p.mark_in_flight("busy:2".into());
        p.set_failed("bad:3".into(), "connection refused".into());

        p.retry_unresolved();

        // A failure is a moment-in-time fact about reachability. The
        // outage that drops a link also fails the probe for the very
        // address the reconnect needs, so holding on to it would veto
        // the retry for the rest of the process.
        assert_eq!(p.get("bad:3"), None);
        assert_eq!(
            p.get("good:1"),
            Some(&ProbeOutcome::Fingerprint("abc".into()))
        );
        // An in-flight probe whose worker died with the network never
        // resolves, and blocks a re-probe for as long as it is set.
        assert_eq!(p.get("busy:2"), None);
    }
}
