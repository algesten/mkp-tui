//! End-to-end: the link drops and the runtime gets it back on its
//! own — same server, same view, cursor where it was — while the
//! TUI keeps the main view up under the server-lost modal instead
//! of handing the user the server list.

mod common;

use std::time::{Duration, Instant};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::views::{shell_model, ShellInput, ShellModel};
use mkpclient_runtime::{ClientMsg, Runtime, TuiCursorEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_screen::Screen;
use mkpclient_tui::app::AppState;
use mkproto::{ListTarget, Playlist, ServerMsg, Song};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, Script, ScriptStep};

fn song(id: &str, title: &str) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        artist_name: "artist".into(),
        album_title: "album".into(),
        duration: 60.0,
        track_number: None,
        url: None,
        artwork_url_small: None,
        artwork_url_large: None,
    }
}

fn playlist(id: &str, name: &str) -> Playlist {
    Playlist {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        track_count: 3,
    }
}

/// A server with two playlists; `p1` streams three songs.
fn script() -> Script {
    Box::new(|msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
        ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
        ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
            playlists: vec![playlist("p1", "Morning"), playlist("p2", "Evening")],
        })],
        ClientMsg::GetPlaylist { id, .. } => vec![
            ScriptStep::Reply(ServerMsg::Ok),
            ScriptStep::Broadcast(ServerMsg::ListBegin {
                target: ListTarget::Playlist { id: id.clone() },
                total: 3,
                focus: 0,
            }),
            ScriptStep::Broadcast(ServerMsg::ListChunk {
                target: ListTarget::Playlist { id: id.clone() },
                offset: 0,
                songs: vec![song("a", "Alpha"), song("b", "Bravo"), song("c", "Charlie")],
            }),
        ],
        _ => vec![ScriptStep::Reply(ServerMsg::Ok)],
    })
}

fn shell(rt: &Runtime) -> ShellModel {
    shell_model(ShellInput::new(
        &rt.sources.pairing,
        &rt.sources.link,
        &rt.sources.session,
    ))
}

fn tracks_loaded(rt: &Runtime) -> bool {
    rt.sources.playlist_tracks.playlist_id.as_deref() == Some("p1")
        && rt.sources.playlist_tracks.pending_task.is_none()
        && rt.sources.playlist_tracks.songs.iter().all(|s| s.is_some())
        && rt.sources.playlist_tracks.songs.len() == 3
}

/// The rows on screen survive the drop, so "loaded" alone does not
/// prove anything was fetched again. The restore lifecycle drops its
/// guard when the session is lost and raises it once the resume has
/// re-issued the view; fresh rows on top of that is the proof.
fn resumed(rt: &Runtime) -> bool {
    rt.sources.session.auto_restored_view && tracks_loaded(rt)
}

fn count(msgs: &[ClientMsg], pred: impl Fn(&ClientMsg) -> bool) -> usize {
    msgs.iter().filter(|m| pred(m)).count()
}

/// Connect, let the restore open the first playlist, and park the
/// cursor on the third row.
fn connect_and_browse(mock: MockServer) -> Harness {
    let mut h = Harness::connect(mock);
    h.tick_until(tracks_loaded, Duration::from_secs(5))
        .expect("first playlist should open after connect");
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.tick_once();
    assert_eq!(h.rt.sources.cursor.middle, 2);
    assert!(matches!(h.rt.sources.screen, Screen::NowPlaying));
    h
}

/// What the runtime went through between the drop and the recovery.
#[derive(Default)]
struct Outage {
    saw_closed: bool,
    saw_lost_modal: bool,
    /// Every tick's shell, so a single frame on the server list
    /// would be caught.
    shells: Vec<ShellModel>,
}

/// Tick until the link is back up with the playlist re-fetched and
/// the lost modal gone, recording what was observed on the way.
/// `dropped_already` says the caller has seen the link close.
fn ride_out_the_outage(h: &mut Harness, timeout: Duration, dropped_already: bool) -> Outage {
    let mut outage = Outage {
        saw_closed: dropped_already,
        ..Default::default()
    };
    let deadline = Instant::now() + timeout;
    loop {
        h.rt.tick();
        let s = &h.rt.sources;
        outage.saw_closed |= s.link.phase == LinkPhase::Closed;
        outage.saw_lost_modal |= matches!(s.screen, Screen::ServerLostModal { .. });
        outage.shells.push(shell(&h.rt));
        let recovered = outage.saw_closed
            && s.link.phase == LinkPhase::Connected
            && resumed(&h.rt)
            && matches!(s.screen, Screen::NowPlaying)
            && s.session.lost_server.is_none();
        if recovered {
            return outage;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "did not recover: phase={:?} screen={:?} lost={:?} tracks={:?}",
            s.link.phase,
            s.screen,
            s.session.lost_server,
            s.playlist_tracks.playlist_id
        );
        h.rt.wait_for_wake(remaining.min(Duration::from_millis(50)));
    }
}

#[test]
fn reconnects_to_the_same_server_and_resumes_the_view() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let before = h.mock.received();

    h.mock.drop_client();
    let outage = ride_out_the_outage(&mut h, Duration::from_secs(10), false);

    assert!(outage.saw_closed, "the drop should have been observed");
    assert!(outage.saw_lost_modal, "the lost modal should have been up");
    assert!(
        outage.shells.iter().all(|s| *s == ShellModel::Main),
        "never fall back to the server list: {:?}",
        outage.shells
    );

    // Same view, same row.
    let s = &h.rt.sources;
    assert!(matches!(s.history.mode, MiddleMode::PlaylistSongs));
    assert_eq!(s.playlist_tracks.playlist_id.as_deref(), Some("p1"));
    assert_eq!(s.cursor.middle, 2);
    assert_eq!(
        s.session.backend_name.as_deref(),
        Some(h.server_name().as_str())
    );

    // Fresh state was requested over the new link: a second
    // handshake and a second fetch of the playlist on screen.
    let after = h.mock.received();
    let hellos = |m: &[ClientMsg]| count(m, |m| matches!(m, ClientMsg::Hello { .. }));
    let fetches = |m: &[ClientMsg]| {
        count(
            m,
            |m| matches!(m, ClientMsg::GetPlaylist { id, .. } if id == "p1"),
        )
    };
    assert_eq!(hellos(&after), hellos(&before) + 1);
    assert_eq!(fetches(&after), fetches(&before) + 1);
}

#[test]
fn reconnects_when_the_server_comes_back_on_a_new_port() {
    let _ = env_logger::builder().is_test(true).try_init();
    let certs = certs::generate();
    let mut h = connect_and_browse(MockServer::start(certs.clone(), script()));
    let name = h.server_name();

    // The server quits and relaunches: same identity (cert), new
    // OS-assigned port, re-advertised under the same mDNS name.
    h.mock.drop_client();
    let relaunched = MockServer::start(certs, script());
    h.rt.sources.discovery.upsert(ServerAd {
        name: name.clone(),
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: relaunched.addr.port(),
    });

    let outage = ride_out_the_outage(&mut h, Duration::from_secs(10), false);
    assert!(outage.shells.iter().all(|s| *s == ShellModel::Main));

    // The new address had no cached probe, so the runtime probed it
    // and then dialed it; the relaunched server saw the handshake
    // and served the resumed view.
    let on_new = relaunched.received();
    assert!(
        on_new.iter().any(|m| matches!(m, ClientMsg::Hello { .. })),
        "expected the handshake on the relaunched server, got {on_new:?}"
    );
    assert!(
        on_new
            .iter()
            .any(|m| matches!(m, ClientMsg::GetPlaylist { id, .. } if id == "p1")),
        "expected the open playlist to be fetched from the relaunched server, got {on_new:?}"
    );
    assert_eq!(h.rt.sources.cursor.middle, 2);
    assert_eq!(
        h.rt.sources.playlist_tracks.playlist_id.as_deref(),
        Some("p1")
    );
}

#[test]
fn keeps_waiting_while_the_server_is_away() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let name = h.server_name();

    // Gone from mDNS as well as from the wire.
    h.mock.drop_client();
    h.rt.sources.discovery.remove(&name);

    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("drop observed");
    // Well past the reconnect backoff: still waiting, still on the
    // main view with the modal up, nothing dialed.
    let waited_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < waited_until {
        h.rt.tick();
        assert_eq!(h.rt.sources.link.phase, LinkPhase::Closed);
        assert_eq!(shell(&h.rt), ShellModel::Main);
        assert!(matches!(
            h.rt.sources.screen,
            Screen::ServerLostModal { .. }
        ));
        h.rt.wait_for_wake(Duration::from_millis(50));
    }

    // Back in mDNS at the same address: reconnects.
    h.rt.sources.discovery.upsert(ServerAd {
        name,
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: h.mock.addr.port(),
    });
    let outage = ride_out_the_outage(&mut h, Duration::from_secs(10), true);
    assert!(outage.shells.iter().all(|s| *s == ShellModel::Main));
}

#[test]
fn giving_up_hands_the_server_list_back_and_stops_dialing() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));

    h.mock.drop_client();
    h.tick_until(
        |rt| matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
        Duration::from_secs(5),
    )
    .expect("lost modal");

    // Enter on the modal.
    h.dispatch(TuiCursorEvent::ServerLostGiveUp);
    h.tick_once();
    assert_eq!(shell(&h.rt), ShellModel::PreConnect);

    let hellos_before = count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. }));
    let waited_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < waited_until {
        h.rt.tick();
        assert_ne!(h.rt.sources.link.phase, LinkPhase::Connected);
        assert_eq!(shell(&h.rt), ShellModel::PreConnect);
        h.rt.wait_for_wake(Duration::from_millis(50));
    }
    let hellos_after = count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. }));
    assert_eq!(
        hellos_after, hellos_before,
        "nothing should have been dialed"
    );
}

#[test]
fn outage_paints_the_main_view_with_the_lost_modal() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let name = h.server_name();

    // Keep the server away so the frame is a steady state.
    h.mock.drop_client();
    h.rt.sources.discovery.remove(&name);
    h.tick_until(
        |rt| matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
        Duration::from_secs(5),
    )
    .expect("lost modal");

    let app = AppState::default();
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
    terminal
        .draw(|frame| mkpclient_tui::render::draw(frame, &app, &h.rt))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let text: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    let contains = |needle: &str| text.iter().any(|row| row.contains(needle));

    assert!(contains(&format!("Lost connection to {name}")), "{text:#?}");
    assert!(contains("reconnecting"), "{text:#?}");
    assert!(contains("Queue"), "main view should be up: {text:#?}");
    assert!(
        !contains("Searching for Make Play server"),
        "must not paint the pre-connect screen: {text:#?}"
    );
}
