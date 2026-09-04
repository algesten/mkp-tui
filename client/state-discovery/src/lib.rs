//! External-fact source: the live list of discovered servers.
//!
//! The runtime mutates this during its ingest phase by folding
//! `DiscoveryEvent`s from the discovery driver. Memos read a
//! projection of `servers` to answer "what's on the network right
//! now?".
//!
//! Entries are keyed by `name` (the mDNS instance name). Ingest
//! upserts on `Added`/`Refreshed` and removes on `Removed`.

use imbl::Vector;
use mkpclient_driver_discovery_core::ServerAd;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Discovery {
    pub servers: Vector<ServerAd>,
}

impl Discovery {
    /// Apply an `Added` / `Refreshed`: upsert by `name`.
    pub fn upsert(&mut self, ad: ServerAd) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == ad.name) {
            *existing = ad;
        } else {
            self.servers.push_back(ad);
        }
    }

    /// Apply a `Removed`: drop any entry with this name.
    pub fn remove(&mut self, name: &str) {
        self.servers.retain(|s| s.name != name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ad(name: &str, port: u16) -> ServerAd {
        ServerAd {
            name: name.into(),
            host: format!("{name}.local"),
            addr: Ipv4Addr::LOCALHOST,
            port,
        }
    }

    #[test]
    fn upsert_inserts_then_replaces_by_name() {
        let mut d = Discovery::default();
        d.upsert(ad("foo", 1));
        d.upsert(ad("bar", 2));
        d.upsert(ad("foo", 3));
        assert_eq!(d.servers.len(), 2);
        let foo = d.servers.iter().find(|s| s.name == "foo").unwrap();
        assert_eq!(foo.port, 3);
    }

    #[test]
    fn remove_drops_by_name() {
        let mut d = Discovery::default();
        d.upsert(ad("foo", 1));
        d.upsert(ad("bar", 2));
        d.remove("foo");
        assert_eq!(d.servers.len(), 1);
        assert_eq!(d.servers[0].name, "bar");
    }
}
