//! Harness that wires a `Runtime` into a `MockServer`.
//!
//! Usage:
//! ```ignore
//! let mock = MockServer::start(certs::generate(), Box::new(|msg| ...));
//! let mut h = Harness::connect(mock);
//! h.dispatch(SemanticEvent::SendRequest { msg: ClientMsg::GetState, task_id: None });
//! h.tick_until(|sources| sources.server.play.is_some(), Duration::from_secs(2));
//! ```
//!
//! `tick_until` blocks the test thread on `wait_for_wake` between
//! ticks so we don't burn CPU spinning.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_runtime::{DispatchEvent, Peer, Runtime, SemanticEvent, Trace};
use mkpclient_state_credentials::PairingEntry;

use super::mock_server::MockServer;

struct NoopTrace;
impl Trace for NoopTrace {}

// Fields read by some tests but not all — cargo emits per-test
// warnings when a target doesn't touch them.
#[allow(dead_code)]
pub struct Harness {
    pub rt: Runtime,
    pub mock: MockServer,
    /// Fingerprint of the mock server's cert — useful for assertions.
    pub fingerprint: String,
}

impl Harness {
    /// Build a Runtime, inject the mock as a discovered server with a
    /// pre-existing pairing credential, and dispatch ConnectTo. Block
    /// until the link reports Connected.
    pub fn connect(mock: MockServer) -> Self {
        // tempdir scopes the persist driver's writes (`last_server`,
        // `last_view`, `search_history`) to a fresh dir per test run
        // so it never touches the developer's real `~/.config/mkp`.
        // The credentials driver isn't gated by this — but `start_for_test`
        // skips its Load command entirely, so it never reads disk.
        // (`credentials_native_fs` resolves paths from HOME, not
        // XDG_CONFIG_HOME, so this var only redirects the persist
        // driver.)
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        // Leak the tempdir so it survives the harness lifetime.
        Box::leak(Box::new(tmp));

        let trace: Arc<dyn Trace> = Arc::new(NoopTrace);
        let peer = Peer {
            user: "test".into(),
            host: "test-host".into(),
        };
        let mut rt = mkpclient_runtime_desktop::start_for_test(trace, peer);

        // Inject discovery — the runtime's discovery driver listens
        // for mDNS, but we go around it and write straight to the
        // source.
        let mock_addr = mock.addr;
        let server_name = format!("mock-{}", mock_addr.port());
        let addr_v4 = match mock_addr.ip() {
            std::net::IpAddr::V4(v4) => v4,
            std::net::IpAddr::V6(_) => Ipv4Addr::LOCALHOST,
        };
        rt.sources.discovery.upsert(ServerAd {
            name: server_name.clone(),
            host: "127.0.0.1".into(),
            addr: addr_v4,
            port: mock_addr.port(),
        });

        // Inject probe fingerprint so `execute` doesn't need to
        // open a TLS probe connection first.
        let addr_key = format!("{}:{}", mock_addr.ip(), mock_addr.port());
        let fingerprint = mock.certs.fingerprint.clone();
        rt.sources
            .probes
            .set_fingerprint(addr_key.clone(), fingerprint.clone());

        // Inject credential keyed on fingerprint.
        rt.sources.credentials.insert(PairingEntry {
            fingerprint: fingerprint.clone(),
            host: "127.0.0.1".into(),
            server_cert_pem: mock.certs.server_cert_pem.clone(),
            client_cert_pem: mock.certs.client_cert_pem.clone(),
            client_key_pem: mock.certs.client_key_pem.clone(),
        });

        rt.dispatch(SemanticEvent::ConnectTo { server_name });

        let mut h = Harness {
            rt,
            mock,
            fingerprint,
        };
        // Tick until the handshake has completed — not merely until
        // the socket is up. The server answers `Hello` with
        // `BackendChanged`, and that reply starts the session:
        // it resets the sources and issues `GetState` /
        // `GetPlaylists`. A test that seeds state in between would
        // have it reset out from under itself.
        h.tick_until(
            |rt| {
                matches!(
                    rt.sources.link.phase,
                    mkpclient_state_link::LinkPhase::Connected
                ) && rt.sources.server.backend.is_some()
            },
            Duration::from_secs(5),
        )
        .expect("link did not connect and hand shake within 5s");
        h
    }

    #[allow(dead_code)]
    pub fn tick_once(&mut self) {
        self.rt.tick();
    }

    /// Run ticks (with `wait_for_wake` between) until `cond(rt)` is
    /// true, or the deadline elapses.
    pub fn tick_until<F: Fn(&Runtime) -> bool>(
        &mut self,
        cond: F,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.rt.tick();
            if cond(&self.rt) {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("timeout".into());
            }
            self.rt
                .wait_for_wake(remaining.min(Duration::from_millis(50)));
        }
    }

    #[allow(dead_code)]
    pub fn dispatch<E: Into<DispatchEvent>>(&mut self, ev: E) {
        self.rt.dispatch(ev);
    }
}
