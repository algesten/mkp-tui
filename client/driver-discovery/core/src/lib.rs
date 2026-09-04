//! Sync core of the mDNS discovery driver.
//!
//! The native worker browses for `_mkplay._tcp.local` services on
//! every local interface and posts `DiscoveryEvent`s back via mpsc.
//! The runtime drains them in its ingest phase and folds them into
//! the `Discovery` source.
//!
//! No commands are wired yet — discovery is always-on for now. A
//! future `DiscoveryCmd` (e.g. `Refresh`, `SetConnected(bool)` to
//! throttle query cadence) can be bolted on without changing the
//! event shape.

use std::net::Ipv4Addr;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// A single discovered server. Stable ABI between the worker and
/// whatever consumes the events.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerAd {
    pub name: String,
    pub host: String,
    pub addr: Ipv4Addr,
    pub port: u16,
}

/// Worker → runtime events. The runtime folds these into the
/// `Discovery` source in its ingest phase.
#[derive(Clone, Debug)]
pub enum DiscoveryEvent {
    /// A server was seen for the first time (or after a TTL drop).
    Added(ServerAd),
    /// An existing server was re-seen; TXT / addr / port may have
    /// changed. Consumers upsert keyed by `name`.
    Refreshed(ServerAd),
    /// The TTL window on this server expired without a re-sighting.
    Removed { name: String },
}

/// `--golden-trace` hook for discovery events. Mirrors the per-driver
/// trace pattern used elsewhere: narrow, driver-specific, unified at
/// the runtime.
pub trait Trace: Send + Sync {
    fn discovery_added(&self, ad: &ServerAd);
    fn discovery_refreshed(&self, ad: &ServerAd);
    fn discovery_removed(&self, name: &str);
}

pub struct NoopTrace;
impl Trace for NoopTrace {
    fn discovery_added(&self, _: &ServerAd) {}
    fn discovery_refreshed(&self, _: &ServerAd) {}
    fn discovery_removed(&self, _: &str) {}
}

/// Sync handle the runtime holds. Drain events with `process()` each
/// tick.
pub struct DiscoveryDriver {
    event_rx: Receiver<DiscoveryEvent>,
    trace: Arc<dyn Trace>,
}

impl DiscoveryDriver {
    pub fn new(event_rx: Receiver<DiscoveryEvent>, trace: Arc<dyn Trace>) -> Self {
        Self { event_rx, trace }
    }

    /// Drain every event currently queued from the native worker.
    /// Non-blocking; returns an empty `Vec` when idle.
    pub fn process(&self) -> Vec<DiscoveryEvent> {
        let mut out: Vec<DiscoveryEvent> = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                DiscoveryEvent::Added(ad) => self.trace.discovery_added(ad),
                DiscoveryEvent::Refreshed(ad) => self.trace.discovery_refreshed(ad),
                DiscoveryEvent::Removed { name } => self.trace.discovery_removed(name),
            }
            out.push(ev);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn process_returns_empty_when_nothing_queued() {
        let (_tx, rx) = mpsc::channel::<DiscoveryEvent>();
        let drv = DiscoveryDriver::new(rx, Arc::new(NoopTrace));
        assert!(drv.process().is_empty());
    }

    #[test]
    fn process_drains_events() {
        let (tx, rx) = mpsc::channel::<DiscoveryEvent>();
        let drv = DiscoveryDriver::new(rx, Arc::new(NoopTrace));
        let ad = ServerAd {
            name: "foo".into(),
            host: "foo.local".into(),
            addr: Ipv4Addr::LOCALHOST,
            port: 4242,
        };
        tx.send(DiscoveryEvent::Added(ad.clone())).unwrap();
        tx.send(DiscoveryEvent::Removed { name: "foo".into() })
            .unwrap();
        let batch = drv.process();
        assert_eq!(batch.len(), 2);
    }
}
