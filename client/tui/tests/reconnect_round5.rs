//! Round-5 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Test code only; no production file is touched.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_runtime::{ClientMsg, SemanticEvent};
use mkpclient_state_link::LinkPhase;
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

/// **The reconnect re-runs the startup view restore, so the user is
/// put back where the *disk* says they were, not where they are.**
///
/// `lifecycle::backend`'s `Clear` arm sets
/// `session.auto_restored_view = false` on every close. On `main`
/// that was inert — the link never came back — but this PR makes it
/// come back, and `desired_restore` is gated on exactly that flag.
/// So the reconnect replays startup: `apply_restore` loads the
/// persisted view and dispatches `RestoreSavedPlaylist`, which calls
/// `view_playlist` (`playlist_tracks.clear()` + a fresh
/// `GetPlaylist`) and then writes `cursor.middle = selected` from
/// disk.
///
/// The persisted `selected` is stale by design: `view_persist`'s diff
/// is mode-only, so j/k presses are deliberately never saved
/// (`lifecycle/view_persist.rs` module doc). Whatever row the user
/// scrolled to since the last *navigation* is therefore thrown away
/// by the reconnect — along with the track list this PR's CHANGELOG
/// promises "stays on screen", which `view_playlist` clears and
/// re-streams a tick later.
///
/// This is the interruption to local state the branch exists to
/// remove, arriving through a different door.
#[test]
fn a_reconnect_does_not_re_run_the_startup_view_restore() {
    let _ = env_logger::builder().is_test(true).try_init();

    let script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::BackendChanged {
                backend: "MusicKit".into(),
            })],
            // Call 0 is the connect handshake; call 1 is the test
            // asking for the drop; call 2 is the reconnect's own.
            ClientMsg::GetState => match calls.fetch_add(1, Ordering::SeqCst) {
                1 => vec![ScriptStep::Disconnect],
                _ => vec![ScriptStep::Reply(ServerMsg::Ok)],
            },
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![playlist("p1")],
            })],
            ClientMsg::GetPlaylist { .. } => tracks(),
            _ => vec![],
        })
    };

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script));
    // Startup restore has nothing on disk, so it opens the first
    // playlist and persists `selected: 0` with it.
    harness
        .tick_until(
            |rt| rt.sources.playlist_tracks.songs.len() == 3,
            Duration::from_secs(5),
        )
        .expect("fixture: the first playlist never streamed");
    let _ = harness.tick_until(|_| false, Duration::from_secs(1));

    // The user scrolls to the last track. A cursor move is
    // deliberately not persisted.
    harness.rt.sources.cursor.middle = 2;
    harness.rt.tick();
    assert_eq!(harness.rt.sources.cursor.middle, 2, "fixture");

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

    harness
        .tick_until(
            |rt| rt.sources.link.phase == LinkPhase::Connected,
            Duration::from_secs(15),
        )
        .expect("the runtime never reconnected");
    let _ = harness.tick_until(|_| false, Duration::from_secs(3));

    assert_eq!(
        harness.rt.sources.cursor.middle, 2,
        "the reconnect moved the user's cursor back to the row the \
         persisted view names. `BackendAction::Clear` re-arms \
         `auto_restored_view = false`, so `apply_restore` replays the \
         startup restore on the way back up: it re-issues \
         `GetPlaylist` (clearing the retained track list) and \
         overwrites `cursor.middle` from disk"
    );
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

    assert!(
        harness.rt.sources.search.first_page_received,
        "the search that was in flight when the link dropped is stranded \
         mid-stream: `first_page_received` is still false, so the middle \
         pane renders `SearchResultsState::Searching` forever. The close \
         keeps `sources.search` but clears only `task_id`, and nothing \
         re-issues the request"
    );
}
