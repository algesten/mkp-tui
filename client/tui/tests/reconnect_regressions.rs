//! Adversarial regression cover for the transparent-reconnect work.
//!
//! Each test here expresses behaviour the reconnect feature promises
//! but does not yet deliver. They are deliberately red on
//! `restore-transparent-reconnect` @ ae5bf5a.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{Peer, Runtime, Trace};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_ui_screen::Screen;
use mkpclient_tui::app::AppState;
use mkpclient_tui::input::{translate, UiInput};

struct NoopTrace;
impl Trace for NoopTrace {}

/// A bare runtime with no server and no mock: every test below drives
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

fn ad(name: &str) -> ServerAd {
    ServerAd {
        name: name.into(),
        host: format!("{name}.local"),
        addr: std::net::Ipv4Addr::LOCALHOST,
        port: 6000,
    }
}

/// Put a runtime where a real `LinkEvent::Closed` leaves it: the phase
/// resting on `Closed` and the backoff armed. These tests drive sources
/// directly (no mock server), so they stand in for ingest here; that a
/// genuine drop arms the backoff is covered end-to-end by
/// `reconnect.rs::a_closed_link_is_dialable_without_being_reset_first`.
fn close_link(rt: &mut Runtime) {
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.schedule_retry(rt.sources.clock.now);
}

fn key(code: KeyCode) -> UiInput {
    UiInput::Key(code, KeyModifiers::NONE, KeyEventKind::Press)
}

/// Put the runtime where a dropped link leaves it: the lost-server
/// modal up, the link released back to `Idle` behind it, and some
/// *other* server sitting at the invisible pre-connect picker's cursor.
fn reconnecting(rt: &mut Runtime) {
    rt.sources.discovery.upsert(ad("someone-else"));
    rt.sources.cursor.server_picker = 0;
    rt.sources.session.lost_server = Some(Arc::from("tower"));
    rt.sources.link.phase = LinkPhase::Idle;
    rt.sources.screen = Screen::ServerLostModal {
        server: Arc::from("tower"),
    };
}

/// The modal this PR made visible paints its own key legend —
/// " Connection lost — Enter=pick another · Esc=keep waiting " — and
/// `input.rs` has a `Screen::ServerLostModal => translate_server_lost_modal`
/// arm for it. That arm is unreachable: the gate above it
/// (`input.rs:130`, `if link.phase != Connected`) still hands every
/// key to the pre-connect picker, and the modal is up exactly when
/// the link is not connected. Only the render half of "the modal was
/// structurally unreachable" was fixed.
///
/// Esc is `Action::DiscoveringQuit` in the picker's keymap, and
/// `translate_picker` returns `true` for it — so the key the modal
/// advertises as "keep waiting" terminates the application.
#[test]
fn esc_on_the_lost_modal_keeps_waiting_instead_of_quitting() {
    let mut rt = runtime();
    let mut app = AppState::default();
    reconnecting(&mut rt);

    let quit = translate(key(KeyCode::Esc), &mut rt, &mut app);

    assert!(
        !quit,
        "Esc on the reconnect modal quit the application — the modal's \
         own title advertises it as \"Esc=keep waiting\""
    );
}

/// Enter is advertised as "pick another". It reaches
/// `translate_picker` instead, whose `DiscoveringSelect` runs
/// `picker_connect`: that dials `discovery.servers[cursor.server_picker]`
/// — a row of a list that is not on screen — rather than the
/// give-up-and-choose flow the modal names.
#[test]
fn enter_on_the_lost_modal_does_not_dial_an_offscreen_picker_row() {
    let mut rt = runtime();
    let mut app = AppState::default();
    reconnecting(&mut rt);

    translate(key(KeyCode::Enter), &mut rt, &mut app);

    assert_eq!(
        rt.sources.intent.target, None,
        "Enter silently connected to {:?}, chosen by the hidden \
         pre-connect picker's cursor",
        rt.sources.intent.target
    );
    assert!(
        !matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
        "Enter left the modal up — the lost-modal key handler never ran"
    );
}

/// `AckWantInput::wants_connection` reads `session.lost_server` and
/// `intent.target`, but not `intent.pair_target`. A pairing link that
/// closes therefore takes the `arm_backoff: false` branch, which
/// *clears* the backoff — and `intent.pair_target` survives the close,
/// so the next tick's `desired_link` returns `Pairing` again and
/// `apply_link` redials immediately. Against a server that is gone
/// (the usual reason a pairing link closes) that is an unthrottled
/// redial storm: `Closed` → `Idle` → `ConnectPair` → `Closed`, as fast
/// as a TCP connect can fail.
///
/// On `main` this could not happen: the link parked on `Closed`.
#[test]
fn a_closed_pairing_link_is_throttled_before_redialling() {
    let mut rt = runtime();
    rt.sources.discovery.upsert(ad("tower"));
    rt.sources.intent.pair_target = Some(Arc::from("tower"));
    close_link(&mut rt);

    rt.tick();

    assert_ne!(
        rt.sources.link.phase,
        LinkPhase::Connecting,
        "a closed pairing link redialled inside its own backoff — \
         `pair_target` survives the close just as `target` does, so the \
         throttle has to cover it too"
    );
    assert!(
        rt.sources.link.retry_pending(),
        "the pairing backoff was cleared rather than withheld"
    );
}

/// `nearest_deadline` is a min-fold, and the loop turns its answer
/// into a sleep with
/// `d.checked_duration_since(now).unwrap_or(Duration::from_secs(60))`
/// (`runtime/src/lib.rs:177`). An instant that has already passed maps
/// to `None` there, so it does not mean "wake now" — it means **sleep
/// a minute**, and because it is the smallest candidate it silently
/// swallows every live deadline in the fold: the 8 Hz spinner, toast
/// expiry, stale-activity reaping.
///
/// `link.retry_at` is the first candidate that can be in the past.
/// `schedule_retry` sets it and only `clear_retry` (a successful
/// connect, or an explicit user pick) ever unsets it — nothing clears
/// it when it merely lapses. While the lost server is absent from
/// mDNS, `desired_connect` answers `Wait`, no attempt is made, and
/// nothing re-arms it: the value stays in the past for as long as the
/// outage lasts. That is precisely the reconnect modal this PR
/// introduces, with its spinner frozen.
#[test]
fn a_lapsed_retry_deadline_does_not_swallow_the_fold() {
    let mut rt = runtime();
    rt.tick();

    // Where `schedule_retry` leaves things once its delay has elapsed
    // and no connect attempt has been possible.
    rt.sources.link.phase = LinkPhase::Idle;
    rt.sources.session.lost_server = Some(Arc::from("tower"));
    rt.sources.link.retry_at = Some(Instant::now() - Duration::from_secs(1));

    // The harm is the sleep the loop computes, which is what the
    // `unwrap_or(60s)` produced. A deadline already due must mean
    // "wake now", never "park for a minute" — otherwise it masks
    // every live candidate behind it.
    assert!(
        rt.next_timeout() < Duration::from_millis(200),
        "a lapsed deadline made the loop park for {:?}, masking the \
         spinner and toast deadlines behind it",
        rt.next_timeout()
    );

    // And the stale instant does not survive a tick: the per-tick
    // sweep forgets it, so the fold is not handed a past candidate in
    // the first place and `retry_pending` keeps meaning "still being
    // withheld".
    rt.tick();
    assert_eq!(
        rt.sources.link.retry_at, None,
        "a lapsed backoff survived the tick; while it lingers it is \
         both a false veto on dialling and the smallest candidate in \
         nearest_deadline"
    );
    let deadline =
        mkpclient_runtime::nearest_deadline(&rt.sources).expect("the spinner keeps one pending");
    assert!(
        deadline >= rt.sources.clock.now,
        "nearest_deadline still returned an instant that had passed"
    );
}

/// The backoff is meant to be what keeps the retry from spinning:
/// "without it the ack would redial in the same tick and spin on an
/// unreachable server" (`link_ack.rs`). It does not gate the ordinary
/// reconnect at all.
///
/// `connect_action`'s new `retry_allowed` gate only guards
/// `apply_connect`, which *writes* `intent.target`. But `intent.target`
/// survives a close — nothing in `ingest`, `link_ack` or `apply_backend`
/// clears it — and `apply_link` reads `intent` directly through
/// `desired_link`, with no retry gate of its own. So the tick after
/// `link_ack` releases the link to `Idle`, `link_action` returns
/// `ConnectClient` and the dial goes out while `retry_at` is still
/// hundreds of milliseconds in the future.
///
/// Against a host that is up but refusing (server process died, mDNS
/// record still cached) that is the reconnect storm the backoff was
/// added to prevent: Closed → Idle → ConnectClient → Closed, at the
/// speed of a TCP RST.
#[test]
fn the_armed_backoff_actually_delays_the_redial() {
    let mut rt = runtime();

    // A live session against "tower", exactly as `connect_to` left it.
    rt.sources.discovery.upsert(ad("tower"));
    rt.sources.intent.target = Some(Arc::from("tower"));
    let addr = format!("{}:{}", std::net::Ipv4Addr::LOCALHOST, 6000);
    rt.sources
        .probes
        .set_fingerprint(addr, "deadbeef".to_string());
    rt.sources
        .credentials
        .insert(mkpclient_state_credentials::PairingEntry {
            fingerprint: "deadbeef".into(),
            host: "127.0.0.1".into(),
            server_cert_pem: String::new(),
            client_cert_pem: String::new(),
            client_key_pem: String::new(),
        });
    rt.sources.session.backend_name = Some(Arc::from("tower"));

    // The drop.
    close_link(&mut rt);
    let retry_at = rt
        .sources
        .link
        .retry_at
        .expect("the drop should have armed a backoff");
    assert!(
        retry_at > Instant::now(),
        "fixture: the backoff should still be pending"
    );

    // The very next tick, still well inside the backoff window.
    rt.tick();

    assert_ne!(
        rt.sources.link.phase,
        LinkPhase::Connecting,
        "the link redialled {:?} before its own backoff lapsed — the gate \
         has to sit on `link_action`, which dials from `intent`, not only \
         on `apply_connect`, which merely writes it",
        retry_at.saturating_duration_since(Instant::now()),
    );
}

/// "Give up" stopped giving up.
///
/// `server_lost_give_up` clears the session breadcrumbs and then
/// tears the socket down conditionally:
///
/// ```ignore
/// if sources.link.phase != LinkPhase::Idle {
///     disconnect(sources, drivers);   // <- the only thing that clears intent.target
/// }
/// ```
///
/// On `main` a dropped link sat on `LinkPhase::Closed`, so that guard
/// was true and `disconnect` ran, clearing `intent.target`. `link_ack`
/// now releases the link to `Idle` at the end of the tick that saw the
/// close, so by the time the user reaches the modal and gives up the
/// guard is false, `disconnect` is skipped, and `intent.target` still
/// names the abandoned server. `apply_link` redials it on the next
/// tick — the user is put back on the server they just walked away
/// from, with the view they were shown having been discarded.
#[test]
fn giving_up_on_a_lost_server_stops_dialling_it() {
    let mut rt = runtime();

    rt.sources.discovery.upsert(ad("tower"));
    rt.sources.intent.target = Some(Arc::from("tower"));
    let addr = format!("{}:{}", std::net::Ipv4Addr::LOCALHOST, 6000);
    rt.sources
        .probes
        .set_fingerprint(addr, "deadbeef".to_string());
    rt.sources
        .credentials
        .insert(mkpclient_state_credentials::PairingEntry {
            fingerprint: "deadbeef".into(),
            host: "127.0.0.1".into(),
            server_cert_pem: String::new(),
            client_cert_pem: String::new(),
            client_key_pem: String::new(),
        });
    rt.sources.session.backend_name = Some(Arc::from("tower"));

    // The drop, then the tick that raises the modal.
    close_link(&mut rt);
    rt.tick();

    rt.dispatch(mkpclient_runtime::TuiCursorEvent::ServerLostGiveUp);

    assert_eq!(
        rt.sources.intent.target, None,
        "giving up left intent.target pointing at the abandoned server"
    );

    // Give-up clears the backoff too, so nothing here is merely waiting:
    // if intent still named the server, this tick would dial it.
    rt.tick();
    assert_eq!(
        rt.sources.intent.target, None,
        "intent.target came back after giving up"
    );
    assert!(
        !matches!(
            rt.sources.link.phase,
            LinkPhase::Connecting | LinkPhase::Connected
        ),
        "the runtime redialled the server the user had just given up on"
    );
}
