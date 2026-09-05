//! Round-7 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Test code and fixtures only; no production file is touched.
//!
//! Two of the three tests below are green at head. They are here
//! because the mutation that they kill survives the rest of the
//! suite: reverting the production line they name leaves all 214
//! tests passing, so the behaviour is currently unpinned. Each says
//! which mutation it kills.

use std::sync::Arc;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{Peer, Runtime, Trace};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_screen::Screen;
use mkpclient_tui::app::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

struct NoopTrace;
impl Trace for NoopTrace {}

/// A bare runtime with no server and no mock: these tests drive
/// sources directly, so nothing needs to be on the wire.
fn runtime() -> Runtime {
    // Keep the persist driver off the developer's real `~/.config/mkp`.
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

fn ad(name: &str, port: u16) -> ServerAd {
    ServerAd {
        name: name.into(),
        host: format!("{name}.local"),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port,
    }
}

fn buffer_contains(terminal: &Terminal<TestBackend>, needle: &str) -> bool {
    let buf = terminal.backend().buffer();
    (0..buf.area.height).any(|y| {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .contains(needle)
    })
}

/// **The automatic reconnect can dial a different server than the one
/// that was lost.**
///
/// `desired_connect` tries `session.preferred_server` *before*
/// `session.lost_server` (`lifecycle/connect.rs`, branches (a)/(d)),
/// and `apply_backend`'s `Clear { lost: true }` re-arms
/// `auto_connect` on every drop. Those two were never in contact on
/// `main`: a dropped link parked on `LinkPhase::Closed`, which
/// `connect_action` refused to dial from, so the preferred branch
/// could only ever run at startup — when there is no lost server to
/// outrank. Making `Closed` dialable is what puts them in the same
/// question.
///
/// `preferred_server` goes stale by an ordinary route. It is seeded
/// once per process from the on-disk `last_server`
/// (`ingest.rs`, `PersistEvent::LastServerLoaded`) and after that only
/// `server_picker_modal_select` and `server_lost_give_up` write it —
/// `picker_connect`, the pre-connect server list, does not. So "my
/// usual server is home, today I picked studio off the picker" leaves
/// `preferred_server = home` while `backend_name = studio`.
///
/// When studio then drops, the modal says "Lost connection to studio"
/// and the runtime dials **home**. Once home answers, the modal
/// closes and the user is on another server's library, with the view
/// restored for that other backend — the reconnect silently became a
/// server switch.
///
/// The knock-on is worse than the switch. The preferred branch
/// returns `clear_auto_connect: true`, so this same tick sets
/// `session.auto_connect = false`, disarming the one path that
/// rebuilds `intent` from `lost_server`. From there the retry loop
/// runs on `intent` alone — and `ingest`'s `LinkEvent::Closed` arm
/// empties `intent.pair_target` whenever a pairing handshake dies
/// with its socket. Reach that (a server that re-keyed while it was
/// away: the probe returns a fingerprint with no credential,
/// `fallback_target_to_pair` moves the intent over, the handshake
/// drops) and `auto_connect` is false, `intent` is empty,
/// `lost_server` is still set — so `desired_connect` answers `Idle`,
/// `desired_link` answers `Closed`, and the modal spins "reconnecting…"
/// over a runtime that will never dial again.
#[test]
fn the_reconnect_dials_the_server_that_was_lost() {
    let mut rt = runtime();
    rt.sources.discovery.upsert(ad("home", 6000));
    rt.sources.discovery.upsert(ad("studio", 6001));

    // Boot seeded `preferred_server` from disk; the user then picked
    // "studio" off the pre-connect picker, which does not update it.
    rt.sources.session.preferred_server = Some(Arc::from("home"));
    rt.sources.session.auto_connect = false;
    rt.sources.session.backend_name = Some(Arc::from("studio"));
    rt.sources.intent.target = Some(Arc::from("studio"));

    // The drop, exactly as `ingest`'s `LinkEvent::Closed` arm leaves
    // it: resting on `Closed` with the backoff armed.
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.schedule_retry(rt.sources.clock.now);
    rt.tick();

    assert_eq!(
        rt.sources.session.lost_server.as_deref(),
        Some("studio"),
        "fixture: the drop should have stashed the lost server"
    );
    assert!(
        rt.sources.session.auto_connect,
        "fixture: the drop should have re-armed auto-connect"
    );

    // The backoff lapses and the retry goes out.
    rt.sources.link.retry_at = None;
    rt.tick();

    assert_eq!(
        rt.sources.intent.target.as_deref(),
        Some("studio"),
        "the reconnect retargeted {:?} — a stale `preferred_server` \
         outranks `lost_server` in `desired_connect`, so the modal names \
         one server while the runtime dials another",
        rt.sources.intent.target,
    );
    assert!(
        rt.sources.session.auto_connect,
        "the retry cleared `auto_connect`, disarming the only path that \
         rebuilds `intent` from `lost_server` if it is ever emptied"
    );
}

/// **Giving up must drop a pairing intent too, and nothing pins that.**
///
/// `reconnect_regressions::giving_up_on_a_lost_server_stops_dialling_it`
/// names this behaviour but is a passenger: restore
/// `server_lost_give_up` to its `main` shape —
///
/// ```ignore
/// if sources.link.phase != LinkPhase::Idle {
///     disconnect(sources, drivers);
/// }
/// ```
///
/// — and the whole suite stays green. Its doc-comment reasons from the
/// deleted `link_ack` ("the link is released to `Idle` now"), but a
/// drop rests on `Closed`, so the old guard is still true and the old
/// `disconnect` still clears `intent.target`. The assertion passes
/// either way.
///
/// What actually differs is `intent.pair_target`, which `disconnect`
/// never touched. It is set whenever `fallback_target_to_pair` swaps a
/// reconnect over to pairing — a server that re-keyed while it was
/// away — and the lost modal is up throughout, because the link is not
/// connected. Give up there and the old code leaves `pair_target`
/// naming the abandoned server, so `desired_link` returns `Pairing`
/// and `apply_link` dials it again on the next tick.
#[test]
fn giving_up_drops_a_pairing_intent_as_well_as_a_client_one() {
    let mut rt = runtime();
    rt.sources.discovery.upsert(ad("tower", 6000));

    // Where a reconnect against a re-keyed server lands: the client
    // intent has been swapped for a pairing one, the link is resting
    // on `Closed`, and the lost modal is up over it.
    rt.sources.session.lost_server = Some(Arc::from("tower"));
    rt.sources.session.auto_connect = false;
    rt.sources.intent.target = None;
    rt.sources.intent.pair_target = Some(Arc::from("tower"));
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.screen = Screen::ServerLostModal {
        server: Arc::from("tower"),
    };

    rt.dispatch(mkpclient_runtime::TuiCursorEvent::ServerLostGiveUp);

    assert_eq!(
        rt.sources.intent.pair_target, None,
        "giving up left `intent.pair_target` naming the abandoned server"
    );

    // Nothing is merely waiting: give-up clears the backoff, so if the
    // intent survived, this tick would dial.
    assert!(!rt.sources.link.retry_pending(), "fixture: backoff cleared");
    rt.tick();
    assert!(
        !matches!(
            rt.sources.link.phase,
            LinkPhase::Connecting | LinkPhase::Connected
        ),
        "the runtime redialled the server the user had just given up on"
    );
}

/// **The headline behaviour of the PR — you are not thrown out to the
/// server picker — is not asserted anywhere.**
///
/// `render::draw` keeps the working layout while the lost modal is up
/// (`render/mod.rs`: `let reconnecting = matches!(…ServerLostModal…)`).
/// Replace that with `false` and every one of the 214 tests still
/// passes: the whole suite reasons about sources, and nothing looks at
/// what is painted. Restoring the pre-connect full-screen server list
/// on a drop *is* the bug the PR exists to fix, so it deserves an
/// assertion on the buffer.
#[test]
fn a_drop_paints_the_modal_over_the_layout_not_the_server_picker() {
    let mut rt = runtime();
    let app = AppState::default();

    rt.sources.session.lost_server = Some(Arc::from("tower"));
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.screen = Screen::ServerLostModal {
        server: Arc::from("tower"),
    };

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
    terminal
        .draw(|frame| mkpclient_tui::render::draw(frame, &app, &rt))
        .expect("draw");

    assert!(
        buffer_contains(&terminal, "Lost connection to tower"),
        "the reconnect modal was not painted"
    );
    assert!(
        !buffer_contains(&terminal, "Connecting to tower..."),
        "the drop repainted the full-screen pre-connect server list — \
         the user is back at server selection, which is exactly what \
         the PR set out to stop"
    );
    // The layout the user was working in is still there behind it.
    assert!(
        buffer_contains(&terminal, "Queue"),
        "the working layout was not painted behind the modal"
    );
}
