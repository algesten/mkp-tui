//! End-to-end: the initial playlist list becomes usable immediately,
//! then task-scoped count continuations enrich it in place.

mod common;

use std::time::Duration;

use mkpclient_runtime::ClientMsg;
use mkproto::{Playlist, ServerMsg};

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

fn playlist(id: &str, count: usize) -> Playlist {
    Playlist {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        track_count: count,
    }
}

#[test]
fn playlist_counts_stream_into_the_initial_list() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mock = MockServer::start(
        certs::generate(),
        Box::new(|msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => vec![
                ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![playlist("one", 0), playlist("two", 0)],
                }),
                ScriptStep::BroadcastWithTask {
                    task_id: 999,
                    msg: ServerMsg::PlaylistTrackCount {
                        playlist_id: "one".into(),
                        track_count: 99,
                    },
                },
                ScriptStep::BroadcastWithTask {
                    task_id: 1,
                    msg: ServerMsg::PlaylistTrackCount {
                        playlist_id: "two".into(),
                        track_count: 12,
                    },
                },
                ScriptStep::BroadcastWithTask {
                    task_id: 1,
                    msg: ServerMsg::TaskCompleted { task_id: 1 },
                },
            ],
            _ => vec![],
        }),
    );

    let mut harness = Harness::connect(mock);
    harness
        .tick_until(
            |runtime| {
                runtime.sources.playlists.loaded
                    && runtime.sources.playlists.pending_task.is_none()
                    && runtime
                        .sources
                        .playlists
                        .items
                        .iter()
                        .any(|playlist| playlist.id == "two" && playlist.track_count == 12)
            },
            Duration::from_secs(3),
        )
        .expect("playlist count stream to complete");

    assert_eq!(harness.rt.sources.playlists.items[0].track_count, 0);
    assert_eq!(harness.rt.sources.playlists.items[1].track_count, 12);
}
