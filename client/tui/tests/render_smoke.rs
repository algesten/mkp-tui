//! Render the TUI against a ratatui `TestBackend` and assert on
//! the in-memory buffer. Catches regressions in title bars, list
//! rendering, and modal placement without spinning a terminal.

mod common;

use std::time::Duration;

use mkpclient_runtime::ClientMsg;
use mkpclient_state_ui_cursor::ColumnFocus;
use mkpclient_state_ui_history::MiddleMode;
use mkpclient_tui::app::AppState;
use mkproto::{SearchResults, SearchType, ServerMsg, Song};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use common::certs;
use common::harness::Harness;
use common::mock_server::{MockServer, ScriptStep};

fn song(id: &str, title: &str) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        artist_name: "test-artist".into(),
        album_title: "test-album".into(),
        duration: 60.0,
        track_number: None,
        url: None,
        artwork_url_small: None,
        artwork_url_large: None,
    }
}

/// Helper: collect every cell on row `row` of a TestBackend buffer.
fn row_text(terminal: &Terminal<TestBackend>, row: u16) -> String {
    let buf = terminal.backend().buffer();
    (0..buf.area.width)
        .map(|x| buf[(x, row)].symbol().to_string())
        .collect::<Vec<_>>()
        .join("")
}

/// Helper: does the buffer contain `needle` anywhere?
fn buffer_contains(terminal: &Terminal<TestBackend>, needle: &str) -> bool {
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if row.contains(needle) {
            return true;
        }
    }
    false
}

#[test]
fn search_results_render_against_test_backend() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mock = MockServer::start(
        certs::generate(),
        Box::new(|msg| match msg {
            ClientMsg::Hello { .. } => vec![ScriptStep::Reply(ServerMsg::Pong)],
            ClientMsg::GetState => vec![ScriptStep::Reply(ServerMsg::Ok)],
            ClientMsg::GetPlaylists => {
                vec![ScriptStep::Reply(ServerMsg::Playlists {
                    playlists: vec![],
                })]
            }
            _ => vec![],
        }),
    );

    let mut h = Harness::connect(mock);

    // Set up a search-results view directly.
    h.rt.sources
        .search
        .begin(1, "love".into(), SearchType::Song);
    h.rt.sources.search.set_first_page(
        1,
        SearchResults::Songs {
            songs: vec![song("a", "Alpha"), song("b", "Bravo"), song("c", "Charlie")],
        },
    );
    h.rt.sources.search.mark_completed(1);

    let app = AppState::default();
    h.rt.sources.history.mode = MiddleMode::SearchResults {
        term: "love".into(),
        search_type: SearchType::Song,
        task_id: Some(1),
    };
    h.rt.sources.cursor.middle = 1;
    h.rt.sources.cursor.focus = ColumnFocus::Middle;

    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| mkpclient_tui::render::draw(frame, &app, &h.rt))
        .expect("draw");

    // Title bar names the search and shows the count.
    assert!(buffer_contains(&terminal, "Search: love (song)"));
    assert!(buffer_contains(&terminal, "— 3"));

    // All three rows rendered.
    assert!(buffer_contains(&terminal, "Alpha"));
    assert!(buffer_contains(&terminal, "Bravo"));
    assert!(buffer_contains(&terminal, "Charlie"));

    // Cursor row (index 1 = "Bravo") gets the cursor styling: a
    // yellow background with dark text. Look at any cell on the
    // Bravo row and assert it has bg=Yellow.
    let buf = terminal.backend().buffer();
    let mut bravo_row = None;
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if row.contains("Bravo") {
            bravo_row = Some(y);
            break;
        }
    }
    let row_y = bravo_row.expect("Bravo row not found");
    let row_styled =
        (0..buf.area.width).any(|x| buf[(x, row_y)].bg == ratatui::style::Color::Yellow);
    assert!(row_styled, "cursor row should have a yellow background");
    let _ = row_text(&terminal, row_y); // silence unused warning if any
}

#[test]
fn empty_discovery_shows_searching_hint() {
    let _ = env_logger::builder().is_test(true).try_init();

    // No mock server — just an idle Runtime.
    let trace: std::sync::Arc<dyn mkpclient_runtime::Trace> = std::sync::Arc::new(NoopTrace);
    let peer = mkpclient_runtime::Peer {
        user: "test".into(),
        host: "test-host".into(),
    };
    let mut rt = mkpclient_runtime_desktop::start_for_test(trace, peer);
    // Tick once so the runtime gets going.
    rt.tick();

    let app = AppState::default();
    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| mkpclient_tui::render::draw(frame, &app, &rt))
        .expect("draw");

    // Pre-connect surfaces "searching the network" (in the title)
    // and "Searching for Make Play servers" (centered hint when
    // discovery is empty). Either is fine — we just need *some*
    // discovery indicator on screen.
    let buf = terminal.backend().buffer();
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }
    assert!(
        all.contains("Searching") || all.contains("searching"),
        "expected a discovery hint, got:\n{all}"
    );
}

struct NoopTrace;
impl mkpclient_runtime::Trace for NoopTrace {}

/// Ticking the runtime long enough to surface the discovery
/// fallback ensures pre-connect screens don't panic when the
/// initial state has zero servers.
#[test]
fn pre_connect_does_not_panic_when_idle() {
    let _ = env_logger::builder().is_test(true).try_init();
    let trace: std::sync::Arc<dyn mkpclient_runtime::Trace> = std::sync::Arc::new(NoopTrace);
    let peer = mkpclient_runtime::Peer {
        user: "test".into(),
        host: "test-host".into(),
    };
    let mut rt = mkpclient_runtime_desktop::start_for_test(trace, peer);

    let app = AppState::default();
    rt.sources.session.preferred_server = Some("missing".into());

    // Tick a few times — runtime lifecycle may interact with discovery.
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        rt.tick();
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| mkpclient_tui::render::draw(frame, &app, &rt))
            .expect("draw");
    }
}
