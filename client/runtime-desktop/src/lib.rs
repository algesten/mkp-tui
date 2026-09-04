//! Desktop wiring for `mkpclient-runtime`.
//!
//! Spawns the std-thread-based native workers and assembles them
//! into a `Runtime`. Mirrors what used to live in
//! `Runtime::start_with_options` before the runtime crate was split
//! into platform-neutral core + per-platform wiring.

use std::sync::mpsc;

use mkpclient_driver_clipboard_native_std as clipboard_native;
use mkpclient_driver_credentials_native_fs as cred_native;
use mkpclient_driver_discovery_core::{DiscoveryDriver, DiscoveryEvent};
use mkpclient_driver_discovery_native_std as disc_native;
use mkpclient_driver_link_native_std as link_native;
use mkpclient_driver_persist_core::{LoadKey, PersistCmd};
use mkpclient_driver_persist_native_fs as persist_native;
use mkpclient_runtime::{
    clipboard_trace, credentials_trace, discovery_trace, link_trace, make_wake, persist_trace,
    Drivers, NativeMarker, PeerIdentity, Runtime, RuntimeOptions, RuntimeTrace, Sources,
};

/// Like [`start_with_options`] with default options — auto-connect
/// to the previously used server as soon as it shows up in mDNS.
pub fn start(trace: RuntimeTrace, peer: PeerIdentity) -> Runtime {
    start_with_options(trace, peer, RuntimeOptions::default())
}

/// Spawn every desktop native worker and return a ready-to-tick
/// `Runtime`. The given `trace` is forwarded to every driver via the
/// runtime's per-driver trace adapters.
pub fn start_with_options(
    trace: RuntimeTrace,
    peer: PeerIdentity,
    options: RuntimeOptions,
) -> Runtime {
    let wake = make_wake();
    let notify = wake.notify.clone();

    let (discovery, disc_marker) =
        disc_native::spawn(discovery_trace(trace.clone()), notify.clone());
    let (credentials, cred_marker) =
        cred_native::spawn(credentials_trace(trace.clone()), notify.clone());
    let (link, link_marker) = link_native::spawn(link_trace(trace.clone()), notify.clone());
    let (persist_handle, persist_marker) =
        persist_native::spawn(persist_trace(trace.clone()), notify.clone());
    let (clipboard, clipboard_marker) =
        clipboard_native::spawn(clipboard_trace(trace.clone()), notify.clone());

    let natives: Vec<NativeMarker> = vec![
        Box::new(disc_marker),
        Box::new(cred_marker),
        Box::new(link_marker),
        Box::new(persist_marker),
        Box::new(clipboard_marker),
    ];
    let drivers = Drivers::from_handles(
        discovery,
        credentials,
        link,
        persist_handle,
        clipboard,
        natives,
    );

    let mut sources = Sources::default();

    sources.persist.loads_in_flight.insert(LoadKey::Keybindings);
    drivers.persist.execute([&PersistCmd::LoadKeybindings]);

    // Kick off credential load so the rest of startup has the
    // creds available before the user picks a server.
    drivers
        .credentials
        .execute([&mkpclient_driver_credentials_core::CredCmd::Load]);

    if options.pick {
        // `--pick`: bypass auto-connect entirely so the user lands
        // on the server picker. Skipping `LoadLastServer` keeps
        // `preferred_server` empty, which avoids a transient
        // "Connecting to <last>" status before discovery fills in.
        sources.session.auto_connect = false;
    } else {
        // Kick off `last_server` load so auto-connect has the
        // preferred name as soon as discovery turns up the server.
        // The driver dedups via `Persist::loads_in_flight`, but only
        // the runtime tracks that — flag the load now.
        sources.persist.loads_in_flight.insert(LoadKey::LastServer);
        drivers.persist.execute([&PersistCmd::LoadLastServer]);
    }

    Runtime::from_parts(sources, drivers, peer, wake)
}

/// Test variant of [`start`] that **does not** spawn the mDNS
/// discovery worker. Returns a runtime whose `DiscoveryDriver` is
/// wired to a closed event channel — `process()` always yields
/// `vec![]` — so tests can write to `sources.discovery` directly
/// without real-network mDNS traffic racing them.
///
/// Other natives (credentials, link, persist) still spawn so tests
/// that exercise the link path keep their existing behavior. Tests
/// that don't want filesystem persistence either inject `XDG_CONFIG_HOME`
/// (per the `Harness::connect` pattern) or stub the persist source
/// directly.
pub fn start_for_test(trace: RuntimeTrace, peer: PeerIdentity) -> Runtime {
    let wake = make_wake();
    let notify = wake.notify.clone();

    // Closed-sender stub: the receiver returns `Empty` immediately
    // and `Disconnected` once `process()` runs. Either way no events
    // ever arrive in `sources.discovery`.
    let (disc_tx, disc_rx) = mpsc::channel::<DiscoveryEvent>();
    drop(disc_tx);
    let discovery = DiscoveryDriver::new(disc_rx, discovery_trace(trace.clone()));

    let (credentials, cred_marker) =
        cred_native::spawn(credentials_trace(trace.clone()), notify.clone());
    let (link, link_marker) = link_native::spawn(link_trace(trace.clone()), notify.clone());
    let (persist_handle, persist_marker) =
        persist_native::spawn(persist_trace(trace.clone()), notify.clone());
    let (clipboard, clipboard_marker) =
        clipboard_native::spawn(clipboard_trace(trace.clone()), notify.clone());

    let natives: Vec<NativeMarker> = vec![
        Box::new(cred_marker),
        Box::new(link_marker),
        Box::new(persist_marker),
        Box::new(clipboard_marker),
    ];
    let drivers = Drivers::from_handles(
        discovery,
        credentials,
        link,
        persist_handle,
        clipboard,
        natives,
    );

    // Tests inject creds + discovery directly; we deliberately do
    // **not** issue `CredCmd::Load` here. The Load worker reads from
    // disk asynchronously and the resulting `CredEvent::Loaded`
    // clears `sources.credentials` before re-populating. If the test
    // injected a credential between `start_for_test` and the first
    // `tick`, the Load result would wipe it out — racing the test
    // into a 5s "link did not connect" timeout. Skipping Load means
    // `sources.credentials.loaded` stays false, which is fine: the
    // link action keys on credential presence, not the loaded flag.
    let mut sources = Sources::default();
    sources.session.auto_connect = false;

    Runtime::from_parts(sources, drivers, peer, wake)
}
