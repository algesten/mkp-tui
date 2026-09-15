//! End-to-end: the link drops and the runtime gets it back on its
//! own — same server, same view, cursor where it was — while the
//! TUI keeps the main view up under the server-lost modal instead
//! of handing the user the server list.

mod common;

use std::time::{Duration, Instant};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::views::{shell_model, ShellInput, ShellModel};
use mkpclient_runtime::{ClientMsg, Runtime, SemanticEvent, TuiCursorEvent};
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

#[test]
fn startup_retries_a_failed_probe_without_user_input() {
    let mock = MockServer::start_with_rejected_connections(certs::generate(), script(), 1);
    let mut h = Harness::prepare(mock);
    let name = h.server_name();
    let addr = h.mock.addr.to_string();
    h.rt.sources.probes.invalidate(&addr);
    h.rt.sources.session.preferred_server = Some(name.clone().into());
    h.rt.sources.session.auto_connect = true;

    // A restarted client waits for discovery before its first attempt.
    h.rt.sources.discovery.remove(&name);
    h.tick_once();
    assert_eq!(shell(&h.rt), ShellModel::PreConnect);
    assert!(h.rt.sources.intent.target.is_none());
    h.rt.sources.discovery.upsert(ServerAd {
        name: name.clone(),
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: h.mock.addr.port(),
    });
    h.tick_until(
        |rt| {
            matches!(
                rt.sources.probes.get(&addr),
                Some(mkpclient_state_probes::ProbeOutcome::Failed { .. })
            )
        },
        Duration::from_secs(5),
    )
    .expect("the server should reject the first probe");
    assert!(h.rt.sources.session.backend_name.is_none());

    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Connected && resumed(rt),
        Duration::from_secs(10),
    )
    .expect("startup should retry the failed probe and load the view");
    assert_eq!(shell(&h.rt), ShellModel::Main);
    assert_eq!(
        h.rt.sources.session.backend_name.as_deref(),
        Some(name.as_str())
    );
    assert_eq!(
        count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. })),
        1
    );
}

#[test]
fn startup_retries_a_failed_connection_without_user_input() {
    let mock = MockServer::start_with_rejected_connections(certs::generate(), script(), 1);
    let mut h = Harness::prepare(mock);
    let name = h.server_name();
    // The probe already succeeded, but the server rejects the actual
    // client connection. There has never been a session to reconnect.
    h.rt.sources.session.preferred_server = Some(name.clone().into());
    h.rt.sources.session.auto_connect = true;
    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("the server should reject the first connection");
    assert!(h.rt.sources.session.backend_name.is_none());
    assert!(h.rt.sources.session.lost_server.is_none());

    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Connected && resumed(rt),
        Duration::from_secs(10),
    )
    .expect("startup should retry the failed connection and load the view");
    assert_eq!(shell(&h.rt), ShellModel::Main);
    assert_eq!(
        h.rt.sources.session.backend_name.as_deref(),
        Some(name.as_str())
    );
    assert_eq!(
        count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. })),
        1
    );
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

#[test]
fn switching_servers_is_not_a_loss() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let old = h.server_name();

    // A second server, paired under a different cert, comes into
    // view; the user picks it in the switch modal.
    let other = MockServer::start(certs::generate(), script());
    let other_name = format!("mock-{}", other.addr.port());
    h.rt.sources.discovery.upsert(ServerAd {
        name: other_name.clone(),
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: other.addr.port(),
    });
    h.rt.sources
        .credentials
        .insert(mkpclient_state_credentials::PairingEntry {
            fingerprint: other.certs.fingerprint.clone(),
            host: "127.0.0.1".into(),
            server_cert_pem: other.certs.server_cert_pem.clone(),
            client_cert_pem: other.certs.client_cert_pem.clone(),
            client_key_pem: other.certs.client_key_pem.clone(),
        });
    let selected =
        h.rt.sources
            .discovery
            .servers
            .iter()
            .position(|s| s.name == other_name)
            .expect("other server listed");
    h.rt.sources.screen = Screen::ServerPicker { selected };
    h.dispatch(TuiCursorEvent::ServerPickerModalSelect);

    // Every tick until the new server is current: never reported as
    // a loss of the old one, and no modal about it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.rt.tick();
        let s = &h.rt.sources;
        assert!(
            s.session.lost_server.is_none(),
            "a switch must not stash the server being left"
        );
        assert!(!matches!(s.screen, Screen::ServerLostModal { .. }));
        // `resumed` rather than `tracks_loaded`: the old server's rows
        // survive the close, so only rows fetched after the restore
        // ran prove the new server is serving the view.
        if s.session.backend_name.as_deref() == Some(other_name.as_str()) && resumed(&h.rt) {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "switch did not complete: {:?}",
            s.link.phase
        );
        h.rt.wait_for_wake(remaining.min(Duration::from_millis(50)));
    }
    assert_ne!(old, other_name);
    assert!(other
        .received()
        .iter()
        .any(|m| matches!(m, ClientMsg::Hello { .. })));
}

#[test]
fn disconnecting_during_a_reconnect_stops_it() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let name = h.server_name();

    h.mock.drop_client();
    h.rt.sources.discovery.remove(&name);
    h.tick_until(
        |rt| matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
        Duration::from_secs(5),
    )
    .expect("lost modal");

    // The user says stop while the runtime is still waiting.
    h.dispatch(SemanticEvent::Disconnect);
    h.tick_once();
    assert!(h.rt.sources.session.lost_server.is_none());
    assert_eq!(shell(&h.rt), ShellModel::PreConnect);

    // The server comes back: nothing dials it.
    h.rt.sources.discovery.upsert(ServerAd {
        name,
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: h.mock.addr.port(),
    });
    let hellos_before = count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. }));
    let waited_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < waited_until {
        h.rt.tick();
        assert_ne!(h.rt.sources.link.phase, LinkPhase::Connected);
        assert_eq!(shell(&h.rt), ShellModel::PreConnect);
        h.rt.wait_for_wake(Duration::from_millis(50));
    }
    let hellos_after = count(&h.mock.received(), |m| matches!(m, ClientMsg::Hello { .. }));
    assert_eq!(hellos_after, hellos_before);
}

#[test]
fn reconnect_preserves_album_search_selection_after_delayed_results() {
    use mkproto::{Album, SearchResults, SearchType};
    let base = script();
    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| {
            if matches!(msg, ClientMsg::Search { .. }) {
                // Let the runtime tick while the resumed search is in flight.
                std::thread::sleep(Duration::from_millis(150));
                return vec![ScriptStep::Reply(ServerMsg::Search(
                    SearchResults::Albums {
                        albums: (0..3)
                            .map(|i| Album {
                                id: format!("album-{i}"),
                                name: format!("Album {i}"),
                                artist_id: "artist".into(),
                                artist_name: "Artist".into(),
                                track_count: 3,
                                detail: None,
                                url: None,
                                artwork_url_small: None,
                                artwork_url_large: None,
                            })
                            .collect(),
                    },
                ))];
            }
            base(msg)
        }),
    );
    let mut h = connect_and_browse(mock);
    h.dispatch(SemanticEvent::RestoreSavedSearch {
        query: "album".into(),
        search_type: SearchType::Album,
        selected: 0,
        selected_id: None,
    });
    h.tick_until(
        |rt| rt.sources.search.albums.len() == 3,
        Duration::from_secs(5),
    )
    .expect("album results");
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.tick_once();
    assert_eq!(h.rt.sources.cursor.middle, 2);
    let original_task = h.rt.sources.search.task_id;
    h.mock.drop_client();
    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("drop observed");
    h.tick_until(
        |rt| {
            rt.sources.link.phase == LinkPhase::Connected
                && rt.sources.session.auto_restored_view
                && rt.sources.search.task_id != original_task
                && rt.sources.search.albums.len() == 3
        },
        Duration::from_secs(10),
    )
    .expect("album search resumed");
    assert!(matches!(
        h.rt.sources.history.mode,
        MiddleMode::SearchResults {
            search_type: SearchType::Album,
            ..
        }
    ));
    assert_eq!(
        h.rt.sources.cursor.middle, 2,
        "keep the selected album after reconnect"
    );
}

#[test]
fn switching_to_an_empty_backend_drops_the_previous_backends_rows() {
    let mut h = connect_and_browse(MockServer::start(certs::generate(), script()));
    let other = MockServer::start(
        certs::generate(),
        Box::new(|msg| match msg {
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![],
            })],
            _ => vec![ScriptStep::Reply(ServerMsg::Ok)],
        }),
    );
    let name = format!("mock-{}", other.addr.port());
    h.rt.sources.discovery.upsert(ServerAd {
        name: name.clone(),
        host: "127.0.0.1".into(),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: other.addr.port(),
    });
    h.rt.sources
        .credentials
        .insert(mkpclient_state_credentials::PairingEntry {
            fingerprint: other.certs.fingerprint.clone(),
            host: "127.0.0.1".into(),
            server_cert_pem: other.certs.server_cert_pem.clone(),
            client_cert_pem: other.certs.client_cert_pem.clone(),
            client_key_pem: other.certs.client_key_pem.clone(),
        });
    let selected =
        h.rt.sources
            .discovery
            .servers
            .iter()
            .position(|s| s.name == name)
            .unwrap();
    h.rt.sources.screen = Screen::ServerPicker { selected };
    h.dispatch(TuiCursorEvent::ServerPickerModalSelect);
    h.tick_until(
        |rt| {
            rt.sources.session.backend_name.as_deref() == Some(name.as_str())
                && rt.sources.session.auto_restored_view
                && rt.sources.playlists.loaded
        },
        Duration::from_secs(10),
    )
    .expect("empty backend connected and restored");
    assert!(h.rt.sources.playlists.items.is_empty());
    assert!(
        h.rt.sources.playlist_tracks.songs.is_empty(),
        "the new server must not expose the previous server's playable rows"
    );
    assert!(h.rt.sources.playlist_tracks.playlist_id.is_none());
}

#[test]
fn reconnect_preserves_artist_detail_selection_after_delayed_results() {
    let base = script();
    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| {
            if matches!(msg, ClientMsg::GetArtistDetail { .. }) {
                std::thread::sleep(Duration::from_millis(150));
                return vec![ScriptStep::Reply(ServerMsg::ArtistDetail {
                    artist: mkproto::Artist {
                        id: "artist".into(),
                        name: "Artist".into(),
                        detail: None,
                        url: None,
                        artwork_url_small: None,
                        artwork_url_large: None,
                    },
                    top_songs: vec![song("a", "Alpha"), song("b", "Bravo"), song("c", "Charlie")],
                })];
            }
            base(msg)
        }),
    );
    let mut h = connect_and_browse(mock);
    h.dispatch(SemanticEvent::RestoreSavedArtist {
        artist_id: "artist".into(),
        artist_name: "Artist".into(),
        selected: 0,
    });
    let detail_loaded = |rt: &Runtime| match rt.sources.history.mode {
        MiddleMode::ArtistDetail {
            awaiting_seq: Some(seq),
            ..
        } => matches!(
            rt.sources.responses.by_seq.get(&seq).map(|r| r.as_ref()),
            Some(ServerMsg::ArtistDetail { .. })
        ),
        _ => false,
    };
    h.tick_until(detail_loaded, Duration::from_secs(5))
        .expect("artist detail loaded");
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.tick_once();
    assert_eq!(h.rt.sources.cursor.middle, 2);
    let original_mode = h.rt.sources.history.mode.clone();
    h.mock.drop_client();
    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("drop observed");
    h.tick_until(
        |rt| {
            rt.sources.link.phase == LinkPhase::Connected
                && rt.sources.session.auto_restored_view
                && rt.sources.history.mode != original_mode
                && detail_loaded(rt)
        },
        Duration::from_secs(10),
    )
    .expect("artist detail resumed");
    assert_eq!(
        h.rt.sources.cursor.middle, 2,
        "keep artist detail selection across reconnect"
    );
}

#[test]
fn reconnect_to_an_emptied_library_discards_deleted_playlist_rows() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let emptied = Arc::new(AtomicBool::new(false));
    let server_emptied = emptied.clone();
    let base = script();
    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| {
            if matches!(msg, ClientMsg::GetPlaylists) && server_emptied.load(Ordering::SeqCst) {
                return vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })];
            }
            base(msg)
        }),
    );
    let mut h = connect_and_browse(mock);
    emptied.store(true, Ordering::SeqCst);
    h.mock.drop_client();
    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("drop observed");
    h.tick_until(
        |rt| {
            rt.sources.link.phase == LinkPhase::Connected
                && rt.sources.session.auto_restored_view
                && rt.sources.playlists.loaded
        },
        Duration::from_secs(10),
    )
    .expect("empty library reconnected");
    assert!(h.rt.sources.playlists.items.is_empty());
    assert!(
        h.rt.sources.playlist_tracks.songs.is_empty(),
        "deleted playlist must not retain playable rows after reconnect"
    );
    assert!(h.rt.sources.playlist_tracks.playlist_id.is_none());
}

#[test]
fn reconnect_preserves_search_selection_from_a_later_streamed_page() {
    use mkproto::{SearchResults, SearchType};
    let base = script();
    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| {
            if matches!(msg, ClientMsg::Search { .. }) {
                return vec![
                    ScriptStep::Reply(ServerMsg::Search(SearchResults::Songs {
                        songs: vec![song("a", "Alpha"), song("b", "Bravo")],
                    })),
                    ScriptStep::Delay(Duration::from_millis(150)),
                    ScriptStep::BroadcastForRequestTask(ServerMsg::SearchMore(
                        SearchResults::Songs {
                            songs: vec![song("c", "Charlie")],
                        },
                    )),
                ];
            }
            base(msg)
        }),
    );
    let mut h = connect_and_browse(mock);
    h.dispatch(SemanticEvent::RestoreSavedSearch {
        query: "song".into(),
        search_type: SearchType::Song,
        selected: 0,
        selected_id: None,
    });
    h.tick_until(
        |rt| rt.sources.search.songs.len() == 3,
        Duration::from_secs(5),
    )
    .expect("all search pages");
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.dispatch(TuiCursorEvent::MiddleCursorDown);
    h.tick_once();
    assert_eq!(h.rt.sources.cursor.middle, 2);
    let original_task = h.rt.sources.search.task_id;
    h.mock.drop_client();
    h.tick_until(
        |rt| rt.sources.link.phase == LinkPhase::Closed,
        Duration::from_secs(5),
    )
    .expect("drop observed");
    h.tick_until(
        |rt| {
            rt.sources.link.phase == LinkPhase::Connected
                && rt.sources.session.auto_restored_view
                && rt.sources.search.task_id != original_task
                && rt.sources.search.songs.len() == 3
        },
        Duration::from_secs(10),
    )
    .expect("streamed search resumed");
    assert_eq!(
        h.rt.sources.cursor.middle, 2,
        "keep the song selected on a later search page"
    );
}
