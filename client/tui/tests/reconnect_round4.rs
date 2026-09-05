//! Round-4 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Test code only; no production file is touched. Each test fails at
//! `235d06b`.

mod common;

use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{ClientMsg, Peer, Runtime, SemanticEvent, Trace};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_pairing::PairingPhase;
use mkproto::{ListTarget, Playlist, QueueDelta, QueueEntry, ServerMsg, Song};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, Script, ScriptStep};

struct NoopTrace;
impl Trace for NoopTrace {}

fn bare_runtime() -> Runtime {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::env::set_var("XDG_CONFIG_HOME", tmp.path());
    Box::leak(Box::new(tmp));

    let trace: Arc<dyn Trace> = Arc::new(NoopTrace);
    mkpclient_runtime_desktop::start_for_test(
        trace,
        Peer {
            user: "test".into(),
            host: "test-host".into(),
        },
    )
}

fn playlist(id: &str) -> Playlist {
    Playlist {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        track_count: 3,
    }
}

fn entry(id: u64, song_id: &str) -> QueueEntry {
    QueueEntry {
        id,
        song: Song {
            id: song_id.into(),
            title: song_id.into(),
            artist_name: String::new(),
            album_title: String::new(),
            duration: 60.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        },
    }
}

fn queue_ids(rt: &Runtime) -> Vec<String> {
    rt.sources
        .queue
        .items
        .iter()
        .map(|s| s.id.clone())
        .collect()
}

/// Grab a port the OS is willing to hand out, then release it — so the
/// next TCP connect to it is refused until something binds it again.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("local_addr").port()
}

/// Exactly what `server/src/session.rs::send_queue_chunked` puts on the
/// wire for a `GetState`: the **stale base snapshot** followed by the
/// **whole delta log**, every frame on seq 0.
///
/// The server deliberately never materialises deltas onto the base
/// between compactions — see the "DO NOT apply deltas to the base
/// state here" comment on `AppState::apply_delta`
/// (`server/src/state.rs:222`): "`send_queue_chunked()` sends the base
/// snapshot + the full log to clients. If you also mutated the base,
/// clients would double-apply every delta."
fn queue_snapshot() -> Vec<ScriptStep> {
    vec![
        ScriptStep::Reply(ServerMsg::Ok),
        ScriptStep::Broadcast(ServerMsg::ListBegin {
            // `send_queue_chunked` hardcodes `version: 0`.
            target: ListTarget::Queue {
                queue_id: 42,
                version: 0,
            },
            total: 2,
            focus: 0,
        }),
        ScriptStep::Broadcast(ServerMsg::QueueChunk {
            queue_id: 42,
            offset: 0,
            entries: vec![entry(10, "alpha"), entry(11, "bravo")],
        }),
        ScriptStep::Broadcast(ServerMsg::QueueCatchUp {
            queue_id: 42,
            deltas: vec![(
                1,
                QueueDelta::Insert {
                    index: 2,
                    entry: entry(12, "charlie"),
                },
            )],
        }),
    ]
}

/// **The reconnect handshake double-applies the server's delta log
/// onto the retained queue.**
///
/// `ingest`'s `LinkEvent::Closed` used to reset `sources.queue`; this
/// PR keeps it, `queue_id` and all. `Queue::reset` is the only thing
/// that empties `items`, and `fold_broadcast` calls it solely when the
/// incoming `queue_id` *differs* — which, on a reconnect to the same
/// server, it does not.
///
/// So the reconnect's `GetState` lands base-snapshot chunks on top of
/// a mirror that already has the log applied: `Queue::chunk`
/// overwrites in place and never truncates, and the `QueueCatchUp`
/// that follows replays every delta a second time. The queue on
/// screen — the thing the CHANGELOG promises "stays on screen" — is
/// then wrong, and it is wrong in the ordinary case, not an edge one:
/// any queue with a non-empty server-side log.
///
/// `GetQueueSince` does not save it. It is sent *after* `GetState` in
/// the same handshake, so the snapshot has already landed by the time
/// any answer arrives.
///
/// The mechanism cannot exist on `main` — the close emptied the
/// mirror, so the first frame back forced a `Queue::reset` — but the
/// test cannot be *run* there either: `main` never reconnects, which is
/// the defect this PR fixes. It fails on `main` at "the runtime never
/// reconnected".
#[test]
fn a_reconnect_does_not_double_apply_the_servers_queue_log() {
    let _ = env_logger::builder().is_test(true).try_init();

    let script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            // Call 0 is the harness's connect handshake, call 1 is the
            // test asking for the drop, call 2 is the reconnect's own
            // handshake — which the real server answers exactly as it
            // answered the first one.
            ClientMsg::GetState => match calls.fetch_add(1, Ordering::SeqCst) {
                1 => vec![ScriptStep::Disconnect],
                _ => queue_snapshot(),
            },
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![playlist("one"), playlist("two")],
            })],
            _ => vec![],
        })
    };

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script));
    harness
        .tick_until(
            |rt| rt.sources.queue.items.len() == 3,
            Duration::from_secs(5),
        )
        .expect("fixture: base snapshot + log should give three entries");
    assert_eq!(queue_ids(&harness.rt), ["alpha", "bravo", "charlie"]);

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

    // The reconnect, plus time for its handshake to round-trip.
    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected");
    let _ = harness.tick_until(|_| false, Duration::from_secs(2));

    assert_eq!(
        queue_ids(&harness.rt),
        ["alpha", "bravo", "charlie"],
        "the reconnect handshake corrupted the retained queue: the \
         server's base snapshot was written over a mirror that already \
         had the delta log applied, and the log was then replayed on \
         top of it"
    );
}

/// **A pairing handshake that drops leaves the client unable to pair
/// with anything, ever again.**
///
/// `235d06b` added, to `link_action`'s `Pairing` arm:
///
/// ```ignore
/// if pairing.handshake_in_flight && !link.phase_connected {
///     return LinkAction::Noop;
/// }
/// ```
///
/// Refusing to *auto*-redial a dead handshake is right. But
/// `sources.pairing.phase` is never cleared on a close: `ingest`'s
/// `LinkEvent::Closed` resets it only via
/// `maybe_persist_confirmed_pairing`, which returns early unless the
/// phase is `Confirming`. So `AwaitingResponse` — where a handshake
/// sits between `LinkEvent::Connected` and `PairingReady` — survives
/// the drop for the life of the process, and with it
/// `handshake_in_flight`.
///
/// The gate has no exemption for a dial the *user* asked for. The
/// picker's Enter (`picker_connect` → `connect_to`) writes
/// `intent.target`, `apply_link`'s `fallback_target_to_pair` swings it
/// to `intent.pair_target` because there are no credentials, and
/// `link_action` answers `Noop` — for that server and every other
/// unpaired one. Nor is there a key that clears the phase:
/// `RejectPair` is reachable only while `AwaitingConfirmation`
/// (`input.rs:124`).
///
/// The driver makes this the *normal* failure: it never emits
/// `PairFailed`, only `LinkEvent::Closed`
/// (`driver-link/native-std/src/lib.rs`), so any server that hangs up
/// mid-handshake lands here.
///
/// On `main` the retry worked — `begin_pair`/`connect_to` called
/// `ack_closed` and the next `apply_link` dialled.
#[test]
fn pairing_can_be_retried_after_a_handshake_drops() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut rt = bare_runtime();
    let port = free_port();
    rt.sources.discovery.upsert(ServerAd {
        name: "tower".into(),
        host: "tower.local".into(),
        addr: Ipv4Addr::LOCALHOST,
        port,
    });
    // Probed, unpaired: the shape that routes the picker's Enter into
    // a pairing handshake via `fallback_target_to_pair`.
    rt.sources
        .probes
        .set_fingerprint(format!("127.0.0.1:{port}"), "unknown-fp".into());

    // Where a handshake that dropped before `PairingReady` leaves the
    // sources: the phase the server never moved off, and a link
    // resting on `Closed`.
    rt.sources.pairing.phase = PairingPhase::AwaitingResponse;
    rt.sources.intent.pair_target = Some(Arc::from("tower"));
    rt.tick();
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.kind = None;

    // The user goes back to the picker and asks for it again. No
    // backoff is in play — `connect_to` clears it.
    rt.dispatch(SemanticEvent::ConnectTo {
        server_name: "tower".into(),
    });
    rt.tick();

    assert_eq!(
        rt.sources.link.phase,
        LinkPhase::Connecting,
        "the user asked to pair again and nothing dialled. \
         `pairing.phase` is still {:?} from the handshake that dropped, \
         `handshake_in_flight` is therefore permanently true, and no key \
         in the TUI can clear it",
        rt.sources.pairing.phase,
    );
}
