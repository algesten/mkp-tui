//! Blocking, std-thread mDNS browser for `_mkplay._tcp.local`.
//!
//! Architecture (no tokio):
//! ```text
//!  per-interface query threads        folder thread
//!  ──────────────────────────         ─────────────
//!    opslag::Server + UdpSocket  ──▶  obs_rx       ──▶  DiscoveryEvent
//!    (one per IPv4 interface)    obs  (dedup by      event_tx
//!                                ──▶   name + TTL)
//! ```
//!
//! Per-interface threads run a synchronous opslag state machine and
//! forward each sighting as a raw `Observation` to the folder thread.
//! The folder thread dedupes, applies a TTL, and emits
//! `DiscoveryEvent::{Added, Refreshed, Removed}` to the runtime —
//! calling `Notifier::notify` after each send.
//!
//! Graceful shutdown isn't wired yet; worker threads run until
//! process exit. The folder thread exits when its `obs_rx` hangs up
//! (i.e. every interface thread has died, or when an explicit
//! `Shutdown` is posted in a future revision).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use opslag::{Cast, Input, Output, Server, Time};
use socket2::{Domain, Type};

use mkpclient_core::Notifier;
use mkpclient_driver_discovery_core::{DiscoveryDriver, DiscoveryEvent, ServerAd, Trace};

pub const SERVICE_TYPE: &str = "_mkplay._tcp.local";

const MDNS_PORT: u16 = 5353;
const GROUP_ADDR_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const GROUP_SOCK_V4: SocketAddrV4 = SocketAddrV4::new(GROUP_ADDR_V4, MDNS_PORT);
const ANY_MDNS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), MDNS_PORT);

/// How long a server can go unseen before the folder thread emits `Removed`.
const SERVER_TTL: Duration = Duration::from_secs(31);

/// Lifecycle marker. Matches the `*Native` pattern from led-rewrite:
/// the runtime keeps one around for the life of the process; threads
/// self-exit on channel hangup.
pub struct DiscoveryNative {
    _marker: (),
}

pub fn spawn(trace: Arc<dyn Trace>, notify: Notifier) -> (DiscoveryDriver, DiscoveryNative) {
    let (event_tx, event_rx) = mpsc::channel::<DiscoveryEvent>();
    let (obs_tx, obs_rx) = mpsc::channel::<Observation>();

    // One folder thread dedupes / TTLs / emits DiscoveryEvents.
    thread::Builder::new()
        .name("mkp-discovery-folder".into())
        .spawn({
            let notify = notify.clone();
            move || folder_loop(obs_rx, event_tx, notify)
        })
        .expect("spawning discovery folder thread should succeed");

    // Per-interface query threads. No rescan yet — interfaces that
    // come up after startup are invisible until the driver is
    // respawned. Good enough for a first slice.
    for ip in all_ipv4() {
        spawn_query_thread(ip, obs_tx.clone());
    }
    // Drop the original `obs_tx`: keeping it alive would stop the
    // folder thread from ever exiting once the interface threads
    // die. Clones given to interface threads are the only senders
    // now.
    drop(obs_tx);

    let driver = DiscoveryDriver::new(event_rx, trace);
    (driver, DiscoveryNative { _marker: () })
}

/// What an interface thread tells the folder thread.
enum Observation {
    Seen(ServerAd),
}

/// Folder-thread state: one entry per known server name.
struct Entry {
    ad: ServerAd,
    last_seen: Instant,
}

fn folder_loop(obs_rx: Receiver<Observation>, event_tx: Sender<DiscoveryEvent>, notify: Notifier) {
    let mut entries: HashMap<String, Entry> = HashMap::new();

    loop {
        // Compute the TTL deadline we actually care about (earliest
        // entry's `last_seen + SERVER_TTL`). With no entries we have
        // no reason to wake up — block indefinitely until an
        // observation arrives.
        let next_ttl = entries.values().map(|e| e.last_seen + SERVER_TTL).min();

        let obs = match next_ttl {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    Err(RecvTimeoutError::Timeout)
                } else {
                    obs_rx.recv_timeout(wait)
                }
            }
            None => obs_rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        match obs {
            Ok(Observation::Seen(ad)) => {
                let now = Instant::now();
                let name = ad.name.clone();
                let event = match entries.get_mut(&name) {
                    Some(existing) => {
                        let changed = existing.ad != ad;
                        existing.ad = ad.clone();
                        existing.last_seen = now;
                        if changed {
                            Some(DiscoveryEvent::Refreshed(ad))
                        } else {
                            None
                        }
                    }
                    None => {
                        entries.insert(
                            name,
                            Entry {
                                ad: ad.clone(),
                                last_seen: now,
                            },
                        );
                        Some(DiscoveryEvent::Added(ad))
                    }
                };
                if let Some(ev) = event {
                    if event_tx.send(ev).is_err() {
                        return;
                    }
                    notify.notify();
                }
            }
            Err(RecvTimeoutError::Timeout) => { /* fall through to TTL sweep */ }
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // TTL sweep — always safe to run; only removes entries that
        // are *actually* past the deadline we blocked on.
        let now = Instant::now();
        let expired: Vec<String> = entries
            .iter()
            .filter(|(_, e)| now.duration_since(e.last_seen) > SERVER_TTL)
            .map(|(name, _)| name.clone())
            .collect();
        for name in expired {
            entries.remove(&name);
            if event_tx.send(DiscoveryEvent::Removed { name }).is_err() {
                return;
            }
            notify.notify();
        }
    }
}

fn spawn_query_thread(ip: Ipv4Addr, obs_tx: Sender<Observation>) {
    thread::Builder::new()
        .name(format!("mkp-discovery-{}", ip))
        .spawn(move || {
            if let Err(e) = query_loop(ip, obs_tx) {
                debug!("mDNS query loop for {} exited: {}", ip, e);
            }
        })
        .expect("spawning discovery query thread should succeed");
}

fn query_loop(ip: Ipv4Addr, obs_tx: Sender<Observation>) -> io::Result<()> {
    let sock = make_query_socket(ip)?;
    let local = sock.local_addr().ok();
    info!(
        "mDNS: discovering {} on {} (local {:?})",
        SERVICE_TYPE, ip, local
    );

    let mask = if ip.is_loopback() {
        [255, 0, 0, 0]
    } else {
        [255, 255, 255, 0]
    };
    let mut server: Server<4, 4, 4, 1, 10> = Server::new(std::iter::empty());
    server.query(SERVICE_TYPE, ip, mask);

    let start = Instant::now();
    let time_now = || Time::from_millis(start.elapsed().as_millis() as u64);

    let mut packet = vec![0u8; 1500];
    let mut output = vec![0u8; 1500];
    let mut next_timeout = time_now();
    let mut input = Input::Timeout(next_timeout);

    loop {
        // Drain opslag outputs until it asks us to wait.
        loop {
            match server.handle(input, &mut output) {
                Output::Packet(n, cast) => {
                    let target = match cast {
                        Cast::Multi { .. } => SocketAddr::V4(GROUP_SOCK_V4),
                        Cast::Uni { target, .. } => target,
                    };
                    if let Err(e) = sock.send_to(&output[..n], target) {
                        debug!("mDNS send_to {target} failed: {e}");
                    }
                    input = Input::Timeout(time_now());
                }
                Output::Remote(service) => {
                    if let IpAddr::V4(addr) = service.ip_address() {
                        if service.port() > 0 {
                            let ad = ServerAd {
                                name: strip_service_suffix(&service.instance_name().to_string()),
                                host: service.hostname().to_string(),
                                addr,
                                port: service.port(),
                            };
                            if obs_tx.send(Observation::Seen(ad)).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    input = Input::Timeout(time_now());
                }
                Output::Timeout(t) => {
                    next_timeout = t;
                    break;
                }
            }
        }

        // Block waiting for a packet, or wake on the opslag-driven
        // timeout so the state machine can re-query.
        let wait_ms = time_now().millis_until(next_timeout);
        if wait_ms == 0 {
            input = Input::Timeout(time_now());
            continue;
        }
        sock.set_read_timeout(Some(Duration::from_millis(wait_ms)))?;

        match sock.recv_from(&mut packet) {
            Ok((n, from)) => {
                input = Input::Packet(&packet[..n], from);
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                input = Input::Timeout(time_now());
            }
            Err(e) => {
                error!("mDNS recv on {}: {}", ip, e);
                return Err(e);
            }
        }
    }
}

fn make_query_socket(ip: Ipv4Addr) -> io::Result<UdpSocket> {
    let sock = socket2::Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    sock.bind(&any.into())?;
    sock.set_multicast_if_v4(&ip)?;
    let std_sock: UdpSocket = sock.into();
    Ok(std_sock)
}

/// We don't currently bind a listen socket joined to the multicast
/// group — opslag sends queries out the ephemeral-port socket and
/// many mDNS responders reply back via unicast, which this socket
/// can receive. A follow-up revision can add a group-joined socket
/// if we find responders that only reply over multicast.
#[allow(dead_code)]
fn make_group_socket(ip: Ipv4Addr) -> io::Result<UdpSocket> {
    let sock = socket2::Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&ANY_MDNS.into())?;
    sock.join_multicast_v4(&GROUP_ADDR_V4, &ip)?;
    sock.set_multicast_if_v4(&ip)?;
    Ok(sock.into())
}

/// Strip the trailing `._mkplay._tcp.local` so UI shows bare names.
fn strip_service_suffix(instance_name: &str) -> String {
    let suffix = format!(".{}", SERVICE_TYPE);
    instance_name
        .strip_suffix(&suffix)
        .unwrap_or(instance_name)
        .to_string()
}

fn all_ipv4() -> Vec<Ipv4Addr> {
    let mut ips = vec![Ipv4Addr::LOCALHOST];
    let networks = sysinfo::Networks::new_with_refreshed_list();
    for (_name, data) in &networks {
        for ip_net in data.ip_networks() {
            if let IpAddr::V4(ip) = ip_net.addr {
                if !ip.is_loopback() && !ip.is_link_local() {
                    ips.push(ip);
                }
            }
        }
    }
    if ips.len() == 1 {
        warn!("mDNS: no non-loopback IPv4 interfaces found; discovery will be loopback-only");
    }
    ips
}
