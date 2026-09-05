//! Round-3 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Each test expresses behaviour the brief promises — "reconnect
//! transparently, without interruption to local state" — but that the
//! branch does not deliver at `370a6b8`. Test code only; no production
//! file is touched.

mod common;

use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{ClientMsg, Peer, Runtime, SemanticEvent, Trace};
use mkpclient_state_credentials::PairingEntry;
use mkpclient_state_link::LinkPhase;
use mkpclient_state_pairing::PairingPhase;
use mkproto::{ListTarget, Playlist, QueueEntry, ServerMsg, Song};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, Script, ScriptStep};

struct NoopTrace;
impl Trace for NoopTrace {}

/// A runtime with no server and no mock: the pairing test drives
/// sources directly, so nothing needs to be on the wire.
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

/// Grab a port the OS is willing to hand out, then release it — so the
/// next TCP connect to it is refused until something binds it again.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("local_addr").port()
}

/// An unremarkable server that never hangs up, and answers `Hello`
/// with `BackendChanged` exactly as the real one does
/// (`server/src/session.rs`).
fn plain_script(backend: &'static str) -> Script {
    Box::new(move |msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
            backend: backend.into(),
        })],
        ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
        ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
            playlists: vec![playlist("one"), playlist("two")],
        })],
        _ => vec![],
    })
}

/// Register a second, already-paired server with the runtime the way
/// mDNS + a stored credential would.
fn register(harness: &mut Harness, mock: &MockServer) -> String {
    let name = format!("mock-{}", mock.addr.port());
    let addr_v4 = match mock.addr.ip() {
        std::net::IpAddr::V4(v4) => v4,
        std::net::IpAddr::V6(_) => Ipv4Addr::LOCALHOST,
    };
    harness.rt.sources.discovery.upsert(ServerAd {
        name: name.clone(),
        host: "127.0.0.1".into(),
        addr: addr_v4,
        port: mock.addr.port(),
    });
    harness.rt.sources.probes.set_fingerprint(
        format!("{}:{}", mock.addr.ip(), mock.addr.port()),
        mock.certs.fingerprint.clone(),
    );
    harness.rt.sources.credentials.insert(PairingEntry {
        fingerprint: mock.certs.fingerprint.clone(),
        host: "127.0.0.1".into(),
        server_cert_pem: mock.certs.server_cert_pem.clone(),
        client_cert_pem: mock.certs.client_cert_pem.clone(),
        client_key_pem: mock.certs.client_key_pem.clone(),
    });
    name
}

/// **A pairing session that drops mid-`AwaitingConfirmation` is
/// restarted behind the user's back.**
///
/// Round 1 reported this ("the confirmation code churns under the user
/// mid-`AwaitingConfirmation`") and the answer was the backoff, "fixed
/// by construction rather than by remembering to list
/// `intent.pair_target`". The backoff throttles it; it does not
/// prevent it. `intent.pair_target` survives the close, `desired_link`
/// still answers `Pairing { tower }`, and `Closed` is now a dialable
/// phase — so once `retry_at` lapses `apply_link` opens a *second*
/// TOFU handshake, and the `PairingReady` it produces overwrites
/// `sources.pairing` wholesale with a fresh code while the user is
/// still reading the old one off the server's screen.
///
/// On `main` this could not happen: the link parked on `Closed` and
/// nothing dialled out of it.
///
/// The knock-on is worse than the churn. If the user presses Enter in
/// the window between the close and the redial, `confirm_pair` sees
/// `AwaitingConfirmation`, moves to `Confirming`, and ships
/// `ConfirmPair` into an idle driver that logs and drops it
/// (`driver-link/native-std/src/lib.rs:130`). The redial's own close
/// then reaches `maybe_persist_confirmed_pairing` with the phase left
/// on `Confirming`, and the certificates from a handshake the *server*
/// never confirmed are written to the credential store.
///
/// A dropped pairing session has no state worth resuming — the code is
/// dead the moment the socket is — so the redial has to wait for the
/// user, not fire on a timer.
#[test]
fn a_dropped_pairing_session_is_not_restarted_under_the_user() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut rt = bare_runtime();
    rt.sources.discovery.upsert(ServerAd {
        name: "tower".into(),
        host: "tower.local".into(),
        addr: Ipv4Addr::LOCALHOST,
        port: free_port(),
    });
    rt.sources.intent.pair_target = Some(Arc::from("tower"));
    rt.sources.pairing.phase = PairingPhase::AwaitingConfirmation;
    rt.sources.pairing.code = Some(Arc::from("482913"));

    // Where `ingest`'s `LinkEvent::Closed` leaves a pairing link, with
    // the backoff it arms already lapsed — i.e. half a second later.
    rt.tick();
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.kind = None;
    rt.sources.link.retry_at = Some(rt.sources.clock.now);

    rt.tick();

    assert_ne!(
        rt.sources.link.phase,
        LinkPhase::Connecting,
        "a pairing session that dropped while the user was reading the \
         confirmation code opened a second handshake on its own; the \
         `PairingReady` that answers it replaces `sources.pairing` and \
         the code the user is comparing against"
    );
}

/// **The view retained across a deliberate disconnect follows the user
/// to the next server.**
///
/// Before this PR `ingest`'s `LinkEvent::Closed` wiped every
/// server-derived source. It now keeps them, and the only thing that
/// discards them on the way back up is `backend_action`'s
/// `drop_retained`, which is computed from `session.lost_server`:
///
/// ```ignore
/// let drop_retained = match current.lost_server {
///     Some(lost) => &**lost != name.as_str(),
///     None => false,        // <- a deliberate disconnect leaves None
/// };
/// ```
///
/// `Clear { lost: false }` — the round-2 fix — deliberately leaves
/// `lost_server` unset for a disconnect the caller asked for. So on
/// the next `ConnectTo`, `drop_retained` is `false` and the previous
/// server's queue, track list, search results and artist extras are
/// still in `sources` while connected to somebody else. The handshake
/// does not save it either: `BackendChanged` only invalidates when the
/// *music backend name* changes, and two Macs both running Apple Music
/// report the same one.
///
/// This is the exact sequence the iOS shell drives —
/// `mkp_client_disconnect` then `mkp_client_connect`
/// (`client/runtime-ios/src/lib.rs:278,288`).
#[test]
fn a_deliberate_disconnect_does_not_carry_the_view_to_the_next_server() {
    let _ = env_logger::builder().is_test(true).try_init();

    // Server A serves a two-entry queue on its first GetState.
    let queue_script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            ClientMsg::GetState => {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    vec![
                        ScriptStep::Reply(ServerMsg::Ok),
                        ScriptStep::Broadcast(ServerMsg::ListBegin {
                            target: ListTarget::Queue {
                                queue_id: 42,
                                version: 3,
                            },
                            total: 2,
                            focus: 0,
                        }),
                        ScriptStep::Broadcast(ServerMsg::QueueChunk {
                            queue_id: 42,
                            offset: 0,
                            entries: vec![entry(10, "a-alpha"), entry(11, "a-bravo")],
                        }),
                    ]
                } else {
                    vec![ScriptStep::Reply(ServerMsg::Ok)]
                }
            }
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![playlist("one"), playlist("two")],
            })],
            _ => vec![],
        })
    };

    let mut harness = Harness::connect(MockServer::start(certs::generate(), queue_script));
    harness
        .tick_until(
            |rt| rt.sources.queue.items.len() == 2,
            Duration::from_secs(5),
        )
        .expect("fixture: server A's queue never arrived");

    // The caller asks to leave A.
    harness.dispatch(SemanticEvent::Disconnect);
    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Closed,
            Duration::from_secs(5),
        )
        .expect("the explicit disconnect never landed as Closed");

    // …and then to join B, a different server running the same music
    // backend.
    let mock_b = MockServer::start(certs::generate(), plain_script("MusicKit"));
    let name_b = register(&mut harness, &mock_b);
    harness.dispatch(SemanticEvent::ConnectTo {
        server_name: name_b.clone(),
    });
    harness
        .tick_until(
            |rt| rt.sources.session.backend_name.as_deref() == Some(name_b.as_str()),
            Duration::from_secs(15),
        )
        .expect("never connected to the second server");
    // Let B's handshake round-trip.
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("second server's playlists never loaded");

    assert!(
        harness.rt.sources.queue.items.is_empty(),
        "server A's queue is still on screen while connected to server \
         B: {:?}. A close the caller asked for keeps the retained view \
         (`lost_server` stays None), and `drop_retained` is false for \
         every connect that follows one",
        harness
            .rt
            .sources
            .queue
            .items
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
    );
}

/// **The reconnect resumes the queue mirror at a version it knows it
/// has fallen behind.**
///
/// `state-queue`'s own contract: "when `version` jumps
/// non-contiguously we've missed deltas and should expect a
/// catch-up." Before this PR the close reset `sources.queue`, so
/// `queue_id` was `None` and the first post-reconnect broadcast forced
/// a `Queue::reset`. The close now keeps the whole mirror, **including
/// the `(queue_id, version)` sync cursor** — and nothing resyncs it:
///
/// * `LinkEvent::Connected` queues only `Hello`, `GetState` and
///   `GetPlaylists`;
/// * the server only re-sends the queue when it changes
///   (`QueueBroadcast::Snapshot`), not on accept;
/// * `mkproto::ClientMsg::GetQueueSince { queue_id, version, focus }`
///   exists for exactly this and the server implements it
///   (`server/src/session.rs:358`), but no client code constructs it;
/// * `fold_broadcast`'s `QueueDelta` arm assigns `version`
///   unconditionally and resets only when `queue_id` differs, so the
///   gap is undetectable after the fact.
///
/// So every delta the server broadcast during the outage is lost, and
/// the next one is applied *by index* to a list that has moved on —
/// `Queue::apply` silently no-ops out-of-bounds and otherwise
/// inserts/removes at the wrong row. The server's `queue_id` is a
/// per-process counter from 0 (`server/src/state.rs:87,206`), so a
/// server restart can even reuse the retained id, and `Queue::chunk`
/// overwrites in place without ever truncating.
///
/// Retaining the *rows* is the feature. Retaining the sync cursor that
/// claims those rows are current is the bug.
#[test]
fn a_reconnect_resyncs_the_queue_instead_of_resuming_a_stale_version() {
    let _ = env_logger::builder().is_test(true).try_init();

    // Serves a queue on the first GetState, hangs up on the second,
    // and — like the real server — says nothing about the queue when
    // the client comes back, because nothing changed on *its* side
    // that it knows this client missed.
    let script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            ClientMsg::GetState => match calls.fetch_add(1, Ordering::SeqCst) {
                0 => vec![
                    ScriptStep::Reply(ServerMsg::Ok),
                    ScriptStep::Broadcast(ServerMsg::ListBegin {
                        target: ListTarget::Queue {
                            queue_id: 42,
                            version: 3,
                        },
                        total: 2,
                        focus: 0,
                    }),
                    ScriptStep::Broadcast(ServerMsg::QueueChunk {
                        queue_id: 42,
                        offset: 0,
                        entries: vec![entry(10, "alpha"), entry(11, "bravo")],
                    }),
                ],
                1 => vec![ScriptStep::Disconnect],
                _ => vec![ScriptStep::Reply(ServerMsg::Ok)],
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
            |rt| rt.sources.queue.items.len() == 2,
            Duration::from_secs(5),
        )
        .expect("fixture: the queue never arrived");
    assert_eq!(harness.rt.sources.queue.queue_id, Some(42));
    assert_eq!(harness.rt.sources.queue.version, 3);

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

    // The reconnect, plus a couple of seconds of handshake.
    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected");
    let _ = harness.tick_until(|_| false, Duration::from_secs(2));

    let asked_for_the_gap = harness
        .mock
        .received()
        .iter()
        .any(|m| matches!(m, ClientMsg::GetQueueSince { .. }));
    let mirror_dropped = harness.rt.sources.queue.queue_id.is_none();

    assert!(
        asked_for_the_gap || mirror_dropped,
        "after reconnecting the client still claims queue_id={:?} at \
         version={} without asking the server for the deltas it missed \
         while the link was down. The next QueueDelta is applied by \
         index onto that stale mirror",
        harness.rt.sources.queue.queue_id,
        harness.rt.sources.queue.version,
    );
}
