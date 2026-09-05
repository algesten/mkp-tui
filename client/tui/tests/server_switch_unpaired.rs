//! End-to-end: switching to a server that isn't paired yet is still a
//! switch.
//!
//! The picker modal lists every discovered server, paired or not, and
//! `execute::apply_link` deliberately routes an unpaired target into
//! pairing — it moves the name out of `intent.target` and into
//! `intent.pair_target` (`fallback_target_to_pair`). That move can
//! happen on the tick before the outgoing link's close is observed,
//! and `backend_action` only reads `intent.target`: with the name
//! gone from there, the teardown half of the switch is scored as a
//! lost connection.

mod common;

use std::time::Duration;

use mkpclient_runtime::{ClientMsg, Runtime, TuiCursorEvent};
use mkpclient_state_ui_screen::Screen;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

fn script(tag: &'static str) -> common::mock_server::Script {
    Box::new(move |msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
        ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
        ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
            playlists: vec![Playlist {
                id: tag.into(),
                name: tag.into(),
                description: String::new(),
                track_count: 3,
            }],
        })],
        _ => vec![],
    })
}

fn selected_name(rt: &Runtime) -> Option<String> {
    let Screen::ServerPicker { selected } = rt.sources.screen else {
        return None;
    };
    rt.sources
        .discovery
        .servers
        .iter()
        .nth(selected)
        .map(|s| s.name.clone())
}

#[test]
fn switching_to_an_unpaired_server_is_not_a_lost_connection() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script("alpha")));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");

    let second_mock = MockServer::start(certs::generate(), script("beta"));
    let second = harness.publish(&second_mock);
    // Discovered and probed, but never paired — the state the picker
    // is in for any server on the network the user hasn't used yet.
    harness
        .rt
        .sources
        .credentials
        .remove(&second_mock.certs.fingerprint);

    harness.rt.sources.cursor.left = 0;
    harness.dispatch(TuiCursorEvent::LeftActivate);
    for _ in 0..harness.rt.sources.discovery.servers.len() {
        if selected_name(&harness.rt).as_deref() == Some(second.as_str()) {
            break;
        }
        harness.dispatch(TuiCursorEvent::ServerPickerModalCursorDown);
    }
    assert_eq!(
        selected_name(&harness.rt).as_deref(),
        Some(second.as_str()),
        "could not put the modal cursor on the second server"
    );
    harness.dispatch(TuiCursorEvent::ServerPickerModalSelect);

    // The user asked for this teardown. Whether the runtime reaches
    // the new server (it has to pair first, and the mock won't) is
    // beside the point — it must not tell the user the connection to
    // the old one was lost, nor re-arm the auto-reconnect that drags
    // them back to it.
    for _ in 0..40 {
        harness.tick_once();
        assert!(
            harness.rt.sources.session.lost_server.is_none(),
            "a switch the user asked for stashed a lost server: {:?} \
             (intent.target = {:?}, intent.pair_target = {:?})",
            harness.rt.sources.session.lost_server,
            harness.rt.sources.intent.target,
            harness.rt.sources.intent.pair_target
        );
        assert!(
            !matches!(harness.rt.sources.screen, Screen::ServerLostModal { .. }),
            "a switch the user asked for was reported as a lost connection"
        );
        harness.rt.wait_for_wake(Duration::from_millis(10));
    }
}
