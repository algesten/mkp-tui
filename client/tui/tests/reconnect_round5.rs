//! Round-5 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Test code only; no production file is touched.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_runtime::{ClientMsg, SemanticEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_history::MiddleMode;
use mkproto::{ListTarget, Playlist, SearchResults, SearchType, ServerMsg, Song};

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

fn song(id: &str) -> Song {
    Song {
        id: id.into(),
        title: id.into(),
        artist_name: String::new(),
        album_title: String::new(),
        duration: 60.0,
        track_number: None,
        url: None,
        artwork_url_small: None,
        artwork_url_large: None,
    }
}

/// The tracks of `p1`, streamed the way the server streams them.
fn tracks() -> Vec<ScriptStep> {
    vec![
        ScriptStep::Broadcast(ServerMsg::ListBegin {
            target: ListTarget::Playlist { id: "p1".into() },
            total: 3,
            focus: 0,
        }),
        ScriptStep::Broadcast(ServerMsg::ListChunk {
            target: ListTarget::Playlist { id: "p1".into() },
            offset: 0,
            songs: vec![song("t1"), song("t2"), song("t3")],
        }),
    ]
}

/// **A streamed load that the drop interrupts is never re-issued.**
///
/// The close keeps `sources.search` and clears only `search.task_id`
/// (`ingest.rs`, `LinkEvent::Closed`), on the reasoning that "the rows
/// they describe stay on screen; only the waiting stops". But the
/// search pane's spinner is not driven by `task_id`: `search_results`
/// returns `SearchResultsState::Searching` whenever
/// `first_page_received` is false, and the middle header prints the
/// bare "Search: <term>" for the same reason. The reply that would
/// have set it died with the socket, `sources.responses.clear()`
/// removes a seq that never arrived, and nothing re-sends the
/// `Search` — no `stale` flag, no refetch lifecycle, no key. The pane
/// says "Searching…" for the life of the process.
///
/// `main` cleared `sources.search` on close, so this cannot happen
/// there.
///
/// It is the same class of problem the round-4 fix solved for the
/// queue by resetting it, and it is the reason the retained set needs
/// deciding as a whole: `playlist_tracks` has the identical hole (a
/// half-filled `songs` with `pending_task` nulled reads as
/// `is_ready()` with `stale == false`, so
/// `playlist_tracks_refetch_action` answers `Noop` forever and the
/// missing rows render as `PlaylistTrackRow::Pending`). Today that
/// one is masked by the startup restore replaying on reconnect — the
/// defect the test above covers — so fixing that one uncovers this
/// one.
#[test]
fn a_search_interrupted_by_the_drop_does_not_spin_forever() {
    let _ = env_logger::builder().is_test(true).try_init();

    let script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![playlist("p1")],
            })],
            ClientMsg::GetPlaylist { .. } => tracks(),
            // The first search never gets its first page: the socket
            // dies with the reply still owed.
            ClientMsg::Search { .. } => match calls.fetch_add(1, Ordering::SeqCst) {
                0 => vec![ScriptStep::Disconnect],
                _ => vec![ScriptStep::Reply(ServerMsg::Search(SearchResults::Songs {
                    songs: vec![song("hit")],
                }))],
            },
            _ => vec![],
        })
    };

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script));
    harness
        .tick_until(|rt| rt.sources.playlists.loaded, Duration::from_secs(5))
        .expect("fixture: playlists never loaded");

    harness.dispatch(SemanticEvent::RestoreSavedSearch {
        query: "kraftwerk".into(),
        search_type: SearchType::Song,
        selected: 0,
        selected_id: None,
    });
    harness
        .tick_until(
            |rt| rt.sources.session.lost_server.is_some(),
            Duration::from_secs(5),
        )
        .expect("the drop was never observed");
    assert!(
        !harness.rt.sources.search.first_page_received,
        "fixture: the search should still be awaiting its first page"
    );

    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected");
    let _ = harness.tick_until(|_| false, Duration::from_secs(3));

    // The pane must not be left waiting on a reply that can never
    // arrive. The mirrored state is dropped on close and the view is
    // rebuilt by the restore that runs on every connect, so what has
    // to hold is that nothing is still claiming to be mid-search.
    let stranded = matches!(
        harness.rt.sources.history.mode,
        MiddleMode::SearchResults { .. }
    ) && !harness.rt.sources.search.first_page_received;
    assert!(
        !stranded,
        "the middle pane is still on SearchResults with no first page, \
         so it renders `Searching…` against a request that died with \
         the socket and is never re-issued"
    );
}
