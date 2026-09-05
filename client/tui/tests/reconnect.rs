//! End-to-end: a dropped link redials itself, and the view survives
//! the round trip.
//!
//! Regression cover for a link that parked on `LinkPhase::Closed`
//! forever. Nothing here dispatches a user event after the drop — if
//! the runtime doesn't redial on its own, the test times out.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_runtime::ClientMsg;
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_screen::Screen;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

fn playlist(id: &str) -> Playlist {
    Playlist {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        track_count: 3,
    }
}

/// Script that serves a normal session, but drops the connection the
/// second time it is asked for state. The first `GetState` is part of
/// the harness's connect handshake; the second is the test asking for
/// the drop.
fn dropping_script() -> common::mock_server::Script {
    let state_calls = AtomicUsize::new(0);
    Box::new(move |msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
        ClientMsg::GetState => {
            if state_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                vec![ScriptStep::Disconnect]
            } else {
                vec![ScriptStep::Reply(ServerMsg::Ok)]
            }
        }
        ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
            playlists: vec![playlist("one"), playlist("two")],
        })],
        _ => vec![],
    })
}

#[test]
fn a_dropped_link_reconnects_without_user_input() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = Harness::connect(MockServer::start(certs::generate(), dropping_script()));

    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");
    let before = harness.rt.sources.playlists.items.len();
    assert_eq!(before, 2, "fixture should have loaded two playlists");

    // Ask for state again — the script answers by hanging up.
    harness.dispatch(mkpclient_runtime::SemanticEvent::SendRequest {
        msg: ClientMsg::GetState,
        task_id: None,
    });
    harness
        .tick_until(
            |rt| rt.sources.session.lost_server.is_some(),
            Duration::from_secs(5),
        )
        .expect("the drop was never observed");

    // The view survives the drop: this is what makes a reconnect a
    // pause rather than a reset.
    assert_eq!(
        harness.rt.sources.playlists.items.len(),
        before,
        "playlists were wiped by the drop"
    );
    assert!(
        matches!(harness.rt.sources.screen, Screen::ServerLostModal { .. }),
        "expected the reconnect modal, got {:?}",
        harness.rt.sources.screen
    );

    // No dispatch here on purpose. The runtime must redial by itself.
    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected on its own");

    harness
        .tick_until(
            |rt| rt.sources.session.lost_server.is_none(),
            Duration::from_secs(5),
        )
        .expect("lost_server was never cleared after reconnecting");
    assert!(
        !matches!(harness.rt.sources.screen, Screen::ServerLostModal { .. }),
        "the reconnect modal should close once the link is back"
    );
    assert_eq!(
        harness.rt.sources.playlists.items.len(),
        before,
        "playlists should still be there after reconnecting"
    );
}

/// `LinkPhase::Closed` must be *dialable*, not a state the runtime has
/// to be released from first.
///
/// The original bug was that `Closed` was a dead end: `apply_link` and
/// `connect_action` both acted only from `Idle`, so a dropped link sat
/// there until a user event called `ack_closed`. The fix is not to
/// bounce the phase back to `Idle` — that would be a transition
/// handler — but for `Closed` to read as "nothing open", so the same
/// desired-state query that answers a first connect answers the
/// reconnect (`EXAMPLE-ARCH.md`: "reconnection is emergent").
#[test]
fn a_closed_link_is_dialable_without_being_reset_first() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = Harness::connect(MockServer::start(certs::generate(), dropping_script()));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");

    harness.dispatch(mkpclient_runtime::SemanticEvent::SendRequest {
        msg: ClientMsg::GetState,
        task_id: None,
    });
    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Closed,
            Duration::from_secs(5),
        )
        .expect("the drop never landed as Closed");

    // The close armed a backoff rather than an acknowledgement, and the
    // link is left resting on Closed — no phase rewriting behind its
    // back.
    assert!(
        harness.rt.sources.link.retry_at.is_some(),
        "the drop should have armed a backoff"
    );

    // No dispatch. The link leaves Closed by dialling out of it, which
    // is the whole claim.
    harness
        .tick_until(
            |rt| {
                matches!(
                    rt.sources.link.phase,
                    LinkPhase::Connecting | LinkPhase::Connected
                )
            },
            Duration::from_secs(15),
        )
        .expect("the link never dialled out of Closed on its own");
}
