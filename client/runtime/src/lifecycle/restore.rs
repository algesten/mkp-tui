//! Step 7 of the lifecycle: auto-restore the saved view after
//! connect + playlists load.
//!
//! Spec §6: `desired_restore()` answers "given session + playlists +
//! the most-recent persist load result, what should the middle pane
//! show?"; `restore_action()` diffs against the actual
//! `auto_restored_view` flag plus `loads_in_flight` and returns
//! Resume / Load / Apply(view) / OpenFirst(id) / Noop. The trampoline
//! writes `auto_restored_view = true` synchronously on Resume / Apply
//! / OpenFirst, and inserts the LoadKey into `loads_in_flight`
//! synchronously on Load — both intent writes that flip the next
//! tick's action to Noop.
//!
//! Two sources of "the view to show": when the backend we just
//! connected to is the one the in-memory view was built against
//! (`session.view_backend`), that view is *resumed* — its data is
//! requested again with the cursor kept — which is what makes a
//! reconnect after a drop land the user where they were. Otherwise
//! the saved view for the new backend is loaded from disk.

use std::sync::Arc;

use imbl::{HashSet as ImHashSet, Vector};

use mkpclient_driver_persist_core::{LoadKey, Persist, PersistCmd, SavedView, ViewLoadResult};
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_playlists::Playlists;
use mkpclient_state_ui_session::UiSession;
use mkproto::Playlist;

use crate::dispatch;
use crate::drivers::Drivers;
use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct RestoreSessionInput<'a> {
    pub auto_restored_view: bool,
    pub backend_name: Option<&'a std::sync::Arc<str>>,
    pub view_backend: Option<&'a std::sync::Arc<str>>,
}

impl<'a> RestoreSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            auto_restored_view: s.auto_restored_view,
            backend_name: s.backend_name.as_ref(),
            view_backend: s.view_backend.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct RestoreLinkInput {
    pub connected: bool,
}

impl RestoreLinkInput {
    pub fn new(l: &Link) -> Self {
        Self {
            connected: matches!(l.phase, LinkPhase::Connected),
        }
    }
}

#[derive(drv::Input)]
pub struct RestorePlaylistsInput<'a> {
    pub loaded: bool,
    pub items: &'a Vector<Arc<Playlist>>,
}

impl<'a> RestorePlaylistsInput<'a> {
    pub fn new(p: &'a Playlists) -> Self {
        Self {
            loaded: p.loaded,
            items: &p.items,
        }
    }
}

#[derive(drv::Input)]
pub struct RestorePersistInput<'a> {
    pub last_view_load: Option<&'a ViewLoadResult>,
    pub loads_in_flight: &'a ImHashSet<LoadKey>,
}

impl<'a> RestorePersistInput<'a> {
    pub fn new(p: &'a Persist) -> Self {
        Self {
            last_view_load: p.last_view_load.as_ref(),
            loads_in_flight: &p.loads_in_flight,
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredRestore {
    /// Conditions not met yet (or already restored): do nothing.
    Idle,
    /// Back on the backend the in-memory view belongs to: request
    /// that view's data again, cursor and all. `first_playlist_id`
    /// is the fallback when the view has nothing to resume (no
    /// playlist was open yet).
    Resume {
        backend: String,
        first_playlist_id: Option<String>,
    },
    /// Issue a `LoadView` for `backend`.
    Load { backend: String },
    /// The persist worker came back with a saved view to apply.
    Apply { backend: String, view: SavedView },
    /// No saved view on disk; open the first playlist (or do nothing
    /// if there are no playlists at all).
    OpenFirst {
        backend: String,
        first_playlist_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreAction {
    Noop,
    Resume {
        backend: String,
        first_playlist_id: Option<String>,
    },
    Load {
        backend: String,
    },
    Apply {
        backend: String,
        view: SavedView,
    },
    OpenFirst {
        backend: String,
        first_playlist_id: Option<String>,
    },
}

#[drv::memo(single)]
pub fn desired_restore<'a, 'b, 'c>(
    session: RestoreSessionInput<'a>,
    link: RestoreLinkInput,
    playlists: RestorePlaylistsInput<'b>,
    persist: RestorePersistInput<'c>,
) -> DesiredRestore {
    if session.auto_restored_view {
        return DesiredRestore::Idle;
    }
    if !link.connected || !playlists.loaded {
        return DesiredRestore::Idle;
    }
    let Some(backend) = session.backend_name else {
        return DesiredRestore::Idle;
    };
    // Same server the current view was built against (a reconnect):
    // the view in memory wins over the one on disk.
    if session.view_backend == Some(backend) {
        return DesiredRestore::Resume {
            backend: backend.to_string(),
            first_playlist_id: playlists.items.iter().next().map(|p| p.id.clone()),
        };
    }
    // Has the worker already replied for this backend?
    if let Some(load) = persist.last_view_load {
        if load.backend.as_str() == &**backend {
            return match load.view.clone() {
                Some(view) => DesiredRestore::Apply {
                    backend: backend.to_string(),
                    view,
                },
                None => DesiredRestore::OpenFirst {
                    backend: backend.to_string(),
                    first_playlist_id: playlists.items.iter().next().map(|p| p.id.clone()),
                },
            };
        }
    }
    DesiredRestore::Load {
        backend: backend.to_string(),
    }
}

#[drv::memo(single)]
pub fn restore_action<'a>(
    desired: DesiredRestore,
    persist: RestorePersistInput<'a>,
) -> RestoreAction {
    match desired {
        DesiredRestore::Idle => RestoreAction::Noop,
        DesiredRestore::Resume {
            backend,
            first_playlist_id,
        } => RestoreAction::Resume {
            backend,
            first_playlist_id,
        },
        DesiredRestore::Load { backend } => {
            // Dedup: a previous tick may already have issued the
            // load. The trampoline also re-checks before sending, but
            // returning Noop here keeps the action memo's diff
            // discipline ("same action twice in a row is wasted").
            if persist
                .loads_in_flight
                .contains(&LoadKey::View(backend.clone()))
            {
                RestoreAction::Noop
            } else {
                RestoreAction::Load { backend }
            }
        }
        DesiredRestore::Apply { backend, view } => RestoreAction::Apply { backend, view },
        DesiredRestore::OpenFirst {
            backend,
            first_playlist_id,
        } => RestoreAction::OpenFirst {
            backend,
            first_playlist_id,
        },
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_restore(sources: &mut Sources, drivers: &Drivers) {
    let desired = desired_restore(
        RestoreSessionInput::new(&sources.session),
        RestoreLinkInput::new(&sources.link),
        RestorePlaylistsInput::new(&sources.playlists),
        RestorePersistInput::new(&sources.persist),
    );
    let action = restore_action(desired, RestorePersistInput::new(&sources.persist));
    match action {
        RestoreAction::Noop => {}
        RestoreAction::Resume {
            backend,
            first_playlist_id,
        } => {
            // Sync intent: same guard flip as Apply. The in-memory
            // view is re-issued as if it had just been loaded from
            // disk, so the cursor / hovered song snap back too.
            sources.session.auto_restored_view = true;
            sources.session.view_backend = Some(Arc::from(backend.as_str()));
            sources.persist.last_view_load = None;
            match dispatch::build_saved_view(sources) {
                Some(view) => dispatch::apply_saved_view(sources, view),
                None => {
                    if let Some(id) = first_playlist_id {
                        dispatch::open_first_playlist_pub(sources, id);
                    }
                }
            }
        }
        RestoreAction::Load { backend } => {
            // Sync intent: insert the LoadKey before firing so the
            // dedup gate flips the next tick to Noop.
            sources
                .persist
                .loads_in_flight
                .insert(LoadKey::View(backend.clone()));
            drivers.persist.execute([&PersistCmd::LoadView { backend }]);
        }
        RestoreAction::Apply { backend, view } => {
            // Sync intent: flip the guard before applying so the
            // next tick is Idle. The view-persist lifecycle will
            // re-write the just-applied view to disk on the next
            // tick (one redundant save per restore — accepted in
            // exchange for keeping persistence centralized).
            sources.session.auto_restored_view = true;
            sources.session.view_backend = Some(Arc::from(backend.as_str()));
            sources.persist.last_view_load = None;
            dispatch::apply_saved_view(sources, view);
        }
        RestoreAction::OpenFirst {
            backend,
            first_playlist_id,
        } => {
            sources.session.auto_restored_view = true;
            sources.session.view_backend = Some(Arc::from(backend.as_str()));
            sources.persist.last_view_load = None;
            if let Some(id) = first_playlist_id {
                dispatch::open_first_playlist_pub(sources, id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str) -> Playlist {
        Playlist {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            track_count: 0,
        }
    }

    fn connected_to(name: &str) -> (UiSession, Link, Playlists, Persist) {
        let session = UiSession {
            backend_name: Some(Arc::from(name)),
            auto_restored_view: false,
            ..Default::default()
        };
        let link = Link {
            phase: LinkPhase::Connected,
            ..Default::default()
        };
        let mut playlists = Playlists::default();
        playlists.set_all(vec![playlist("p1"), playlist("p2")]);
        (session, link, playlists, Persist::default())
    }

    fn desired(s: &UiSession, l: &Link, p: &Playlists, d: &Persist) -> DesiredRestore {
        desired_restore(
            RestoreSessionInput::new(s),
            RestoreLinkInput::new(l),
            RestorePlaylistsInput::new(p),
            RestorePersistInput::new(d),
        )
    }

    #[test]
    fn reconnecting_to_the_views_backend_resumes_instead_of_loading() {
        let (mut session, link, playlists, persist) = connected_to("home");
        session.view_backend = Some(Arc::from("home"));
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Resume {
                backend: "home".into(),
                first_playlist_id: Some("p1".into()),
            }
        );
    }

    #[test]
    fn a_different_backend_loads_its_saved_view_from_disk() {
        let (mut session, link, playlists, persist) = connected_to("work");
        session.view_backend = Some(Arc::from("home"));
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Load {
                backend: "work".into()
            }
        );
    }

    #[test]
    fn first_connect_loads_from_disk() {
        let (session, link, playlists, persist) = connected_to("home");
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Load {
                backend: "home".into()
            }
        );
    }

    #[test]
    fn resume_waits_for_link_and_playlists_and_runs_once() {
        let (mut session, mut link, mut playlists, persist) = connected_to("home");
        session.view_backend = Some(Arc::from("home"));

        link.phase = LinkPhase::Connecting;
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Idle
        );

        link.phase = LinkPhase::Connected;
        playlists.loaded = false;
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Idle
        );

        playlists.loaded = true;
        session.auto_restored_view = true;
        assert_eq!(
            desired(&session, &link, &playlists, &persist),
            DesiredRestore::Idle
        );
    }
}
