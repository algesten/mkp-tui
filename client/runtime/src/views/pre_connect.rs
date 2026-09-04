//! View model for the pre-connect screen.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The pre-connect
//! screen reads four sources (discovery, link, probes, credentials)
//! through narrow projections.

use imbl::{HashMap as ImHashMap, Vector};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_state_credentials::{Credentials, PairingEntry};
use mkpclient_state_discovery::Discovery;
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_probes::{ProbeOutcome, Probes};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ConnectingKind {
    /// Brand-new install; mDNS hasn't found anything yet.
    Discovering,
    /// We have a target name (preferred / lost) but it's not
    /// visible yet, OR we've already issued the connect.
    ToServer { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PreConnectRow {
    /// `host` plus optional `(paired)` suffix. Renderer adds
    /// leading two-space padding.
    pub label: String,
    pub is_cursor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum PreConnectModel {
    Status(ConnectingKind),
    ServerList { rows: Vector<PreConnectRow> },
}

#[derive(drv::Input)]
pub struct PreConnectInput<'a> {
    pub servers: &'a Vector<ServerAd>,
    pub link_connecting: bool,
    pub probes: &'a ImHashMap<String, ProbeOutcome>,
    pub creds: &'a ImHashMap<String, PairingEntry>,
}

impl<'a> PreConnectInput<'a> {
    pub fn new(
        discovery: &'a Discovery,
        link: &'a Link,
        probes: &'a Probes,
        creds: &'a Credentials,
    ) -> Self {
        Self {
            servers: &discovery.servers,
            link_connecting: link.phase == LinkPhase::Connecting,
            probes: &probes.by_addr,
            creds: &creds.entries,
        }
    }
}

#[drv::memo(single)]
pub fn pre_connect_model<'a>(
    sources: PreConnectInput<'a>,
    preferred_server: Option<&str>,
    lost_server: Option<&str>,
    auto_connect: bool,
    server_picker_selected: usize,
) -> PreConnectModel {
    // Are we still waiting for a named server to appear in mDNS?
    let waiting_for_preferred = match preferred_server {
        Some(name) => auto_connect && !sources.servers.iter().any(|s| s.name == name),
        None => false,
    };

    if sources.servers.is_empty() || waiting_for_preferred || sources.link_connecting {
        let kind = match lost_server.or(preferred_server) {
            Some(name) => ConnectingKind::ToServer {
                name: name.to_string(),
            },
            None => ConnectingKind::Discovering,
        };
        return PreConnectModel::Status(kind);
    }

    let mut rows: Vector<PreConnectRow> = Vector::new();
    for (i, s) in sources.servers.iter().enumerate() {
        let addr = format!("{}:{}", s.addr, s.port);
        let paired = match sources.probes.get(&addr) {
            Some(ProbeOutcome::Fingerprint(fp)) => sources.creds.contains_key(fp),
            _ => false,
        };
        let mark = if paired { " (paired)" } else { "" };
        rows.push_back(PreConnectRow {
            label: format!("{}{}", s.host, mark),
            is_cursor: i == server_picker_selected,
        });
    }
    PreConnectModel::ServerList { rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mkpclient_driver_discovery_core::ServerAd;
    use mkpclient_state_credentials::Credentials;
    use mkpclient_state_discovery::Discovery;
    use mkpclient_state_link::Link;
    use mkpclient_state_probes::Probes;
    use std::net::Ipv4Addr;

    fn ad(name: &str, host: &str) -> ServerAd {
        ServerAd {
            name: name.into(),
            host: host.into(),
            addr: Ipv4Addr::new(127, 0, 0, 1),
            port: 6000,
        }
    }

    /// Test-only bundle so `run` doesn't trip the `too_many_arguments`
    /// lint while still letting individual cases override every knob.
    struct RunArgs<'a> {
        discovery: &'a Discovery,
        link: &'a Link,
        probes: &'a Probes,
        creds: &'a Credentials,
        preferred: Option<&'a str>,
        lost: Option<&'a str>,
        auto: bool,
        sel: usize,
    }

    fn run(args: RunArgs<'_>) -> PreConnectModel {
        pre_connect_model(
            PreConnectInput::new(args.discovery, args.link, args.probes, args.creds),
            args.preferred,
            args.lost,
            args.auto,
            args.sel,
        )
    }

    fn empty() -> (Discovery, Link, Probes, Credentials) {
        (
            Discovery::default(),
            Link::default(),
            Probes::default(),
            Credentials::default(),
        )
    }

    #[test]
    fn empty_discovery_yields_discovering_status() {
        let (d, l, p, c) = empty();
        let m = run(RunArgs {
            discovery: &d,
            link: &l,
            probes: &p,
            creds: &c,
            preferred: None,
            lost: None,
            auto: true,
            sel: 0,
        });
        assert_eq!(m, PreConnectModel::Status(ConnectingKind::Discovering));
    }

    #[test]
    fn preferred_unseen_yields_to_server_status() {
        let (d, l, p, c) = empty();
        let m = run(RunArgs {
            discovery: &d,
            link: &l,
            probes: &p,
            creds: &c,
            preferred: Some("home"),
            lost: None,
            auto: true,
            sel: 0,
        });
        assert_eq!(
            m,
            PreConnectModel::Status(ConnectingKind::ToServer {
                name: "home".into()
            })
        );
    }

    #[test]
    fn preferred_visible_yields_server_list() {
        let mut d = Discovery::default();
        d.upsert(ad("home", "tower"));
        let l = Link::default();
        let p = Probes::default();
        let c = Credentials::default();
        let m = run(RunArgs {
            discovery: &d,
            link: &l,
            probes: &p,
            creds: &c,
            preferred: Some("home"),
            lost: None,
            auto: true,
            sel: 0,
        });
        if let PreConnectModel::ServerList { rows } = m {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].label, "tower");
            assert!(rows[0].is_cursor);
        } else {
            panic!("expected ServerList");
        }
    }

    #[test]
    fn lost_server_takes_priority_over_preferred() {
        let (d, l, p, c) = empty();
        let m = run(RunArgs {
            discovery: &d,
            link: &l,
            probes: &p,
            creds: &c,
            preferred: Some("preferred"),
            lost: Some("lost"),
            auto: true,
            sel: 0,
        });
        assert_eq!(
            m,
            PreConnectModel::Status(ConnectingKind::ToServer {
                name: "lost".into()
            })
        );
    }
}
