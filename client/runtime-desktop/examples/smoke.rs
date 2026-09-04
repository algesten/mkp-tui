//! Smoke-test binary for the drv-based client runtime.
//!
//! Boots the runtime (spawning every native worker), then loops for
//! a fixed duration printing the current sources state each second.
//! If a paired server is discovered, dispatches a ConnectTo intent.
//!
//! Usage:
//! ```text
//! cargo run -p mkpclient-runtime-desktop --example smoke
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use mkpclient_runtime::{Peer, SemanticEvent, Trace};

struct NoopTrace;
impl Trace for NoopTrace {}

fn main() {
    env_logger::init();

    println!("[smoke] starting runtime…");
    let trace: Arc<dyn Trace> = Arc::new(NoopTrace);
    let peer = Peer {
        user: std::env::var("USER").unwrap_or_else(|_| "smoke".into()),
        host: "smoke-host".into(),
    };
    let mut rt = mkpclient_runtime_desktop::start(trace, peer);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut dispatched_connect = false;

    while Instant::now() < deadline {
        rt.tick();

        println!(
            "[smoke] link={:?} kind={:?} target={:?} err={:?} creds_loaded={} n_servers={} n_creds={}",
            rt.sources.link.phase,
            rt.sources.link.kind,
            rt.sources.link.target,
            rt.sources.link.last_err,
            rt.sources.credentials.loaded,
            rt.sources.discovery.servers.len(),
            rt.sources.credentials.entries.len()
        );
        for s in rt.sources.discovery.servers.iter() {
            let addr = format!("{}:{}", s.addr, s.port);
            let probe = rt.sources.probes.get(&addr);
            println!(
                "  server name={:?} host={:?} {} probe={:?}",
                s.name, s.host, addr, probe
            );
        }

        // If we see any discovered server, try to connect to the
        // first one. The runtime will probe its fingerprint and
        // match it to stored credentials on its own.
        if !dispatched_connect && rt.sources.credentials.loaded && rt.sources.link.target.is_none()
        {
            if let Some(s) = rt.sources.discovery.servers.iter().next() {
                println!(
                    "[smoke] dispatching ConnectTo {{ server_name = {:?} }}",
                    s.name
                );
                rt.dispatch(SemanticEvent::ConnectTo {
                    server_name: s.name.clone(),
                });
                dispatched_connect = true;
            }
        }

        rt.wait_for_wake(Duration::from_secs(1));
    }

    println!("[smoke] done.");
}
