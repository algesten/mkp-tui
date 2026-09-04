//! Round-trip test: a fresh `Sources` + a fake `UiBridgeDriver`
//! produces one push per `ViewKind` on the first `run_render`, and
//! zero pushes on the second (everything cached as in-flight).

use std::sync::mpsc;
use std::sync::Arc;

use mkpclient_driver_ui_bridge_core::{BridgeCmd, NoopTrace, UiBridgeDriver, UiBridgeState};
use mkpclient_runtime::render::run_render;
use mkpclient_runtime::Sources;
use mkproto::Peer;

/// Total ViewKind variants currently wired through the bridge.
/// Update this number whenever `ViewKind` (in driver-ui-bridge-core)
/// gets a new variant + a `push_*` arm in `render::run_render`.
const TOTAL_VIEW_KINDS: usize = 24;

#[test]
fn first_render_pushes_one_per_view_kind() {
    let (tx, rx) = mpsc::channel::<BridgeCmd>();
    let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
    let mut state = UiBridgeState::default();
    let sources = Sources::default();
    let peer = Peer {
        user: "u".into(),
        host: "h".into(),
    };

    run_render(&sources, &peer, &driver, &mut state);

    let mut count = 0;
    while rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(
        count, TOTAL_VIEW_KINDS,
        "first render should push every view"
    );
}

#[test]
fn second_render_is_a_noop_when_sources_unchanged() {
    let (tx, rx) = mpsc::channel::<BridgeCmd>();
    let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
    let mut state = UiBridgeState::default();
    let sources = Sources::default();
    let peer = Peer {
        user: "u".into(),
        host: "h".into(),
    };

    run_render(&sources, &peer, &driver, &mut state);
    while rx.try_recv().is_ok() {} // drain initial pushes

    run_render(&sources, &peer, &driver, &mut state);
    assert!(
        rx.try_recv().is_err(),
        "second render with unchanged sources should be a no-op"
    );
}

#[test]
fn modal_push_changes_when_screen_flips() {
    use mkpclient_driver_ui_bridge_core::ViewKind;
    use mkpclient_state_ui_screen::Screen;

    let (tx, rx) = mpsc::channel::<BridgeCmd>();
    let driver = UiBridgeDriver::new(tx, Arc::new(NoopTrace));
    let mut state = UiBridgeState::default();
    let mut sources = Sources::default();
    let peer = Peer {
        user: "u".into(),
        host: "h".into(),
    };

    // First render with default (no modal active): every modal
    // payload encodes `None`.
    run_render(&sources, &peer, &driver, &mut state);
    while rx.try_recv().is_ok() {}

    // Open the help overlay and re-render — only that modal's
    // payload should change.
    sources.screen = Screen::HelpOverlay { scroll: 0 };
    run_render(&sources, &peer, &driver, &mut state);

    let mut changed_kinds = Vec::new();
    while let Ok(BridgeCmd::Push { kind, .. }) = rx.try_recv() {
        changed_kinds.push(kind);
    }
    assert_eq!(
        changed_kinds,
        vec![ViewKind::HelpOverlay],
        "only HelpOverlay should re-push when its screen state flips"
    );

    // Close it again — payload reverts to None, one push fires.
    sources.screen = Screen::NowPlaying;
    run_render(&sources, &peer, &driver, &mut state);
    let mut closed_kinds = Vec::new();
    while let Ok(BridgeCmd::Push { kind, .. }) = rx.try_recv() {
        closed_kinds.push(kind);
    }
    assert_eq!(closed_kinds, vec![ViewKind::HelpOverlay]);
}
