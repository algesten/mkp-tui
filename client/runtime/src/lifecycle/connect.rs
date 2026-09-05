//! Step 1 of the lifecycle: auto-connect the link to the user's
//! preferred server, the previously-connected server (lost-server
//! reconnect), or the lone discovered server on first run.
//!
//! Spec §6: `desired_connect()` answers "given session + discovery,
//! who should we be trying to dial?"; `connect_action()` diffs against
//! `link.phase` and returns Connect / Noop. `apply_connect()` writes
//! `intent.target` synchronously (which the existing `desired_link`
//! → `link_action` chain then turns into a probe + `ConnectClient`).
//!
//! Grace-timer: the preferred-server path waits 30 s for the named
//! server to appear in mDNS before falling through to lost / single-
//! server paths. The trampoline computes `grace_expired` and passes
//! it as a memo input so the bool flip causes a single recompute.
//! The 8 Hz spinner deadline already wakes the loop frequently enough
//! to notice the flip — no extra entry in `nearest_deadline`.

use std::sync::Arc;
use std::time::Duration;

use imbl::Vector;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_state_discovery::Discovery;
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_ui_session::UiSession;

use crate::sources::Sources;

const GRACE: Duration = Duration::from_secs(30);

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct ConnectSessionInput<'a> {
    pub auto_connect: bool,
    pub preferred_server: Option<&'a std::sync::Arc<str>>,
    pub lost_server: Option<&'a std::sync::Arc<str>>,
    /// Computed by the trampoline: has the 30 s grace for finding
    /// `preferred_server` already lapsed?
    pub grace_expired: bool,
}

impl<'a> ConnectSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            auto_connect: s.auto_connect,
            preferred_server: s.preferred_server.as_ref(),
            lost_server: s.lost_server.as_ref(),
            grace_expired: s.started_at.elapsed() >= GRACE,
        }
    }
}

#[derive(drv::Input)]
pub struct ConnectDiscoveryInput<'a> {
    pub servers: &'a Vector<ServerAd>,
}

impl<'a> ConnectDiscoveryInput<'a> {
    pub fn new(d: &'a Discovery) -> Self {
        Self {
            servers: &d.servers,
        }
    }
}

#[derive(drv::Input)]
pub struct ConnectLinkInput {
    pub idle: bool,
    /// Has the reconnect backoff lapsed? A dropped link is released
    /// back to `Idle` immediately (see `lifecycle::link_ack`), so
    /// idleness alone would redial in the same tick and spin against
    /// an unreachable server. The runtime sleeps until `retry_at`
    /// rather than polling for it.
    pub retry_allowed: bool,
}

impl ConnectLinkInput {
    pub fn new(l: &Link, now: std::time::Instant) -> Self {
        Self {
            idle: matches!(l.phase, LinkPhase::Idle),
            retry_allowed: l.retry_allowed(now),
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredConnect {
    Idle,
    Wait,
    /// Trampoline writes `intent.target = server`. `clear_auto_connect`
    /// = should we flip `session.auto_connect` to false on apply?
    /// Lost-server reconnect leaves it true so a subsequent drop
    /// retries.
    TryConnect {
        server: String,
        clear_auto_connect: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectAction {
    Noop,
    Connect {
        server: String,
        clear_auto_connect: bool,
    },
}

#[drv::memo(single)]
pub fn desired_connect<'a, 'b>(
    session: ConnectSessionInput<'a>,
    discovery: ConnectDiscoveryInput<'b>,
) -> DesiredConnect {
    if !session.auto_connect {
        return DesiredConnect::Idle;
    }

    // (a)+(b) Preferred server, with grace.
    if let Some(name) = session.preferred_server {
        let found = discovery.servers.iter().any(|s| s.name.as_str() == &**name);
        if found {
            return DesiredConnect::TryConnect {
                server: name.to_string(),
                clear_auto_connect: true,
            };
        }
        if !session.grace_expired {
            return DesiredConnect::Wait;
        }
        // Grace expired: fall through to lost / single-server.
    }

    // (d) Lost-server reconnect.
    if let Some(name) = session.lost_server {
        if discovery.servers.iter().any(|s| s.name.as_str() == &**name) {
            return DesiredConnect::TryConnect {
                server: name.to_string(),
                clear_auto_connect: false,
            };
        }
        return DesiredConnect::Wait;
    }

    // (c) Single-server first-run, only if no preferred was set.
    if session.preferred_server.is_none() && discovery.servers.len() == 1 {
        let name = discovery.servers[0].name.clone();
        return DesiredConnect::TryConnect {
            server: name,
            clear_auto_connect: true,
        };
    }

    DesiredConnect::Idle
}

#[drv::memo(single)]
pub fn connect_action(desired: DesiredConnect, link: ConnectLinkInput) -> ConnectAction {
    if !link.idle || !link.retry_allowed {
        return ConnectAction::Noop;
    }
    match desired {
        DesiredConnect::TryConnect {
            server,
            clear_auto_connect,
        } => ConnectAction::Connect {
            server,
            clear_auto_connect,
        },
        DesiredConnect::Idle | DesiredConnect::Wait => ConnectAction::Noop,
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_connect(sources: &mut Sources) {
    let desired = desired_connect(
        ConnectSessionInput::new(&sources.session),
        ConnectDiscoveryInput::new(&sources.discovery),
    );
    let action = connect_action(
        desired,
        ConnectLinkInput::new(&sources.link, sources.clock.now),
    );
    let ConnectAction::Connect {
        server,
        clear_auto_connect,
    } = action
    else {
        return;
    };
    // Sync intent write: feed `desired_link` so the link driver
    // picks this up next tick.
    sources.intent.target = Some(Arc::from(server.as_str()));
    sources.intent.pair_target = None;
    if clear_auto_connect {
        sources.session.auto_connect = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::Ipv4Addr;
    use std::time::Instant;

    use mkpclient_state_link::LinkPhase;

    fn ad(name: &str) -> ServerAd {
        ServerAd {
            name: name.into(),
            host: format!("{name}.local"),
            addr: Ipv4Addr::LOCALHOST,
            port: 6000,
        }
    }

    fn desired(session: &UiSession, discovery: &Discovery) -> DesiredConnect {
        desired_connect(
            ConnectSessionInput::new(session),
            ConnectDiscoveryInput::new(discovery),
        )
    }

    /// The reconnect case: a server we lost, back in mDNS, with
    /// `auto_connect` re-armed by `lifecycle::backend`.
    fn lost_session(name: &str) -> UiSession {
        UiSession {
            auto_connect: true,
            lost_server: Some(std::sync::Arc::from(name)),
            ..Default::default()
        }
    }

    #[test]
    fn a_lost_server_back_in_discovery_is_redialled() {
        let mut d = Discovery::default();
        d.upsert(ad("tower"));

        assert_eq!(
            desired(&lost_session("tower"), &d),
            DesiredConnect::TryConnect {
                server: "tower".into(),
                // Left true so a second drop retries too — clearing it
                // would make reconnect a one-shot.
                clear_auto_connect: false,
            }
        );
    }

    #[test]
    fn a_lost_server_still_missing_waits_rather_than_picking_another() {
        let mut d = Discovery::default();
        d.upsert(ad("someone-else"));
        assert_eq!(desired(&lost_session("tower"), &d), DesiredConnect::Wait);
    }

    #[test]
    fn an_idle_link_past_its_backoff_connects() {
        let t0 = Instant::now();
        let mut d = Discovery::default();
        d.upsert(ad("tower"));
        let want = desired(&lost_session("tower"), &d);

        let mut link = Link {
            phase: LinkPhase::Idle,
            ..Default::default()
        };
        assert!(matches!(
            connect_action(want.clone(), ConnectLinkInput::new(&link, t0)),
            ConnectAction::Connect { .. }
        ));

        // Backoff pending: the answer is "not yet", not "never". The
        // loop sleeps to `retry_at` (folded into `nearest_deadline`)
        // instead of asking again on a timer.
        link.schedule_retry(t0);
        assert_eq!(
            connect_action(want.clone(), ConnectLinkInput::new(&link, t0)),
            ConnectAction::Noop
        );

        let after = link.retry_at.unwrap();
        assert!(matches!(
            connect_action(want, ConnectLinkInput::new(&link, after)),
            ConnectAction::Connect { .. }
        ));
    }

    #[test]
    fn a_link_that_is_not_idle_is_left_alone() {
        let t0 = Instant::now();
        let mut d = Discovery::default();
        d.upsert(ad("tower"));
        let want = desired(&lost_session("tower"), &d);

        for phase in [
            LinkPhase::Connecting,
            LinkPhase::Connected,
            LinkPhase::Closing,
            LinkPhase::Closed,
        ] {
            let link = Link {
                phase: phase.clone(),
                ..Default::default()
            };
            assert_eq!(
                connect_action(want.clone(), ConnectLinkInput::new(&link, t0)),
                ConnectAction::Noop,
                "{phase:?}"
            );
        }
    }
}
