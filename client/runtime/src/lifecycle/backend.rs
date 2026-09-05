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

#[derive(drv::Input)]
pub struct BackendNameInput<'a> {
    pub backend_name: Option<&'a std::sync::Arc<str>>,
    /// The server the retained view belongs to while the link is
    /// down. A drop no longer empties the screen, so on the way back
    /// up the runtime has to decide whether that data still describes
    /// the server it just reached.
    pub lost_server: Option<&'a std::sync::Arc<str>>,
}

impl<'a> BackendNameInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
            lost_server: s.lost_server.as_ref(),
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
        /// Discard the view retained across the drop. True when the
        /// link came back on a *different* server than the one the
        /// retained playlists / queue / tracks describe — that data
        /// belongs to a server we are no longer talking to. False for
        /// a genuine reconnect, where the same server's data is about
        /// to be refreshed in place and dropping it would blank the
        /// screen for the duration of the refetch.
        drop_retained: bool,
    },
    /// Link closed while a backend was active. Trampoline saves the
    /// current view, stashes the old backend as `lost_server`, clears
    /// `backend_name`, and resets the auto-connect / auto-restored
    /// guards.
    Clear {
        old: String,
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
pub fn backend_action<'a>(desired: DesiredBackend, current: BackendNameInput<'a>) -> BackendAction {
    match (desired, current.backend_name) {
        (DesiredBackend::Connected { name }, None) => {
            // Coming up from a drop: `lost_server` names whoever the
            // retained view describes. Same server → a reconnect, keep
            // it. Anyone else → the data is foreign, drop it.
            let drop_retained = match current.lost_server {
                Some(lost) => &**lost != name.as_str(),
                None => false,
            };
            BackendAction::Set {
                name,
                drop_retained,
            }
        }
        (DesiredBackend::Connected { name }, Some(cur)) if &**cur != name.as_str() => {
            // Different server became connected mid-flight (rare —
            // would mean a transparent re-target). Treat as fresh
            // Set; the previous backend's view-save already happened
            // on its Closed.
            BackendAction::Set {
                name,
                drop_retained: true,
            }
        }
        (DesiredBackend::Disconnected, Some(cur)) => BackendAction::Clear {
            old: cur.to_string(),
        },
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
    let action = backend_action(desired, BackendNameInput::new(&sources.session));
    match action {
        BackendAction::Noop => {}
        BackendAction::Set {
            name,
            drop_retained,
        } => {
            if drop_retained {
                // Foreign data from a server we are no longer on —
                // including `server.backend`, which names the music
                // backend the retained rows came from.
                sources.server = Default::default();
                sources.queue = Default::default();
                sources.playlists = Default::default();
                sources.playlist_tracks.clear();
                sources.search.clear();
                sources.artist_extras.clear();
            }
            // Sync intent writes first.
            sources.session.backend_name = Some(Arc::from(name.as_str()));
            sources.session.lost_server = None;
            // Force restore to re-run for the (possibly different)
            // backend, and drop the previous backend's saved-key so
            // view-persist doesn't accidentally write stale `mode`
            // state to the new backend before restore overwrites it.
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
        BackendAction::Clear { old } => {
            // No save fires here: view-persist has been mirroring
            // every navigation for the outgoing backend on every
            // tick, so the disk is already up to date.
            sources.session.lost_server = Some(Arc::from(old.as_str()));
            sources.session.backend_name = None;
            sources.session.auto_connect = true;
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

    fn session(backend: Option<&str>, lost: Option<&str>) -> UiSession {
        UiSession {
            backend_name: backend.map(Arc::from),
            lost_server: lost.map(Arc::from),
            ..Default::default()
        }
    }

    fn act(desired: DesiredBackend, s: &UiSession) -> BackendAction {
        backend_action(desired, BackendNameInput::new(s))
    }

    #[test]
    fn reconnecting_to_the_same_server_keeps_the_retained_view() {
        // The drop left `lost_server = tower` and the view painted.
        // Coming back up on tower, that data is about to be refreshed
        // in place — dropping it would blank the screen for the length
        // of the refetch, which is the flicker this avoids.
        let s = session(None, Some("tower"));
        assert_eq!(
            act(
                DesiredBackend::Connected {
                    name: "tower".into()
                },
                &s
            ),
            BackendAction::Set {
                name: "tower".into(),
                drop_retained: false,
            }
        );
    }

    #[test]
    fn landing_on_a_different_server_discards_the_retained_view() {
        let s = session(None, Some("tower"));
        assert_eq!(
            act(
                DesiredBackend::Connected {
                    name: "laptop".into()
                },
                &s
            ),
            BackendAction::Set {
                name: "laptop".into(),
                drop_retained: true,
            }
        );
    }

    #[test]
    fn a_first_connect_has_nothing_to_discard() {
        let s = session(None, None);
        assert_eq!(
            act(
                DesiredBackend::Connected {
                    name: "tower".into()
                },
                &s
            ),
            BackendAction::Set {
                name: "tower".into(),
                drop_retained: false,
            }
        );
    }

    #[test]
    fn a_close_clears_the_active_backend() {
        let s = session(Some("tower"), None);
        assert_eq!(
            act(DesiredBackend::Disconnected, &s),
            BackendAction::Clear {
                old: "tower".into()
            }
        );
        // Nothing was connected — nothing to clear.
        assert_eq!(
            act(DesiredBackend::Disconnected, &session(None, None)),
            BackendAction::Noop
        );
    }
}
