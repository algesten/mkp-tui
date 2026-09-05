//! Step 2 of the lifecycle: keep `session.backend_name` in sync with
//! the link's connected/closed state, persist the new "last server"
//! on connect, save the current view + flag lost-server on close.
//!
//! Spec §6: `desired_backend()` answers "what should `backend_name`
//! be given link + probes + discovery?"; `backend_action()` diffs
//! against the actual `session.backend_name` and returns
//! Set / Clear / Noop. The trampoline writes `backend_name`
//! synchronously and folds in the on-connect / on-close side effects
//! (persist saves, breadcrumb clears) — the spec's "execute writes
//! intent + starts async work" pattern.

use std::sync::Arc;

use imbl::{HashMap as ImHashMap, Vector};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_state_discovery::Discovery;
use mkpclient_state_intent::Intent;
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_probes::{ProbeOutcome, Probes};
use mkpclient_state_ui_session::UiSession;

use crate::drivers::Drivers;
use crate::sources::Sources;
use crate::SemanticEvent;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct LinkConnectedInput<'a> {
    pub closed: bool,
    pub connected: bool,
    pub target_fp: Option<&'a std::sync::Arc<str>>,
}

impl<'a> LinkConnectedInput<'a> {
    pub fn new(l: &'a Link) -> Self {
        Self {
            closed: matches!(l.phase, LinkPhase::Closed),
            connected: matches!(l.phase, LinkPhase::Connected),
            target_fp: l.target.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct DiscoveryAdsInput<'a> {
    pub servers: &'a Vector<ServerAd>,
}

impl<'a> DiscoveryAdsInput<'a> {
    pub fn new(d: &'a Discovery) -> Self {
        Self {
            servers: &d.servers,
        }
    }
}

#[derive(drv::Input)]
pub struct ProbesByAddrInput<'a> {
    pub by_addr: &'a ImHashMap<String, ProbeOutcome>,
}

impl<'a> ProbesByAddrInput<'a> {
    pub fn new(p: &'a Probes) -> Self {
        Self {
            by_addr: &p.by_addr,
        }
    }
}

/// Does anything still name a server to be on? `intent` is what
/// `apply_link` dials from, so an empty intent is the runtime's own
/// record that the close was asked for rather than suffered.
#[derive(drv::Input)]
pub struct BackendIntentInput<'a> {
    pub target: Option<&'a std::sync::Arc<str>>,
    pub pair_target: Option<&'a std::sync::Arc<str>>,
}

impl<'a> BackendIntentInput<'a> {
    pub fn new(i: &'a Intent) -> Self {
        Self {
            target: i.target.as_ref(),
            pair_target: i.pair_target.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct BackendNameInput<'a> {
    pub backend_name: Option<&'a std::sync::Arc<str>>,
}

impl<'a> BackendNameInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredBackend {
    /// Link is connected and a probe identified the active server.
    Connected { name: String },
    /// Link is closed; whatever backend_name was set should clear.
    Disconnected,
    /// Connected but no probe data yet, or some other intermediate
    /// state — leave backend_name alone.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendAction {
    Noop,
    /// New backend connected. Trampoline writes `backend_name` and
    /// fires `PersistSaveLastServer` + `PersistRequestLoadLastAddPlaylist`
    /// + clears `lost_server`.
    Set {
        name: String,
    },
    /// Link closed while a backend was active. Trampoline clears
    /// `backend_name` and resets the auto-restored guard.
    Clear {
        old: String,
        /// Was the server lost, rather than left? Only then is `old`
        /// stashed as `lost_server` and `auto_connect` re-armed, which
        /// together are what make the runtime dial it again. An
        /// explicit disconnect that re-armed them would reconnect the
        /// user to the server they just left.
        lost: bool,
    },
}

#[drv::memo(single)]
pub fn desired_backend<'a, 'b, 'c>(
    link: LinkConnectedInput<'a>,
    discovery: DiscoveryAdsInput<'b>,
    probes: ProbesByAddrInput<'c>,
) -> DesiredBackend {
    if link.closed {
        return DesiredBackend::Disconnected;
    }
    if !link.connected {
        return DesiredBackend::Unknown;
    }
    let Some(fp) = link.target_fp else {
        return DesiredBackend::Unknown;
    };
    let name = discovery
        .servers
        .iter()
        .find(|s| {
            let addr = format!("{}:{}", s.addr, s.port);
            matches!(probes.by_addr.get(&addr), Some(ProbeOutcome::Fingerprint(o)) if o.as_str() == &**fp)
        })
        .map(|s| s.name.clone());
    match name {
        Some(name) => DesiredBackend::Connected { name },
        None => DesiredBackend::Unknown,
    }
}

#[drv::memo(single)]
pub fn backend_action<'a, 'b>(
    desired: DesiredBackend,
    current: BackendNameInput<'a>,
    intent: BackendIntentInput<'b>,
) -> BackendAction {
    match (desired, current.backend_name) {
        (DesiredBackend::Connected { name }, None) => BackendAction::Set { name },
        (DesiredBackend::Connected { name }, Some(cur)) if &**cur != name.as_str() => {
            // Different server became connected mid-flight (rare —
            // would mean a transparent re-target). Treat as fresh
            // Set; the previous backend's view-save already happened
            // on its Closed.
            BackendAction::Set { name }
        }
        (DesiredBackend::Disconnected, Some(cur)) => {
            // A loss is a close where we still want *this* server.
            // "Intent is non-empty" is not that test: a deliberate
            // switch sets intent to the *new* server and then tears the
            // old link down, so it would score as a loss and stash the
            // server the user just left — which the reconnect would
            // then dial, undoing the switch.
            let lost = match intent.target {
                Some(t) => **t == **cur,
                None => false,
            };
            BackendAction::Clear {
                old: cur.to_string(),
                lost,
            }
        }
        _ => BackendAction::Noop,
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_backend(sources: &mut Sources, drivers: &Drivers) {
    let desired = desired_backend(
        LinkConnectedInput::new(&sources.link),
        DiscoveryAdsInput::new(&sources.discovery),
        ProbesByAddrInput::new(&sources.probes),
    );
    let action = backend_action(
        desired,
        BackendNameInput::new(&sources.session),
        BackendIntentInput::new(&sources.intent),
    );
    match action {
        BackendAction::Noop => {}
        BackendAction::Set { name } => {
            // Sync intent writes first.
            sources.session.backend_name = Some(Arc::from(name.as_str()));
            sources.session.lost_server = None;
            // Force restore to re-run: the mirrored state was dropped
            // on close, so the view is rebuilt from the saved one on
            // every connect. Drop the previous backend's saved-key too
            // so view-persist doesn't write stale `mode` state to the
            // new backend before restore overwrites it.
            sources.session.auto_restored_view = false;
            sources.persist.last_view_saved_key = None;
            sources.persist.last_add_playlist_saved = None;
            sources.persist.last_pushed_search_task = None;
            // Persist trampolines (fire-and-forget driver commands).
            crate::dispatch::dispatch(
                SemanticEvent::PersistSaveLastServer { name: name.clone() },
                sources,
                drivers,
            );
            crate::dispatch::dispatch(
                SemanticEvent::PersistRequestLoadLastAddPlaylist { backend: name },
                sources,
                drivers,
            );
        }
        BackendAction::Clear { old, lost } => {
            // No save fires here: view-persist has been mirroring
            // every navigation for the outgoing backend on every
            // tick, so the disk is already up to date.
            if lost {
                sources.session.lost_server = Some(Arc::from(old.as_str()));
                sources.session.auto_connect = true;
            }
            sources.session.backend_name = None;
            sources.session.auto_restored_view = false;
            sources.persist.last_view_saved_key = None;
            sources.persist.last_add_playlist_saved = None;
            sources.persist.last_pushed_search_task = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(backend: Option<&str>) -> UiSession {
        UiSession {
            backend_name: backend.map(Arc::from),
            ..Default::default()
        }
    }

    fn act(desired: DesiredBackend, s: &UiSession) -> BackendAction {
        // Default intent: still wanting a server, i.e. a genuine drop.
        let wanting = Intent {
            target: Some(Arc::from("tower")),
            ..Default::default()
        };
        act_with(desired, s, &wanting)
    }

    fn act_with(desired: DesiredBackend, s: &UiSession, i: &Intent) -> BackendAction {
        backend_action(
            desired,
            BackendNameInput::new(s),
            BackendIntentInput::new(i),
        )
    }

    #[test]
    fn a_close_clears_the_active_backend() {
        let s = session(Some("tower"));
        assert_eq!(
            act(DesiredBackend::Disconnected, &s),
            BackendAction::Clear {
                old: "tower".into(),
                lost: true,
            }
        );
        // Nothing was connected — nothing to clear.
        assert_eq!(
            act(DesiredBackend::Disconnected, &session(None)),
            BackendAction::Noop
        );
    }

    /// The difference between a server that went away and one the user
    /// left. Only the first should be stashed as `lost_server` and
    /// re-arm `auto_connect` — doing that for a deliberate disconnect
    /// reconnects the user to the server they just walked away from.
    #[test]
    fn a_close_the_user_asked_for_is_not_a_loss() {
        let s = session(Some("tower"));
        assert_eq!(
            act_with(DesiredBackend::Disconnected, &s, &Intent::default()),
            BackendAction::Clear {
                old: "tower".into(),
                lost: false,
            }
        );
    }
}
