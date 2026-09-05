//! A backend swap has to restart the session, not just tear it down.
//!
//! The server announces a swap with a seq-0 `BackendChanged`, having
//! already dropped its play state, queue and caches. The client
//! mirrors that by dropping everything derived from the old backend
//! — and then has to ask for the new one's world, or it sits on an
//! empty playlist column with a spinner that never stops.

mod common;

use std::time::Duration;

use common::certs;
use common::harness::Harness;
use common::mock_server;
use common::mock_server::{MockServer, ScriptStep};
use mkpclient_runtime::SemanticEvent;
use mkpclient_state_ui_history::MiddleMode;
use mkproto::{ClientMsg, Playlist, ServerMsg};

fn playlist(id: &str, track_count: usize) -> Playlist {
    Playlist {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        track_count,
    }
}

/// Playlists named per backend so the assertions can tell which
/// catalogue the client is showing.
fn playlists_for(backend: &str) -> Vec<Playlist> {
    match backend {
        "tidal" => vec![playlist("tidal-1", 0), playlist("tidal-2", 0)],
        _ => vec![playlist("mk-1", 0)],
    }
}

/// A mock whose catalogue flips when the "user" switches backend.
/// `Ping` stands in for the menu-bar action on the server side: it
/// answers with the same seq-0 broadcast a real swap emits.
fn switching_mock() -> MockServer {
    let backend = std::sync::Mutex::new("musickit".to_string());
    MockServer::start(
        certs::generate(),
        Box::new(move |msg| {
            let current = backend.lock().unwrap().clone();
            match msg {
                ClientMsg::Hello { .. } => mock_server::hello_reply_from(&current),
                ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
                ClientMsg::GetPlaylists => vec![
                    ScriptStep::Reply(ServerMsg::Playlists {
                        playlists: playlists_for(&current),
                    }),
                    ScriptStep::BroadcastWithTask {
                        task_id: 1,
                        msg: ServerMsg::PlaylistTrackCount {
                            playlist_id: playlists_for(&current)[0].id.clone(),
                            track_count: 42,
                        },
                    },
                ],
                ClientMsg::Ping => {
                    *backend.lock().unwrap() = "tidal".to_string();
                    vec![ScriptStep::Broadcast(ServerMsg::BackendChanged {
                        backend: "tidal".into(),
                    })]
                }
                _ => vec![],
            }
        }),
    )
}

/// The connect-time handshake is itself a `BackendChanged`, so the
/// bookkeeping for the in-flight `GetPlaylists` has to survive it.
/// It used to be issued *before* the reply landed, and the reply
/// wiped it — dropping the streamed counts on the floor.
#[test]
fn connect_handshake_keeps_the_streamed_track_counts() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut h = Harness::connect(switching_mock());
    h.tick_until(
        |rt| {
            rt.sources
                .playlists
                .items
                .iter()
                .any(|p| p.id == "mk-1" && p.track_count == 42)
        },
        Duration::from_secs(3),
    )
    .expect("streamed track count to land on the connect-time playlist list");

    assert_eq!(
        h.rt.sources.server.backend.as_deref(),
        Some("musickit"),
        "handshake should record the backend the server reported"
    );
}

#[test]
fn switching_backend_reloads_the_new_catalogue() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut h = Harness::connect(switching_mock());
    h.tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(3))
        .expect("initial playlists to load");
    assert_eq!(h.rt.sources.playlists.items.len(), 1);

    // Drill into something that only exists on the old backend, so
    // the reset has stale navigation to clear.
    h.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::Ping,
        task_id: None,
    });

    h.tick_until(
        |rt| {
            rt.sources.playlists.loaded
                && rt.sources.playlists.items.iter().any(|p| p.id == "tidal-1")
        },
        Duration::from_secs(3),
    )
    .expect("client to refetch playlists after the backend swap");

    let p = &h.rt.sources.playlists;
    assert_eq!(p.items.len(), 2, "new backend's catalogue, not the old one");
    assert!(
        p.items.iter().all(|p| p.id.starts_with("tidal-")),
        "no playlist from the previous backend may survive the swap"
    );
    assert_eq!(h.rt.sources.server.backend.as_deref(), Some("tidal"));
}

#[test]
fn switching_backend_clears_navigation_into_the_old_catalogue() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut h = Harness::connect(switching_mock());
    h.tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(3))
        .expect("initial playlists to load");

    // Park the middle pane on an album that belongs to the outgoing
    // backend; its id is meaningless to the new one.
    h.rt.sources.history.mode = MiddleMode::AlbumDetail {
        album_id: "mk-album".into(),
        album_title: "Old".into(),
        awaiting_seq: None,
    };

    h.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::Ping,
        task_id: None,
    });
    h.tick_until(
        |rt| rt.sources.server.backend.as_deref() == Some("tidal"),
        Duration::from_secs(3),
    )
    .expect("backend swap to be observed");

    assert_eq!(
        h.rt.sources.history.mode,
        MiddleMode::PlaylistSongs,
        "an album id from the previous backend must not survive the swap"
    );
    assert!(h.rt.sources.playlist_tracks.playlist_id.is_none());
    assert!(h.rt.sources.queue.queue_id.is_none());
}
