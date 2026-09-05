//! End-to-end: client connects to the mock, issues a `Search`,
//! and verifies the streaming reduction folds first-page +
//! SearchMore broadcasts into `sources.search`.

mod common;

use std::time::Duration;

use mkpclient_runtime::{ClientMsg, SemanticEvent};
use mkproto::{Album, SearchResults, SearchType, ServerMsg, Song};

use common::certs;
use common::harness::Harness;
use common::mock_server;
use common::mock_server::{MockServer, ScriptStep};

fn song(id: &str, title: &str) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        artist_name: "test-artist".into(),
        album_title: "test-album".into(),
        duration: 180.0,
        track_number: None,
        url: None,
        artwork_url_small: None,
        artwork_url_large: None,
    }
}

#[test]
fn search_streams_first_page_then_appends_more() {
    let _ = env_logger::builder().is_test(true).try_init();

    let certs = certs::generate();
    let mock = MockServer::start(
        certs,
        Box::new(|msg| match msg {
            ClientMsg::Hello { .. } => mock_server::hello_reply(),
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => {
                vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })]
            }
            ClientMsg::Search { .. } => vec![
                ScriptStep::Reply(ServerMsg::Search(SearchResults::Songs {
                    songs: vec![song("a", "Alpha"), song("b", "Bravo")],
                })),
                ScriptStep::BroadcastWithTask {
                    task_id: 1,
                    msg: ServerMsg::SearchMore(SearchResults::Songs {
                        songs: vec![song("c", "Charlie")],
                    }),
                },
                ScriptStep::BroadcastWithTask {
                    task_id: 1,
                    msg: ServerMsg::SearchMore(SearchResults::Songs {
                        songs: vec![song("d", "Delta"), song("e", "Echo")],
                    }),
                },
                ScriptStep::Broadcast(ServerMsg::TaskCompleted { task_id: 1 }),
            ],
            _ => vec![],
        }),
    );

    let mut h = Harness::connect(mock);

    // Begin a search with task_id = 1 so the mock's broadcasts
    // correlate properly.
    h.rt.sources.search.begin(1, "x".into(), SearchType::Song);
    h.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::Search {
            term: "x".into(),
            search_type: SearchType::Song,
        },
        task_id: Some(1),
    });

    h.tick_until(
        |rt| rt.sources.search.completed && rt.sources.search.songs.len() == 5,
        Duration::from_secs(3),
    )
    .expect("search to complete with 5 results");

    let titles: Vec<&str> =
        h.rt.sources
            .search
            .songs
            .iter()
            .map(|s| s.title.as_str())
            .collect();
    assert_eq!(titles, ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]);
    assert!(h.rt.sources.search.completed);
    assert!(h.rt.sources.search.first_page_received);
}

#[test]
fn search_more_with_wrong_task_id_is_ignored() {
    let _ = env_logger::builder().is_test(true).try_init();

    let certs = certs::generate();
    let mock = MockServer::start(
        certs,
        Box::new(|msg| match msg {
            ClientMsg::Hello { .. } => mock_server::hello_reply(),
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => {
                vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })]
            }
            ClientMsg::Search { .. } => vec![
                ScriptStep::Reply(ServerMsg::Search(SearchResults::Albums {
                    albums: vec![Album {
                        id: "alb-1".into(),
                        name: "Album 1".into(),
                        artist_id: "art-1".into(),
                        artist_name: "Artist".into(),
                        track_count: 5,
                        detail: None,
                        url: None,
                        artwork_url_small: None,
                        artwork_url_large: None,
                    }],
                })),
                // Stale page from a different task — must be dropped.
                ScriptStep::BroadcastWithTask {
                    task_id: 999,
                    msg: ServerMsg::SearchMore(SearchResults::Albums {
                        albums: vec![Album {
                            id: "stale".into(),
                            name: "Stale".into(),
                            artist_id: String::new(),
                            artist_name: String::new(),
                            track_count: 0,
                            detail: None,
                            url: None,
                            artwork_url_small: None,
                            artwork_url_large: None,
                        }],
                    }),
                },
                ScriptStep::Broadcast(ServerMsg::TaskCompleted { task_id: 1 }),
            ],
            _ => vec![],
        }),
    );

    let mut h = Harness::connect(mock);

    h.rt.sources.search.begin(1, "x".into(), SearchType::Album);
    h.dispatch(SemanticEvent::SendRequest {
        msg: ClientMsg::Search {
            term: "x".into(),
            search_type: SearchType::Album,
        },
        task_id: Some(1),
    });

    h.tick_until(|rt| rt.sources.search.completed, Duration::from_secs(3))
        .expect("task to complete");

    assert_eq!(
        h.rt.sources.search.albums.len(),
        1,
        "stale page must be dropped"
    );
    assert_eq!(h.rt.sources.search.albums[0].name, "Album 1");
}
