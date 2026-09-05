//! Round-6 adversarial regression cover for the reconnect work.
//!
//! Test code only; no production file is touched.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mkpclient_runtime::{ClientMsg, SemanticEvent};
use mkpclient_state_ui_history::MiddleMode;
use mkproto::{Playlist, SearchResults, SearchType, ServerMsg, Song};

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

/// **A server that accepts the TLS connection and then hangs up is
/// redialled twice a second, for ever, with no widening backoff.**
///
/// `RETRY_BACKOFF` only widens across *consecutive* failures, and
/// `ingest`'s `LinkEvent::Connected` arm calls `link.clear_retry()`
/// — "reset on a successful connect". But `Connected` fires when the
/// TLS handshake completes, which is long before the session has
/// proved itself. A server that is reachable but not usable (the app
/// wedged, MusicKit unauthorised, a protocol version it refuses)
/// completes the handshake and then drops the socket, so every cycle
/// is scored as a success followed by a first failure:
///
///   Connected -> clear_retry (attempts = 0)
///   Closed    -> schedule_retry (RETRY_BACKOFF[0] = 500 ms)
///
/// The delay never leaves the bottom of the table. The result is a
/// permanent 2 Hz loop of TLS handshakes — and, because
/// `apply_backend` runs `Set` then `Clear` on each pass, two persist
/// driver commands and a lost-modal open/close on every one of them.
/// That is precisely the spin the backoff exists to prevent.
///
/// On `main` this could not happen: the link parked on
/// `LinkPhase::Closed` and never dialled again.
///
/// The mock below hangs up the moment it decodes the handshake's
/// `Hello`, so each `Hello` it records is one connection attempt.
/// A backoff that widens produces four attempts inside the five
/// second window (0, 0.5, 1.5, 3.5); a flat 500 ms one produces
/// eleven.
#[test]
fn a_server_that_hangs_up_on_hello_is_not_redialled_twice_a_second() {
    let _ = env_logger::builder().is_test(true).try_init();

    let script: Script = Box::new(|msg| match msg {
        ClientMsg::Hello { .. } => vec![ScriptStep::Disconnect],
        _ => vec![],
    });

    let mut harness = Harness::connect(MockServer::start(certs::generate(), script));

    // Let the runtime redial for five seconds without touching it.
    let _ = harness.tick_until(|_| false, Duration::from_secs(5));

    let attempts = harness
        .mock
        .received()
        .iter()
        .filter(|m| matches!(m, ClientMsg::Hello { .. }))
        .count();

    assert!(
        attempts <= 6,
        "the client opened {attempts} connections in five seconds against a \
         server that hangs up on every one. `clear_retry()` on \
         `LinkEvent::Connected` resets the schedule before the session has \
         proved itself, so the backoff never leaves RETRY_BACKOFF[0] and \
         the reconnect degrades into a 2 Hz handshake loop"
    );
}

/// **The middle pane must not survive the drop still claiming a mode
/// whose data went with the socket.**
///
/// This is what `ingest`'s `sources.history = Default::default()`
/// buys, and nothing in the suite pins it: delete that line and all
/// 213 tests stay green. The round-5 test that was retargeted onto
/// this invariant
/// (`reconnect_round5::a_search_interrupted_by_the_drop_does_not_spin_forever`)
/// asserts only *after* the reconnect, by which point `apply_restore`
/// has re-issued the search and answered it — so it passes with the
/// history wipe removed and measures the restore, not the wipe.
///
/// Asserted at the drop instead, where the reconnect cannot yet have
/// masked it: the backoff withholds the redial for 500 ms, and the
/// close happens in the same tick that sets `lost_server`.
#[test]
fn the_drop_leaves_no_pane_claiming_a_mode_whose_data_is_gone() {
    let _ = env_logger::builder().is_test(true).try_init();

    let script: Script = {
        let calls = AtomicUsize::new(0);
        Box::new(move |msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => vec![ScriptStep::Reply(ServerMsg::Playlists {
                playlists: vec![playlist("p1")],
            })],
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
        !matches!(
            harness.rt.sources.history.mode,
            MiddleMode::SearchResults { .. }
        ),
        "the close left `history.mode` on SearchResults while \
         `sources.search` was cleared, so the middle pane renders \
         `Searching…` behind the reconnect modal against a request that \
         died with the socket. Got {:?}",
        harness.rt.sources.history.mode
    );
}
