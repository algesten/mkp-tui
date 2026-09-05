//! Round-2 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Each test expresses behaviour the brief promises — "reconnect
//! transparently, without interruption to local state" — but that the
//! branch does not deliver at `0dcc1e4`.

mod common;

use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{ClientMsg, SemanticEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_probes::ProbeOutcome;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, Script, ScriptStep};

fn playlist(id: &str) -> Playlist {
    Playlist {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        track_count: 3,
    }
}

/// Serves a normal session, but hangs up the *second* time it is asked
/// for state. The first `GetState` is the harness's connect handshake;
/// the second is the test asking for the drop.
fn dropping_script() -> Script {
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

/// An unremarkable server that never hangs up.
fn plain_script() -> Script {
    Box::new(move |msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
        ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
        ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
            playlists: vec![playlist("one"), playlist("two")],
        })],
        _ => vec![],
    })
}

/// Grab a port the OS is willing to hand out, then release it — so the
/// next TCP connect to it is refused until something binds it again.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("local_addr").port()
}

/// **A single failed probe ends the reconnect for good.**
///
/// `ProbeOutcome::Failed` (and `InFlight`) are absolute vetoes in
/// `link_action` (`execute.rs`: `Some(InFlight) | Some(Failed(_)) =>
/// LinkAction::Noop`), and the only thing that ever clears them is
/// `Probes::retry_unresolved`, which runs from exactly three places:
/// `apply_link_ack` (on a `Closed` → `Idle` release), `connect_to` and
/// `begin_pair` (explicit user picks).
///
/// A probe failure does not close a link — the link is already `Idle`
/// — so none of those three fire, and no backoff is re-armed either
/// (`apply_link_ack` only arms one when it sees `Closed`). The link
/// sits on `Idle` with `intent.target` set, a poisoned probe, and no
/// deadline: the reconnect the PR exists to deliver is over after one
/// attempt, and the modal stays up forever while the 8 Hz spinner
/// wake keeps the loop busy.
///
/// The route is the one the brief calls out: the mkp server binds an
/// **OS-assigned port**, so restarting it re-advertises the same mDNS
/// name at a new `addr:port`. That address has no cached fingerprint,
/// so the reconnect must probe it — and a probe fired while the
/// server's listener is still coming up simply fails.
#[test]
fn a_reconnect_probe_that_fails_once_is_tried_again() {
    let _ = env_logger::builder().is_test(true).try_init();

    let certs = certs::generate();
    let mut harness = Harness::connect(MockServer::start(certs.clone(), dropping_script()));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");

    let server_name = format!("mock-{}", harness.mock.addr.port());

    // The drop.
    harness.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::GetState,
        task_id: None,
    });
    harness
        .tick_until(
            |rt| rt.sources.session.lost_server.is_some(),
            Duration::from_secs(5),
        )
        .expect("the drop was never observed");

    // The server comes back on a new OS-assigned port; mDNS refreshes
    // the same name. Nothing is listening there yet.
    let port = free_port();
    harness.rt.sources.discovery.upsert(ServerAd {
        name: server_name,
        host: "127.0.0.1".into(),
        addr: Ipv4Addr::LOCALHOST,
        port,
    });
    let addr_key = format!("127.0.0.1:{port}");

    // The reconnect probes the new address and the probe fails.
    harness
        .tick_until(
            |rt| {
                matches!(
                    rt.sources.probes.get(&addr_key),
                    Some(ProbeOutcome::Failed(_))
                )
            },
            Duration::from_secs(10),
        )
        .expect("the reconnect never probed the server's new address");

    // …and now the server's listener is up.
    let _mock2 = MockServer::start_at(port, certs, plain_script());

    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect(
            "the reconnect gave up permanently after one failed probe: \
             `Failed` is a veto in `link_action` and nothing re-arms a \
             backoff or clears the probe once the link is already Idle",
        );
}

/// **An explicit `Disconnect` now redials itself.**
///
/// `SemanticEvent::Disconnect` (the runtime API the iOS shell calls
/// through `mkp_client_disconnect`) clears `intent.target` and tears
/// the socket down. The close is then observed by `apply_backend`,
/// whose `Clear` arm sets `session.lost_server = Some(old)` and
/// `session.auto_connect = true` — the breadcrumbs that mean "we lost
/// this one, get it back".
///
/// On `main` that was harmless: the link parked on `LinkPhase::Closed`
/// and both `connect_action` and `apply_link` only act from `Idle`, so
/// nothing dialled. `link_ack` now releases the link to `Idle`, so the
/// breadcrumbs are acted on and the runtime reconnects to the server
/// the caller just asked to leave — with the "Connection lost" modal
/// on screen in between.
///
/// This is the same defect class as the round-1 give-up blocker
/// (`server_lost_give_up` had to start clearing `intent`
/// unconditionally), at the entry point that was not fixed.
#[test]
fn an_explicit_disconnect_stays_disconnected() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut harness = Harness::connect(MockServer::start(certs::generate(), plain_script()));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("playlists did not load");

    harness.dispatch(SemanticEvent::Disconnect);

    // Well past the first backoff step (500 ms).
    let redialled = harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(3),
        )
        .is_ok();

    assert!(
        !redialled,
        "an explicit Disconnect redialled the server it had just left — \
         `apply_backend`'s Clear arm re-arms `lost_server` + \
         `auto_connect`, and the link is no longer parked on Closed to \
         stop it"
    );
}

/// **The retained view is thrown away by the reconnect handshake.**
///
/// `backend_action` goes to some trouble to answer `drop_retained:
/// false` for a genuine reconnect, because "dropping it would blank
/// the screen for the duration of the refetch". The handshake then
/// blanks it anyway: the real server answers every `Hello` with
/// `ServerMsg::BackendChanged` (`server/src/session.rs`, unconditional
/// on Hello), and `ingest`'s `BackendChanged` arm resets `queue`,
/// `playlists`, `playlist_tracks` and `search` wholesale.
///
/// So the data survives the outage and dies the moment the link comes
/// back — after `apply_backend` has already closed the modal — which
/// is precisely the reset the CHANGELOG says no longer happens. The
/// mock below replies to `Hello` exactly as the server does, and is
/// slow to answer the reconnect's `GetPlaylists`, which is all it
/// takes for the blank to be on screen.
#[test]
fn the_retained_view_survives_the_reconnect_handshake() {
    let _ = env_logger::builder().is_test(true).try_init();

    // Answers Hello with BackendChanged like the real server; drops on
    // the second GetState; and does not answer the reconnect's
    // GetPlaylists, standing in for a server that is slow to refetch.
    let script: Script = {
        let state_calls = AtomicUsize::new(0);
        let playlist_calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            ClientMsg::GetState => {
                if state_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    vec![ScriptStep::Disconnect]
                } else {
                    vec![ScriptStep::Reply(ServerMsg::Ok)]
                }
            }
            ClientMsg::GetPlaylists => {
                if playlist_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    vec![ScriptStep::Reply(ServerMsg::Playlists {
                        playlists: vec![playlist("one"), playlist("two")],
                    })]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        })
    };

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script));
    harness
        .tick_until(
            |rt| rt.sources.playlists.items.len() == 2,
            Duration::from_secs(5),
        )
        .expect("fixture should have loaded two playlists");

    harness.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::GetState,
        task_id: None,
    });
    harness
        .tick_until(
            |rt| rt.sources.session.lost_server.is_some(),
            Duration::from_secs(5),
        )
        .expect("the drop was never observed");
    assert_eq!(
        harness.rt.sources.playlists.items.len(),
        2,
        "fixture: the view should survive the drop itself"
    );

    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected");

    // Let the handshake round-trip.
    let wiped = harness
        .tick_until(
            |rt| rt.sources.playlists.items.is_empty(),
            Duration::from_secs(3),
        )
        .is_ok();

    assert!(
        !wiped,
        "the reconnect handshake wiped the retained playlists — the \
         server's BackendChanged reply to Hello resets playlists / \
         queue / tracks / search, so the screen blanks behind the \
         just-closed modal"
    );
}
