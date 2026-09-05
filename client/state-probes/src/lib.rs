//! External-fact source: `addr -> fingerprint` cache from TOFU
//! probes. A probe is a short TLS handshake that captures the
//! server's cert, closes, and produces a fingerprint. The runtime
//! fires one before a fresh connect so it can match the right
//! stored credential — the old mDNS host may have had its cert
//! rotated, and multiple paired sessions can share a hostname.

use std::time::{Duration, Instant};

use imbl::HashMap;

/// How long a failed probe is held before the address is eligible
/// for another probe. A server that is advertised but not yet
/// answering (just relaunched, or a stale mDNS entry) fails its
/// probe; without an expiry that failure would pin the address
/// forever and the runtime could never reconnect to it.
pub const FAILED_PROBE_TTL: Duration = Duration::from_secs(5);

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
    /// Probe failed at `at`. Held so the probe isn't re-fired in a
    /// tight loop; the runtime drops it after [`FAILED_PROBE_TTL`]
    /// so the next execute-phase probe re-fires.
    Failed { message: String, at: Instant },
}

impl Probes {
    pub fn mark_in_flight(&mut self, addr: String) {
        self.by_addr.insert(addr, ProbeOutcome::InFlight);
    }

    pub fn set_fingerprint(&mut self, addr: String, fingerprint: String) {
        self.by_addr
            .insert(addr, ProbeOutcome::Fingerprint(fingerprint));
    }

    pub fn set_failed(&mut self, addr: String, message: String, at: Instant) {
        self.by_addr
            .insert(addr, ProbeOutcome::Failed { message, at });
    }

    pub fn get(&self, addr: &str) -> Option<&ProbeOutcome> {
        self.by_addr.get(addr)
    }

    /// Drop a cached outcome so the next execute-phase probe re-fires.
    pub fn invalidate(&mut self, addr: &str) {
        self.by_addr.remove(addr);
    }

    /// Forget failures older than `ttl` so their addresses get probed
    /// again. Called once per tick, before memos look at the source.
    pub fn drop_expired_failures(&mut self, now: Instant, ttl: Duration) {
        self.by_addr.retain(|_, outcome| match outcome {
            ProbeOutcome::Failed { at, .. } => now < *at + ttl,
            _ => true,
        });
    }

    /// The earliest instant at which a held failure expires, for the
    /// runtime's wake-deadline fold. `None` when nothing is held.
    pub fn nearest_retry_at(&self, ttl: Duration) -> Option<Instant> {
        self.by_addr
            .values()
            .filter_map(|outcome| match outcome {
                ProbeOutcome::Failed { at, .. } => Some(*at + ttl),
                _ => None,
            })
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_failures_are_dropped_and_fresh_ones_kept() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(5);
        let mut p = Probes::default();
        p.set_failed("a:1".into(), "refused".into(), t0);
        p.set_failed("b:2".into(), "refused".into(), t0 + Duration::from_secs(3));
        p.set_fingerprint("c:3".into(), "fp".into());

        p.drop_expired_failures(t0 + Duration::from_secs(4), ttl);
        assert!(matches!(p.get("a:1"), Some(ProbeOutcome::Failed { .. })));

        p.drop_expired_failures(t0 + Duration::from_secs(5), ttl);
        assert!(p.get("a:1").is_none());
        assert!(matches!(p.get("b:2"), Some(ProbeOutcome::Failed { .. })));
        assert!(matches!(p.get("c:3"), Some(ProbeOutcome::Fingerprint(_))));
    }

    #[test]
    fn nearest_retry_is_the_earliest_failure_plus_ttl() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(5);
        let mut p = Probes::default();
        assert_eq!(p.nearest_retry_at(ttl), None);
        p.set_fingerprint("c:3".into(), "fp".into());
        assert_eq!(p.nearest_retry_at(ttl), None);
        p.set_failed("b:2".into(), "refused".into(), t0 + Duration::from_secs(3));
        p.set_failed("a:1".into(), "refused".into(), t0);
        assert_eq!(p.nearest_retry_at(ttl), Some(t0 + ttl));
    }
}
