//! Round-8 adversarial regression cover for the transparent-reconnect
//! work.
//!
//! Test code and fixtures only; no production file is touched.

use std::sync::Arc;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{Peer, Runtime, Trace};
use mkpclient_state_link::{LinkKind, LinkPhase};
use mkpclient_state_ui_screen::Screen;

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

/// **A deliberate server switch is hijacked back to the server the
/// user just left.**
///
/// `server_picker_modal_select` (`dispatch.rs`) is the "switch server"
/// path: it points `preferred_server` and `intent.target` at the new
/// server, re-arms `auto_connect`, clears `lost_server`, and asks the
/// driver to close the current link.
///
/// The close then runs `apply_backend`, whose `lost` flag is
/// `intent.target.is_some() || intent.pair_target.is_some()`
/// (`lifecycle/backend.rs`). A switch has just *set* `intent.target`,
/// so the deliberate teardown is classified as a loss: the outgoing
/// server is stashed in `lost_server` and `auto_connect` is re-armed.
///
/// On `main` that was inert. `desired_connect` asked about
/// `preferred_server` first — which the switch had just pointed at the
/// new server — so the stale `lost_server` never got a turn, and
/// `connect_action` would not dial from `Closed` anyway.
///
/// `bbf5526` moved the `lost_server` branch *above* `preferred_server`.
/// Now, in the window where the new server's probe is still in flight
/// and the link is resting on `Closed` (which `bbf5526`'s sibling
/// change made dialable), `desired_connect` answers with the outgoing
/// server and `apply_connect` overwrites `intent.target` with it. The
/// user asked for "new" and the runtime dials "old".
#[test]
fn switching_servers_is_not_hijacked_back_to_the_old_one() {
    let mut rt = runtime();
    rt.sources.discovery.upsert(ad("old", 6000));
    rt.sources.discovery.upsert(ad("new", 6001));

    // Connected to "old", as after an ordinary startup connect.
    rt.sources.session.backend_name = Some(Arc::from("old"));
    rt.sources.session.preferred_server = Some(Arc::from("old"));
    rt.sources.session.auto_connect = false;
    rt.sources.intent.target = Some(Arc::from("old"));
    rt.sources.link.phase = LinkPhase::Connected;
    rt.sources.link.kind = Some(LinkKind::Client);

    // The user opens the server-picker modal and picks "new".
    rt.sources.screen = Screen::ServerPicker { selected: 1 };
    rt.dispatch(mkpclient_runtime::TuiCursorEvent::ServerPickerModalSelect);
    assert_eq!(
        rt.sources.intent.target.as_deref(),
        Some("new"),
        "fixture: the switch should have re-pointed intent at \"new\""
    );

    // The link driver reports the requested teardown.
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.kind = None;

    // First tick: the close is noticed. The new server has no probe
    // yet, so the link stays at rest on `Closed` while the probe runs.
    rt.tick();
    // A backoff armed by the teardown is not what this test is about;
    // let it lapse so the next tick is free to dial.
    rt.sources.link.retry_at = None;

    // Second tick: this is where the auto-connect query gets to speak.
    rt.tick();

    assert_eq!(
        rt.sources.intent.target.as_deref(),
        Some("new"),
        "the switch was hijacked: `apply_backend` recorded the \
         deliberate teardown of \"old\" as a loss, and `desired_connect` \
         now ranks that stale `lost_server` above the `preferred_server` \
         the user just chose, so the runtime redials the server the user \
         asked to leave"
    );
}

/// **After a reconnect, a deliberate disconnect is immediately undone.**
///
/// `apply_backend`'s `Clear { lost: true }` re-arms
/// `session.auto_connect` on every drop (`lifecycle/backend.rs`), and
/// the lost-server branch of `desired_connect` deliberately returns
/// `clear_auto_connect: false` so a *second* drop retries too. So once
/// a link has dropped and come back, `auto_connect` stays `true` for
/// the rest of the session while `preferred_server` still names the
/// server from disk.
///
/// On `main` that was inert: `connect_action` only dialled from
/// `LinkPhase::Idle`, and nothing in the codebase ever writes `Idle`
/// after startup — a closed link parked on `Closed` forever. This
/// branch makes `Closed` dialable, which is the fix the PR is for, but
/// it also hands the still-armed `auto_connect` a live path.
///
/// The user then asks to disconnect (`SemanticEvent::Disconnect`,
/// the server row's Enter). `disconnect` clears `intent.target`,
/// `apply_backend` correctly scores the close as `lost: false` — and
/// then `apply_connect` sees `auto_connect: true` with a
/// `preferred_server` that is present in mDNS and writes
/// `intent.target` straight back. The user cannot leave the server.
#[test]
fn a_deliberate_disconnect_after_a_reconnect_is_not_undone() {
    let mut rt = runtime();
    rt.sources.discovery.upsert(ad("tower", 6000));

    // Connected to "tower" after an ordinary startup auto-connect,
    // which leaves `auto_connect` cleared.
    rt.sources.session.preferred_server = Some(Arc::from("tower"));
    rt.sources.session.backend_name = Some(Arc::from("tower"));
    rt.sources.session.auto_connect = false;
    rt.sources.intent.target = Some(Arc::from("tower"));
    rt.sources.link.phase = LinkPhase::Connected;
    rt.sources.link.kind = Some(LinkKind::Client);

    // The link drops. This is real production code re-arming things.
    rt.sources.link.phase = LinkPhase::Closed;
    rt.tick();
    assert!(
        rt.sources.session.auto_connect,
        "fixture: the drop should have re-armed auto-connect"
    );

    // The redial lands: `apply_backend`'s `Set` arm clears
    // `lost_server` and re-adopts the backend. `auto_connect` is left
    // armed on purpose, so a later drop retries too.
    rt.sources.session.lost_server = None;
    rt.sources.session.backend_name = Some(Arc::from("tower"));
    rt.sources.link.phase = LinkPhase::Connected;
    rt.sources.link.kind = Some(LinkKind::Client);
    rt.sources.link.clear_retry();
    rt.tick();
    assert!(
        rt.sources.session.auto_connect,
        "fixture: a lost-server reconnect leaves auto-connect armed"
    );

    // The user asks to disconnect from the server row.
    rt.dispatch(mkpclient_runtime::SemanticEvent::Disconnect);
    assert_eq!(
        rt.sources.intent.target, None,
        "fixture: `disconnect` should have dropped the intent"
    );

    // The link driver reports the teardown.
    rt.sources.link.phase = LinkPhase::Closed;
    rt.sources.link.kind = None;
    rt.tick();
    rt.sources.link.retry_at = None;
    rt.tick();

    assert_eq!(
        rt.sources.intent.target, None,
        "the runtime redialled a server the user deliberately left: \
         `auto_connect` is still armed from the earlier drop and \
         `Closed` is now dialable, so `apply_connect` rewrote the \
         intent `disconnect` had just cleared"
    );
}
