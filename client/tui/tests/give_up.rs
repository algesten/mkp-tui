//! End-to-end: "give up" on a lost server must actually give up.
//!
//! `server_lost_give_up` clears `lost_server`, `preferred_server` and
//! `auto_connect` so that nothing is wanted any more — but it only
//! reaches `disconnect()` (the one place that clears `intent.target`)
//! when `link.phase != Idle`, and the link is released to `Idle`
//! inside the same tick that observed the close. So the target
//! outlives the give-up.
//!
//! That stale target is now load-bearing twice over: `pre_connect`
//! reads it as "a connect we can still make progress on" and paints
//! the progress status instead of the server list, and `apply_link`
//! reads it as a link to bring up.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_runtime::{views, ClientMsg, Runtime, TuiCursorEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_screen::Screen;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

/// Serves a normal session but hangs up the second time it is asked
/// for state — the first `GetState` is the harness's connect
/// handshake, the second is the test asking for the drop. The
/// listener stays bound, so the server is still reachable afterwards.
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
            playlists: vec![Playlist {
                id: "one".into(),
                name: "one".into(),
                description: String::new(),
                track_count: 3,
            }],
        })],
        _ => vec![],
    })
}

/// The model the TUI would paint full-screen right now (`render::draw`
/// hands the whole frame to the pre-connect screen whenever the link
/// is down and the reconnect modal isn't holding the main view up).
fn pre_connect(rt: &Runtime) -> views::PreConnectModel {
    views::pre_connect_model(
        views::PreConnectInput::new(
            &rt.sources.discovery,
            &rt.sources.link,
            &rt.sources.probes,
            &rt.sources.credentials,
        ),
        rt.sources.session.preferred_server.as_deref(),
        rt.sources.session.lost_server.as_deref(),
        rt.sources.session.auto_connect,
        rt.sources.cursor.server_picker,
        rt.sources.intent.target.as_deref(),
    )
}

/// Connect, let the server hang up, and wait for the lost-server
/// modal to come up.
fn harness_after_a_drop() -> Harness {
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
            |rt| matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
            Duration::from_secs(5),
        )
        .expect("the lost-server modal never came up");
    harness
}

#[test]
fn giving_up_hands_back_the_picker() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = harness_after_a_drop();
    let lost = harness
        .rt
        .sources
        .session
        .lost_server
        .as_deref()
        .expect("the drop should have stashed a lost server")
        .to_string();

    // "Enter = pick another": the whole point of the binding is to
    // get the user to the list of servers.
    harness.dispatch(TuiCursorEvent::ServerLostGiveUp);

    let model = pre_connect(&harness.rt);
    assert!(
        matches!(model, views::PreConnectModel::ServerList { .. }),
        "giving up on {lost} left the pre-connect screen showing progress \
         with no way back to the picker: {model:?}"
    );
}

#[test]
fn giving_up_does_not_redial_the_server() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = harness_after_a_drop();
    harness.dispatch(TuiCursorEvent::ServerLostGiveUp);

    // Nothing is wanted any more — `lost_server`, `preferred_server`
    // and `auto_connect` were all cleared, and the retry schedule was
    // wiped. The link must come to rest rather than dial the server
    // the user just walked away from.
    let redialled = harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(3),
        )
        .is_ok();
    assert!(
        !redialled,
        "the runtime reconnected to the server the user gave up on"
    );
}
