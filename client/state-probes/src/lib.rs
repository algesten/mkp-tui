//! External-fact source: `addr -> fingerprint` cache from TOFU
//! probes. A probe is a short TLS handshake that captures the
//! server's cert, closes, and produces a fingerprint. The runtime
//! fires one before a fresh connect so it can match the right
//! stored credential — the old mDNS host may have had its cert
//! rotated, and multiple paired sessions can share a hostname.

use std::time::{Duration, Instant};

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
    /// Probe failed at `at`. Held so the probe isn't re-fired in a
    /// tight loop; the runtime treats it as absent once its TTL has
    /// passed, and the next probe overwrites it.
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

    /// The earliest instant after `now` at which a held failure
    /// expires, for the runtime's wake-deadline fold. `None` when
    /// nothing is still held.
    pub fn nearest_retry_at(&self, now: Instant, ttl: Duration) -> Option<Instant> {
        self.by_addr
            .values()
            .filter_map(|outcome| match outcome {
                ProbeOutcome::Failed { at, .. } => Some(*at + ttl),
                _ => None,
            })
            .filter(|t| *t > now)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_retry_is_the_earliest_upcoming_failure_expiry() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(5);
        let mut p = Probes::default();
        assert_eq!(p.nearest_retry_at(t0, ttl), None);
        p.set_fingerprint("c:3".into(), "fp".into());
        assert_eq!(p.nearest_retry_at(t0, ttl), None);
        p.set_failed("b:2".into(), "refused".into(), t0 + Duration::from_secs(3));
        p.set_failed("a:1".into(), "refused".into(), t0);
        assert_eq!(p.nearest_retry_at(t0, ttl), Some(t0 + ttl));
        // Once a's expiry has passed it is no longer a deadline to
        // wake for; b's still is.
        assert_eq!(
            p.nearest_retry_at(t0 + ttl, ttl),
            Some(t0 + Duration::from_secs(3) + ttl)
        );
        assert_eq!(p.nearest_retry_at(t0 + Duration::from_secs(60), ttl), None);
    }
}
