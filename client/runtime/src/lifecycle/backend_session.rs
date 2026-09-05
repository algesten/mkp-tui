//! Restart the session when the server's backend and the backend our
//! sources were built from disagree.
//!
//! Spec §5 / §6. The server owns the backend: it reports one on
//! connect (as the `Hello` reply) and can swap it under a live link
//! (as a seq-0 broadcast). Ingest folds either frame into
//! `server.backend` as the plain fact it is. The decision of what to
//! do about it lives here, as a diff between that fact and
//! `server.built_from` — the backend the rest of the sources
//! actually hold.
//!
//! Stating it as a diff rather than as a handler on the frame is
//! what makes connect, swap and reconnect one rule instead of three,
//! and what makes a missed or duplicated frame a non-event: the
//! answer depends on the current state, not on having observed the
//! transition.

use mkpclient_state_server_state::ServerState;
use mkproto::ClientMsg;

use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct BackendSessionInput<'a> {
    pub backend: Option<&'a std::sync::Arc<str>>,
    pub built_from: Option<&'a std::sync::Arc<str>>,
}

impl<'a> BackendSessionInput<'a> {
    pub fn new(s: &'a ServerState) -> Self {
        Self {
            backend: s.backend.as_ref(),
            built_from: s.built_from.as_ref(),
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendSessionAction {
    Noop,
    /// The sources do not hold this backend's catalogue. Drop what
    /// the previous one produced and ask for this one's world.
    Restart {
        backend: String,
    },
}

#[drv::memo(single)]
pub fn backend_session_action<'a>(input: BackendSessionInput<'a>) -> BackendSessionAction {
    let Some(backend) = input.backend else {
        // Nothing connected, or the handshake hasn't landed.
        return BackendSessionAction::Noop;
    };
    if input.built_from == Some(backend) {
        return BackendSessionAction::Noop;
    }
    BackendSessionAction::Restart {
        backend: backend.to_string(),
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_backend_session(sources: &mut Sources) {
    let action = backend_session_action(BackendSessionInput::new(&sources.server));
    let BackendSessionAction::Restart { backend } = action else {
        return;
    };

    // A swap is a reconnect that skipped the socket, so it drops
    // what a close drops — including the optimistic playlist
    // mutations whose confirmations are never coming.
    crate::ingest::reset_server_derived_state(sources);

    // Queued requests describe the outgoing catalogue, so they go
    // too — except an unshipped `Hello`. A swap broadcast can reach
    // ingest in the same drain as `LinkEvent::Connected`, and
    // dropping the handshake would leave the server without our
    // identity and skip the protocol-version check.
    sources
        .requests
        .pending
        .retain(|p| matches!(p.msg, ClientMsg::Hello { .. }));

    // Sync intent write: record what we are now building from, so
    // this action returns Noop next tick. `backend` itself is
    // ingest's to write and is left exactly as it folded it.
    sources.server.built_from = Some(std::sync::Arc::from(backend.as_str()));

    // Navigation goes on every restart, reconnects included. `mode`
    // and the back / forward stacks hold album and artist ids only
    // one catalogue can resolve, and a restart cannot tell whether
    // it is the same one: `built_from` describes what the sources
    // hold, and the close that preceded a reconnect emptied them. So
    // a server that swapped backend while the link was down is
    // indistinguishable here from one that did not, and keeping the
    // stacks would leave Back drilling into ids the server can no
    // longer resolve. `restore` re-establishes what the user sees;
    // only the stacks behind it are lost.
    sources.history = Default::default();

    // Let the restore lifecycle re-run for the new backend once its
    // playlists land. Dropping the save-dedup key with it means the
    // new backend's first view is written even in the unlikely event
    // that it has the same identity as the outgoing backend's last,
    // and dropping any stashed load keeps a result captured for the
    // previous session from being applied to this one.
    sources.session.auto_restored_view = false;
    sources.persist.last_view_saved_key = None;
    sources.persist.last_view_load = None;

    sources.requests.push(ClientMsg::GetState, None);
    let task_id = sources.requests.alloc_task_id();
    let seq = sources
        .requests
        .push(ClientMsg::GetPlaylists, Some(task_id));
    // Track this seq so an `Error` reply still flips
    // `playlists.loaded = true` (with empty items) and is consumed
    // before the `ErrorModal` lifecycle sees it.
    sources.playlists.pending_request = Some(seq);
    sources.playlists.pending_task = Some(task_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(backend: Option<&str>, built_from: Option<&str>) -> ServerState {
        ServerState {
            backend: backend.map(std::sync::Arc::from),
            built_from: built_from.map(std::sync::Arc::from),
            ..Default::default()
        }
    }

    #[test]
    fn noop_before_the_handshake_reports_a_backend() {
        let s = state(None, None);
        assert_eq!(
            backend_session_action(BackendSessionInput::new(&s)),
            BackendSessionAction::Noop
        );
    }

    #[test]
    fn restarts_when_nothing_is_built_yet() {
        let s = state(Some("musickit"), None);
        assert_eq!(
            backend_session_action(BackendSessionInput::new(&s)),
            BackendSessionAction::Restart {
                backend: "musickit".into()
            }
        );
    }

    #[test]
    fn restarts_when_the_server_swaps_under_a_live_link() {
        let s = state(Some("tidal"), Some("musickit"));
        assert_eq!(
            backend_session_action(BackendSessionInput::new(&s)),
            BackendSessionAction::Restart {
                backend: "tidal".into()
            }
        );
    }

    #[test]
    fn noop_once_the_sources_hold_that_backend() {
        let s = state(Some("tidal"), Some("tidal"));
        assert_eq!(
            backend_session_action(BackendSessionInput::new(&s)),
            BackendSessionAction::Noop
        );
    }

    /// The restart discards the queued requests, which describe the
    /// outgoing catalogue — but never an unshipped `Hello`. The
    /// server registers its broadcast receivers when it accepts, so
    /// a swap announced in that window reaches ingest in the same
    /// drain as `LinkEvent::Connected`, and dropping the handshake
    /// would leave the server without our identity and skip the
    /// protocol-version check.
    ///
    /// Nothing else in the suite covers this: replacing the retain
    /// with `requests.clear()` leaves every other test green.
    #[test]
    fn a_restart_keeps_an_unshipped_hello_and_drops_the_rest() {
        let mut sources = Sources::default();
        sources.server.backend = Some(std::sync::Arc::from("musickit"));
        sources.requests.push(
            ClientMsg::Hello {
                peer: mkproto::Peer {
                    user: "u".into(),
                    host: "h".into(),
                },
                version: mkproto::PROTOCOL_VERSION,
            },
            None,
        );
        sources.requests.push(
            ClientMsg::GetAlbumDetail {
                id: "album-on-the-outgoing-backend".into(),
            },
            None,
        );

        apply_backend_session(&mut sources);

        let queued: Vec<ClientMsg> = sources
            .requests
            .pending
            .iter()
            .map(|p| p.msg.clone())
            .collect();
        assert!(
            matches!(queued.first(), Some(ClientMsg::Hello { .. })),
            "the handshake must survive, and stay in front: {queued:?}"
        );
        assert!(
            !queued
                .iter()
                .any(|m| matches!(m, ClientMsg::GetAlbumDetail { .. })),
            "requests aimed at the outgoing catalogue must go: {queued:?}"
        );
        assert!(
            queued.iter().any(|m| matches!(m, ClientMsg::GetPlaylists)),
            "the restart must ask for the new backend's world: {queued:?}"
        );
        assert_eq!(
            sources.server.built_from.as_deref(),
            Some("musickit"),
            "the sync intent write is what makes the next tick a Noop"
        );
    }

    /// A redundant `BackendChanged` naming the backend we already
    /// hold is not a transition, so it does nothing — the diff cares
    /// about state, not about frames observed.
    #[test]
    fn a_repeated_announcement_is_not_a_restart() {
        let s = state(Some("musickit"), Some("musickit"));
        assert_eq!(
            backend_session_action(BackendSessionInput::new(&s)),
            BackendSessionAction::Noop
        );
    }
}
