//! Round-9 adversarial regression cover for the transparent-reconnect
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

/// A bare runtime with no server and no mock: this test drives sources
/// directly, so nothing needs to be on the wire.
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

/// **Disconnecting while the reconnect is running leaves a modal that
/// spins forever in front of a runtime that will never dial again.**
///
/// `6452f05` disarmed `session.auto_connect` inside `disconnect()`
/// (`dispatch.rs`) so a deliberate disconnect could not be undone by
/// the still-armed lost-server retry. It did not touch
/// `session.lost_server`, and nothing else clears it: `apply_backend`
/// only drops it in the `Set` arm (a *successful* reconnect), and
/// `Clear { lost: false }` leaves it alone.
///
/// `desired_lost_modal` (`lifecycle/lost_modal.rs`) is
/// `!connected && lost_server.is_some()`, so the modal stays up.
/// `desired_connect` (`lifecycle/connect.rs`) short-circuits on
/// `!auto_connect`, so nothing dials. The two agree on nothing: the
/// TUI keeps painting "⠋ reconnecting…" over the main view (the
/// `reconnecting` branch in `render/mod.rs` deliberately suppresses
/// the pre-connect screen for exactly this screen) and the 8 Hz
/// spinner deadline keeps the loop awake to animate a reconnect that
/// is not happening.
///
/// This is a state the earlier commits did not have. At `e7dbcaa` the
/// same sequence kept dialling — the round-8 defect. The fix swapped
/// "the disconnect is undone" for "the disconnect strands the modal";
/// neither is the desired state. A disconnect the user asked for
/// should leave `lost_server` clear, exactly as `server_lost_give_up`
/// does.
#[test]
fn disconnecting_during_a_reconnect_does_not_strand_the_modal() {
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

    // The link drops. Production code stashes `lost_server`, re-arms
    // `auto_connect` and raises the modal.
    rt.sources.link.phase = LinkPhase::Closed;
    rt.tick();
    assert!(
        matches!(rt.sources.screen, Screen::ServerLostModal { .. }),
        "fixture: the drop should have raised the reconnect modal"
    );
    assert_eq!(
        rt.sources.session.lost_server.as_deref(),
        Some("tower"),
        "fixture: the drop should have stashed the lost server"
    );

    // The user does not wait for the reconnect — they disconnect.
    rt.dispatch(mkpclient_runtime::SemanticEvent::Disconnect);

    // Let the loop run. The backoff is cleared each time so the
    // runtime is never merely waiting on it.
    for _ in 0..4 {
        rt.sources.link.retry_at = None;
        rt.tick();
    }

    let still_dialling = rt.sources.intent.target.is_some();
    let modal_up = matches!(rt.sources.screen, Screen::ServerLostModal { .. });
    assert!(
        !modal_up || still_dialling,
        "the reconnect modal is up with nothing left to dial it: \
         `disconnect` disarmed `auto_connect` but left `lost_server` \
         set, so `desired_lost_modal` keeps showing \"reconnecting…\" \
         while `desired_connect` returns Idle forever. \
         lost_server={:?} auto_connect={} intent.target={:?}",
        rt.sources.session.lost_server,
        rt.sources.session.auto_connect,
        rt.sources.intent.target,
    );
}
