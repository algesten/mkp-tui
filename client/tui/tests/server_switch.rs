//! End-to-end: picking a server in the switch modal is enough.
//!
//! Regression cover for a switch that tore the old link down and then
//! dropped the user on the full-screen server list — the picker
//! asking them to choose the server they had just chosen. Nothing
//! here dispatches a user event after the selection: if the runtime
//! doesn't carry the switch through on its own, the test times out.

mod common;

use std::time::{Duration, Instant};

use mkpclient_runtime::{views, ClientMsg, Runtime, TuiCursorEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_screen::Screen;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

/// A server whose playlists name it, so the two are told apart by
/// what they serve.
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

/// Which server the modal's cursor is on.
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

/// Would the TUI be painting the full-screen server list right now?
/// Mirrors the gate in `render::draw`: the pre-connect screen takes
/// the whole frame whenever the link is down and the reconnect modal
/// isn't holding the main view up. This is the failure the bug report
/// describes.
fn shows_full_screen_picker(rt: &Runtime) -> bool {
    if rt.sources.link.phase == LinkPhase::Connected {
        return false;
    }
    if matches!(rt.sources.screen, Screen::ServerLostModal { .. }) {
        return false;
    }
    let model = views::pre_connect_model(
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
    );
    matches!(model, views::PreConnectModel::ServerList { .. })
}

/// Connect to one server, publish a second, and drive the left pane's
/// server row → modal → select sequence onto it. Returns the harness,
/// the second server's name, and the mock backing it (kept alive by
/// the caller).
fn switch_to_second(
    first_tag: &'static str,
    second_tag: &'static str,
) -> (Harness, String, MockServer) {
    let mut harness = Harness::connect(MockServer::start(certs::generate(), script(first_tag)));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");

    let second_mock = MockServer::start(certs::generate(), script(second_tag));
    let second = harness.publish(&second_mock);
    assert_ne!(
        harness.rt.sources.session.backend_name.as_deref(),
        Some(second.as_str()),
        "fixture should start on the other server"
    );

    // Open the picker from the left pane's server row.
    harness.rt.sources.cursor.left = 0;
    harness.dispatch(TuiCursorEvent::LeftActivate);
    assert!(
        matches!(harness.rt.sources.screen, Screen::ServerPicker { .. }),
        "the server row should open the picker modal, got {:?}",
        harness.rt.sources.screen
    );

    // Walk onto the other server. The cursor starts on the connected
    // one, so this is the "select one" step of the report.
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
    (harness, second, second_mock)
}

#[test]
fn picking_a_server_in_the_modal_is_enough_to_switch() {
    let _ = env_logger::builder().is_test(true).try_init();

    let (mut harness, second, _mock) = switch_to_second("alpha", "beta");

    // No further input from here. Every tick is inspected: the switch
    // must never fall back to the full-screen list, and must never be
    // reported to the user as a lost connection.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        harness.tick_once();
        assert!(
            !shows_full_screen_picker(&harness.rt),
            "the switch dropped out to the full-screen server selection"
        );
        assert!(
            !matches!(harness.rt.sources.screen, Screen::ServerLostModal { .. }),
            "a switch the user asked for was reported as a lost connection"
        );
        if harness.rt.sources.session.backend_name.as_deref() == Some(second.as_str()) {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "the switch never completed");
        harness
            .rt
            .wait_for_wake(remaining.min(Duration::from_millis(50)));
    }

    assert_eq!(harness.rt.sources.link.phase, LinkPhase::Connected);
    assert!(
        matches!(harness.rt.sources.screen, Screen::NowPlaying),
        "expected the main view after the switch, got {:?}",
        harness.rt.sources.screen
    );
    assert!(
        harness.rt.sources.session.lost_server.is_none(),
        "a deliberate switch left a lost server behind"
    );
}

#[test]
fn switching_replaces_the_previous_servers_view() {
    let _ = env_logger::builder().is_test(true).try_init();

    let (mut harness, second, _mock) = switch_to_second("alpha", "beta");

    harness
        .tick_until(
            |rt| rt.sources.session.backend_name.as_deref() == Some(second.as_str()),
            Duration::from_secs(15),
        )
        .expect("the switch never completed");

    // Checked the instant the new server becomes current, before its
    // own `GetPlaylists` reply lands — waiting for that would pass
    // either way, since the reply replaces the list wholesale. The
    // window in between is the one that shows the previous server's
    // playlists under the new server's name.
    assert!(
        !harness
            .rt
            .sources
            .playlists
            .items
            .iter()
            .any(|p| p.id == "alpha"),
        "the previous server's playlists survived into the new session"
    );

    // And the new server's own world does arrive.
    harness
        .tick_until(
            |rt| rt.sources.playlists.items.iter().any(|p| p.id == "beta"),
            Duration::from_secs(5),
        )
        .expect("the new server's playlists never arrived");
}
