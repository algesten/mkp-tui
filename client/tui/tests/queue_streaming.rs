//! End-to-end: server streams a queue via ListBegin / QueueChunk
//! and QueueDelta, and `sources.queue` ends up with the expected
//! contents.

mod common;

use std::time::Duration;

use mkpclient_runtime::{ClientMsg, SemanticEvent, TuiCursorEvent};
use mkproto::{ListTarget, QueueDelta, QueueEntry, ServerMsg, Song};

use common::certs;
use common::harness::Harness;
use common::mock_server;
use common::mock_server::{MockServer, ScriptStep};

fn song(id: &str, title: &str) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        artist_name: String::new(),
        album_title: String::new(),
        duration: 60.0,
        track_number: None,
        url: None,
        artwork_url_small: None,
        artwork_url_large: None,
    }
}

fn entry(id: u64, song_id: &str, title: &str) -> QueueEntry {
    QueueEntry {
        id,
        song: song(song_id, title),
    }
}

#[test]
fn queue_assembles_from_streamed_chunks() {
    let _ = env_logger::builder().is_test(true).try_init();

    let entries1 = vec![entry(10, "a", "Alpha"), entry(11, "b", "Bravo")];
    let entries2 = vec![
        entry(12, "c", "Charlie"),
        entry(13, "d", "Delta"),
        entry(14, "e", "Echo"),
    ];
    let target = ListTarget::Queue {
        queue_id: 42,
        version: 0,
    };

    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => mock_server::hello_reply(),
            ClientMsg::GetState => vec![
                ScriptStep::Reply(ServerMsg::Ok),
                ScriptStep::Broadcast(ServerMsg::ListBegin {
                    target: target.clone(),
                    total: 5,
                    focus: 0,
                }),
                ScriptStep::Broadcast(ServerMsg::QueueChunk {
                    queue_id: 42,
                    offset: 0,
                    entries: entries1.clone(),
                }),
                ScriptStep::Broadcast(ServerMsg::QueueChunk {
                    queue_id: 42,
                    offset: 2,
                    entries: entries2.clone(),
                }),
                ScriptStep::Broadcast(ServerMsg::QueueDelta {
                    queue_id: 42,
                    version: 1,
                    delta: QueueDelta::SetIndex { index: Some(1) },
                }),
            ],
            ClientMsg::GetPlaylists => {
                vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })]
            }
            _ => vec![],
        }),
    );

    let mut h = Harness::connect(mock);

    h.tick_until(
        |rt| rt.sources.queue.items.len() == 5 && rt.sources.queue.current_index == Some(1),
        Duration::from_secs(3),
    )
    .expect("queue to fill to 5 items");

    let titles: Vec<&str> =
        h.rt.sources
            .queue
            .items
            .iter()
            .map(|s| s.title.as_str())
            .collect();
    assert_eq!(titles, ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]);
    assert_eq!(
        h.rt.sources
            .queue
            .entry_ids
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [10, 11, 12, 13, 14]
    );
    assert_eq!(h.rt.sources.queue.queue_id, Some(42));
    assert_eq!(h.rt.sources.queue.version, 1);

    h.rt.sources.cursor.queue = 3;
    h.dispatch(TuiCursorEvent::QueueActivate);
    for _ in 0..10 {
        h.tick_once();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.mock.received().iter().any(|message| matches!(
        message,
        ClientMsg::SkipToQueueEntry {
            queue_id: 42,
            entry_id: 13,
        }
    )));
}

#[test]
fn queue_delta_moves_current_index() {
    let _ = env_logger::builder().is_test(true).try_init();

    let target = ListTarget::Queue {
        queue_id: 7,
        version: 0,
    };

    let mock = MockServer::start(
        certs::generate(),
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => mock_server::hello_reply(),
            ClientMsg::GetState => vec![
                ScriptStep::Reply(ServerMsg::Ok),
                ScriptStep::Broadcast(ServerMsg::ListBegin {
                    target: target.clone(),
                    total: 3,
                    focus: 0,
                }),
                ScriptStep::Broadcast(ServerMsg::QueueChunk {
                    queue_id: 7,
                    offset: 0,
                    entries: vec![entry(0, "a", "A"), entry(1, "b", "B"), entry(2, "c", "C")],
                }),
            ],
            ClientMsg::GetPlaylists => {
                vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })]
            }
            ClientMsg::Skip => vec![ScriptStep::Broadcast(ServerMsg::QueueDelta {
                queue_id: 7,
                version: 2,
                delta: QueueDelta::SetIndex { index: Some(2) },
            })],
            _ => vec![],
        }),
    );

    let mut h = Harness::connect(mock);

    h.tick_until(
        |rt| rt.sources.queue.items.len() == 3,
        Duration::from_secs(3),
    )
    .expect("queue fill");

    h.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::Skip,
        task_id: None,
    });

    h.tick_until(
        |rt| rt.sources.queue.current_index == Some(2),
        Duration::from_secs(2),
    )
    .expect("queue to move to index 2");
}
