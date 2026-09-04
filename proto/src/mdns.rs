use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, info, warn};
use opslag::{Cast, Input, Output, Server, ServiceInfo, Time};
use socket2::{Domain, Type};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::time::Instant;

pub const SERVICE_TYPE: &str = "_mkplay._tcp.local";

const MDNS_PORT: u16 = 5353;
const GROUP_ADDR_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const GROUP_SOCK_V4: SocketAddrV4 = SocketAddrV4::new(GROUP_ADDR_V4, MDNS_PORT);
const ANY_MDNS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), MDNS_PORT);

/// Strip the `._mkplay._tcp.local` suffix from an mDNS instance name.
fn strip_service_suffix(instance_name: &str) -> String {
    let suffix = format!(".{}", SERVICE_TYPE);
    instance_name
        .strip_suffix(&suffix)
        .unwrap_or(instance_name)
        .to_string()
}

/// How often to re-query when disconnected.
const REQUERY_INTERVAL: Duration = Duration::from_secs(14);

/// How long before a server entry is considered stale.
const SERVER_TTL: Duration = Duration::from_secs(31);

/// How often to re-enumerate network interfaces.
const INTERFACE_RESCAN: Duration = Duration::from_secs(30);

/// All IPv4 addresses on all interfaces, including loopback.
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
    ips
}

pub fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "mkplay".into())
}

fn make_socket(ip: Ipv4Addr) -> std::io::Result<UdpSocket> {
    let sock = socket2::Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&ANY_MDNS.into())?;
    sock.join_multicast_v4(&GROUP_ADDR_V4, &ip)?;
    sock.set_multicast_if_v4(&ip)?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock)
}

/// Query-only socket: ephemeral port, no multicast group membership.
/// Sends queries to the multicast group and receives unicast responses.
fn make_query_socket(ip: Ipv4Addr) -> std::io::Result<UdpSocket> {
    let sock = socket2::Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    sock.set_nonblocking(true)?;
    let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    sock.bind(&any.into())?;
    sock.set_multicast_if_v4(&ip)?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock)
}

/// Discovered server info.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub name: String,
    pub host: String,
    pub addr: Ipv4Addr,
    pub port: u16,
}

// Internal entry with last-seen timestamp.
struct Entry {
    server: Discovered,
    last_seen: Instant,
}

/// Guard that aborts all sub-tasks when dropped (e.g. when the parent task is aborted).
struct TaskGuard(HashMap<Ipv4Addr, tokio::task::JoinHandle<()>>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        for (_, handle) in self.0.drain() {
            handle.abort();
        }
    }
}

/// Advertise this server on mDNS across all network interfaces.
/// Spawns a task per interface and rescans periodically. Runs forever.
pub async fn advertise(port: u16) {
    let host = hostname();
    let mut tasks = TaskGuard(HashMap::new());

    let ips = all_ipv4();
    if ips.is_empty() {
        warn!("mDNS: no network interfaces found for advertising");
    }

    for ip in ips {
        tasks.0.insert(ip, spawn_advertise_task(ip, port, &host));
    }

    // Watcher: periodically re-scan interfaces, spawn new tasks, abort stale ones.
    loop {
        tokio::time::sleep(INTERFACE_RESCAN).await;
        let current: HashSet<Ipv4Addr> = all_ipv4().into_iter().collect();

        // Abort tasks for interfaces that disappeared.
        tasks.0.retain(|ip, handle| {
            if current.contains(ip) && !handle.is_finished() {
                true
            } else {
                if !current.contains(ip) {
                    info!("mDNS: interface {} gone, stopping advertise task", ip);
                    handle.abort();
                }
                false
            }
        });

        // Spawn tasks for new interfaces.
        for ip in &current {
            if !tasks.0.contains_key(ip) {
                info!("mDNS: new interface {}, spawning advertise task", ip);
                tasks.0.insert(*ip, spawn_advertise_task(*ip, port, &host));
            }
        }
    }
}

fn spawn_advertise_task(ip: Ipv4Addr, port: u16, host: &str) -> tokio::task::JoinHandle<()> {
    let host = host.to_string();
    tokio::spawn(async move {
        let host_local = if host.ends_with(".local") {
            host.clone()
        } else {
            format!("{}.local", host)
        };
        let instance = host.trim_end_matches(".local");

        let mask = if ip.is_loopback() {
            [255, 0, 0, 0]
        } else {
            [255, 255, 255, 0]
        };

        let info = ServiceInfo::<4>::new(SERVICE_TYPE, instance, &host_local, ip, mask, port);

        let sock = match make_socket(ip) {
            Ok(s) => s,
            Err(e) => {
                error!("mDNS: advertise socket error for {}: {}", ip, e);
                return;
            }
        };

        info!("mDNS: advertising {} on {}:{}", SERVICE_TYPE, ip, port);
        mdns_loop(sock, [info].into_iter(), |_| {}).await;
    })
}

/// Handle to background mDNS discovery. Maintains a live server list.
#[derive(Clone)]
pub struct Discovery {
    entries: Arc<RwLock<Vec<Entry>>>,
    connected: Arc<AtomicBool>,
}

impl Discovery {
    /// Start background mDNS discovery. Returns immediately.
    /// Spawns a query loop on every network interface (including loopback)
    /// and a watcher task that periodically nudges queries and re-scans interfaces.
    pub fn start() -> Self {
        let entries = Arc::new(RwLock::new(Vec::new()));
        let nudge = Arc::new(tokio::sync::Notify::new());
        let connected = Arc::new(AtomicBool::new(false));
        let active_ips = Arc::new(std::sync::Mutex::new(HashSet::<Ipv4Addr>::new()));
        let discovery = Discovery {
            entries: entries.clone(),
            connected: connected.clone(),
        };

        let ips = all_ipv4();
        if ips.is_empty() {
            warn!("mDNS: no network interfaces found, discovery disabled");
            return discovery;
        }

        for ip in &ips {
            active_ips.lock().unwrap().insert(*ip);
        }

        for ip in ips {
            spawn_query_task(ip, entries.clone(), nudge.clone(), active_ips.clone());
        }

        // Watcher: periodically nudges query tasks and re-scans interfaces.
        {
            let nudge = nudge.clone();
            let entries = entries.clone();
            let active_ips = active_ips.clone();
            let connected = connected.clone();
            tokio::spawn(async move {
                let mut last_rescan = Instant::now();
                loop {
                    let interval = if connected.load(Ordering::Relaxed) {
                        INTERFACE_RESCAN
                    } else {
                        REQUERY_INTERVAL
                    };
                    tokio::time::sleep(interval).await;

                    nudge.notify_waiters();

                    if last_rescan.elapsed() >= INTERFACE_RESCAN {
                        last_rescan = Instant::now();
                        for ip in all_ipv4() {
                            let is_new = active_ips.lock().unwrap().insert(ip);
                            if is_new {
                                info!("mDNS: new interface {}, spawning query task", ip);
                                spawn_query_task(
                                    ip,
                                    entries.clone(),
                                    nudge.clone(),
                                    active_ips.clone(),
                                );
                            }
                        }
                    }
                }
            });
        }

        discovery
    }

    /// Signal whether the client is connected to a server.
    /// When connected, query rate is reduced.
    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    /// Remove all entries for a server by name.
    pub fn remove_server(&self, name: &str) {
        if let Ok(mut list) = self.entries.try_write() {
            list.retain(|e| e.server.name != name);
        }
    }

    /// Return all servers seen within the TTL window, deduplicated by
    /// mDNS name (keeping the entry with the lowest IP address).
    pub async fn servers(&self) -> Vec<Discovered> {
        let list = self.entries.read().await;
        let cutoff = Instant::now() - SERVER_TTL;
        let mut by_name: HashMap<&str, &Discovered> = HashMap::new();
        for entry in list.iter().filter(|e| e.last_seen > cutoff) {
            by_name
                .entry(&entry.server.name)
                .and_modify(|existing| {
                    if entry.server.addr < existing.addr {
                        *existing = &entry.server;
                    }
                })
                .or_insert(&entry.server);
        }
        let mut result: Vec<Discovered> = by_name.into_values().cloned().collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }
}

fn spawn_query_task(
    ip: Ipv4Addr,
    entries: Arc<RwLock<Vec<Entry>>>,
    nudge: Arc<tokio::sync::Notify>,
    active_ips: Arc<std::sync::Mutex<HashSet<Ipv4Addr>>>,
) {
    tokio::spawn(async move {
        let sock = match make_query_socket(ip) {
            Ok(s) => s,
            Err(e) => {
                debug!("mDNS: socket error for {}: {}", ip, e);
                active_ips.lock().unwrap().remove(&ip);
                return;
            }
        };

        let local_addr = sock.local_addr().ok();
        info!(
            "mDNS: discovering {} on {} (local {:?})",
            SERVICE_TYPE, ip, local_addr
        );

        let mask = if ip.is_loopback() {
            [255, 0, 0, 0]
        } else {
            [255, 255, 255, 0]
        };
        let mut server: Server<4, 4, 4, 1, 10> = Server::new(std::iter::empty());
        server.query(SERVICE_TYPE, ip, mask);

        mdns_loop_server_with_nudge(sock, server, nudge, ip, |d| {
            let entries = entries.clone();
            tokio::spawn(async move {
                let mut list = entries.write().await;
                if let Some(entry) = list
                    .iter_mut()
                    .find(|e| e.server.addr == d.addr && e.server.port == d.port)
                {
                    entry.last_seen = Instant::now();
                    entry.server = d;
                } else {
                    info!("mDNS: found {} at {}:{}", d.name, d.addr, d.port);
                    list.push(Entry {
                        server: d,
                        last_seen: Instant::now(),
                    });
                }
            });
        })
        .await;

        // Task exiting — remove from active set.
        active_ips.lock().unwrap().remove(&ip);
    });
}

/// Core mDNS event loop from services. Calls `on_remote` for each discovered service.
async fn mdns_loop<'a>(
    sock: UdpSocket,
    services: impl Iterator<Item = ServiceInfo<'a, 4>>,
    on_remote: impl Fn(Discovered),
) {
    let server: Server<4, 4, 4, 1, 10> = Server::new(services);
    mdns_loop_server(sock, server, on_remote).await;
}

/// Core mDNS event loop from a pre-built server. Calls `on_remote` for each discovered service.
async fn mdns_loop_server(
    sock: UdpSocket,
    mut server: Server<'_, 4, 4, 4, 1, 10>,
    on_remote: impl Fn(Discovered),
) {
    mdns_loop_inner(&sock, &mut server, &on_remote, None, None).await;
}

/// Like mdns_loop_server but re-queries on nudge signal.
async fn mdns_loop_server_with_nudge(
    sock: UdpSocket,
    mut server: Server<'_, 4, 4, 4, 1, 10>,
    nudge: Arc<tokio::sync::Notify>,
    ip: Ipv4Addr,
    on_remote: impl Fn(Discovered),
) {
    mdns_loop_inner(&sock, &mut server, &on_remote, Some(&nudge), Some(ip)).await;
}

async fn mdns_loop_inner<'a>(
    sock: &UdpSocket,
    server: &mut Server<'a, 4, 4, 4, 1, 10>,
    on_remote: &impl Fn(Discovered),
    nudge: Option<&tokio::sync::Notify>,
    query_ip: Option<Ipv4Addr>,
) {
    let start = tokio::time::Instant::now();
    let now = || {
        let ms = start.elapsed().as_millis() as u64;
        Time::from_millis(ms)
    };

    let mut packet = vec![0u8; 1500];
    let mut output = vec![0u8; 1500];
    let mut next_timeout = now();
    let mut input = Input::Timeout(next_timeout);

    loop {
        // Drain all pending outputs until the library yields a Timeout.
        loop {
            match server.handle(input, &mut output) {
                Output::Packet(n, cast) => {
                    let target = match cast {
                        Cast::Multi { .. } => SocketAddr::V4(GROUP_SOCK_V4),
                        Cast::Uni { target, .. } => target,
                    };
                    let res = sock.send_to(&output[..n], target).await;
                    debug!("mDNS: send {} bytes to {} -> {:?}", n, target, res);
                    input = Input::Timeout(now());
                }
                Output::Remote(service) => {
                    if let IpAddr::V4(addr) = service.ip_address() {
                        if service.port() > 0 {
                            on_remote(Discovered {
                                name: strip_service_suffix(&service.instance_name().to_string()),
                                host: service.hostname().to_string(),
                                addr,
                                port: service.port(),
                            });
                        }
                    }
                    input = Input::Timeout(now());
                }
                Output::Timeout(time) => {
                    next_timeout = time;
                    break;
                }
            }
        }

        // Wait for a packet, the next timeout, or a nudge to re-query.
        let millis = now().millis_until(next_timeout);
        if millis == 0 {
            input = Input::Timeout(now());
            continue;
        }

        let wait = Duration::from_millis(millis);
        let recv_fut = sock.recv_from(&mut packet);
        let nudge_fut = async {
            match nudge {
                Some(n) => n.notified().await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            result = tokio::time::timeout(wait, recv_fut) => {
                match result {
                    Ok(Ok((n, from))) => {
                        debug!("mDNS: recv {} bytes from {}", n, from);
                        input = Input::Packet(&packet[..n], from);
                    }
                    Ok(Err(e)) => {
                        error!("mDNS recv error: {}", e);
                        return;
                    }
                    Err(_) => {
                        debug!("mDNS: recv timeout (waited {}ms)", wait.as_millis());
                        input = Input::Timeout(now());
                    }
                }
            }
            _ = nudge_fut => {
                // Re-query immediately
                if let Some(ip) = query_ip {
                    server.query(SERVICE_TYPE, ip, [255, 255, 255, 0]);
                }
                input = Input::Timeout(now());
            }
        }
    }
}
