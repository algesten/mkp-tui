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
}

impl ConnectLinkInput {
    pub fn new(l: &Link) -> Self {
        Self {
            idle: matches!(l.phase, LinkPhase::Idle),
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
    if !link.idle {
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
    let action = connect_action(desired, ConnectLinkInput::new(&sources.link));
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
