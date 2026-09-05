//! Execute phase: thin trampoline over `*_action` memos.
//!
//! Per spec §6 the link's "what should be true / what to do" logic
//! is split into two memos:
//!   - [`desired_link`] — the user-decision answer ("we want a
//!     paired client session against server X"), derived from
//!     `intent` and (for `AwaitingConfirmation` survival)
//!     `pairing.phase`.
//!   - [`link_action`] — the diff against the live link source plus
//!     pre-requisites (probe, creds), returning [`LinkAction`]. The
//!     trampoline below dispatches each variant to the link driver.
//!
//! A link the worker reports `Closed` is just a link that is not
//! connected: as long as `intent.target` still names a server, the
//! diff re-dials it. Reconnecting after a drop is therefore emergent
//! rather than a handler — the only extra rule is a backoff so a
//! server that refuses us is not hammered every tick. That backoff,
//! and the one on failed probes, is [`retry_due`]: a query over the
//! instants on the link / probes / intent sources and the clock
//! (spec § "Time is a source field"). Nothing is aged out of a
//! source by the tick; the memo answers whether the held fact is
//! still current.

use std::sync::Arc;
use std::time::{Duration, Instant};

use imbl::{HashMap as ImHashMap, Vector};

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_driver_link_core::LinkCmd;
use mkpclient_state_clock::Clock;
use mkpclient_state_credentials::{Credentials, PairingEntry};
use mkpclient_state_discovery::Discovery;
use mkpclient_state_intent::Intent;
use mkpclient_state_link::{Link, LinkKind as StateLinkKind, LinkPhase};
use mkpclient_state_pairing::{Pairing, PairingPhase};
use mkpclient_state_probes::{ProbeOutcome, Probes};

use crate::drivers::Drivers;
use crate::sources::Sources;

/// How long after the worker reports `Closed` before a still-wanted
/// link is dialed again. Long enough that a refusing server is not
/// hit every tick, short enough that a blip reads as a pause.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// How long a failed probe stands before the address is probed
/// again. A server that is advertised but not yet answering (just
/// relaunched, or a stale mDNS entry) fails its probe; without an
/// expiry that failure would pin the address for good.
pub const FAILED_PROBE_TTL: Duration = Duration::from_secs(5);

// ─── outputs ────────────────────────────────────────────────────────

/// What the user (and pairing-survival rules) want the link to be.
/// Computed from `state-intent` plus `state-pairing.phase` — pure
/// "should be", with no I/O semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredLink {
    /// No active link wanted.
    Closed,
    /// User wants a paired-client session against this server name.
    Client { server_name: String },
    /// User wants a TOFU pairing handshake against this server name.
    Pairing { server_name: String },
}

/// Diff against the live link state. Drivers honour this verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Nothing to do.
    Noop,
    /// Probe the server's TOFU cert; the runtime needs the
    /// fingerprint to look up creds before opening a client link.
    Probe { addr: String },
    /// Open a client (mTLS) session.
    ConnectClient {
        addr: String,
        server_cert_pem: String,
        client_cert_pem: String,
        client_key_pem: String,
        fingerprint: String,
    },
    /// Open a TOFU pairing session.
    ConnectPair { addr: String, server_name: String },
    /// Close any active session.
    Disconnect,
}

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct IntentInput<'a> {
    pub target: Option<&'a std::sync::Arc<str>>,
    pub pair_target: Option<&'a std::sync::Arc<str>>,
}

impl<'a> IntentInput<'a> {
    pub fn new(i: &'a Intent) -> Self {
        Self {
            target: i.target.as_ref(),
            pair_target: i.pair_target.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct IntentRequestInput {
    pub requested_at: Option<Instant>,
}

impl IntentRequestInput {
    pub fn new(i: &Intent) -> Self {
        Self {
            requested_at: i.requested_at,
        }
    }
}

#[derive(drv::Input)]
pub struct LinkStateInput<'a> {
    pub phase_connected: bool,
    /// Teardown in flight: nothing may be dialed until the worker
    /// reports `Closed`.
    pub phase_closing: bool,
    /// The worker reported the link gone (peer closed, or the dial
    /// failed). Dialing again is gated by [`retry_due`].
    pub phase_closed: bool,
    pub kind_client: bool,
    pub kind_pairing: bool,
    pub target: Option<&'a std::sync::Arc<str>>,
}

impl<'a> LinkStateInput<'a> {
    pub fn new(l: &'a Link) -> Self {
        Self {
            phase_connected: matches!(l.phase, LinkPhase::Connected | LinkPhase::Connecting),
            phase_closing: matches!(l.phase, LinkPhase::Closing),
            phase_closed: matches!(l.phase, LinkPhase::Closed),
            kind_client: matches!(l.kind, Some(StateLinkKind::Client)),
            kind_pairing: matches!(l.kind, Some(StateLinkKind::Pairing)),
            target: l.target.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct LinkClosedInput {
    pub closed_at: Option<Instant>,
}

impl LinkClosedInput {
    pub fn new(l: &Link) -> Self {
        Self {
            closed_at: l.closed_at,
        }
    }
}

#[derive(drv::Input)]
pub struct ClockInput {
    pub now: Instant,
}

impl ClockInput {
    pub fn new(c: &Clock) -> Self {
        Self { now: c.now }
    }
}

#[derive(drv::Input)]
pub struct PairingPhaseInput {
    pub awaiting_confirmation: bool,
}

impl PairingPhaseInput {
    pub fn new(p: &Pairing) -> Self {
        Self {
            awaiting_confirmation: p.phase == PairingPhase::AwaitingConfirmation,
        }
    }
}

#[derive(drv::Input)]
pub struct DiscoveryInput<'a> {
    pub servers: &'a Vector<ServerAd>,
}

impl<'a> DiscoveryInput<'a> {
    pub fn new(d: &'a Discovery) -> Self {
        Self {
            servers: &d.servers,
        }
    }
}

#[derive(drv::Input)]
pub struct ProbesInput<'a> {
    pub by_addr: &'a ImHashMap<String, ProbeOutcome>,
}

impl<'a> ProbesInput<'a> {
    pub fn new(p: &'a Probes) -> Self {
        Self {
            by_addr: &p.by_addr,
        }
    }
}

#[derive(drv::Input)]
pub struct CredentialsInput<'a> {
    pub entries: &'a ImHashMap<String, PairingEntry>,
}

impl<'a> CredentialsInput<'a> {
    pub fn new(c: &'a Credentials) -> Self {
        Self {
            entries: &c.entries,
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[drv::memo(single)]
pub fn desired_link<'a>(intent: IntentInput<'a>, pairing: PairingPhaseInput) -> Arc<DesiredLink> {
    if let Some(name) = intent.pair_target {
        return Arc::new(DesiredLink::Pairing {
            server_name: name.to_string(),
        });
    }
    if let Some(name) = intent.target {
        return Arc::new(DesiredLink::Client {
            server_name: name.to_string(),
        });
    }
    if pairing.awaiting_confirmation {
        // Don't tear down a pairing link while the user is mid-
        // confirmation — even if intent has been cleared.
        return Arc::new(DesiredLink::Pairing {
            server_name: String::new(),
        });
    }
    Arc::new(DesiredLink::Closed)
}

/// Which time-gated retries are open right now. The one memo that
/// reads the clock on the link path: it recomputes every tick, and
/// its consumer [`link_action`] only re-fires when an answer flips.
#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub struct RetryDue {
    /// The user asked for a link since it last closed: dial now,
    /// regardless of backoff. Pairing links are only ever re-dialed
    /// this way.
    pub link_requested: bool,
    /// [`RECONNECT_DELAY`] has passed since the link closed (or it
    /// never closed).
    pub link_backoff_elapsed: bool,
    /// Addresses whose probe failure is older than
    /// [`FAILED_PROBE_TTL`]: treat as never probed.
    pub probe_addrs: Vec<String>,
}

#[drv::memo(single)]
pub fn retry_due<'a>(
    link: LinkClosedInput,
    intent: IntentRequestInput,
    probes: ProbesInput<'a>,
    clock: ClockInput,
) -> RetryDue {
    let (link_requested, link_backoff_elapsed) = match link.closed_at {
        None => (true, true),
        Some(closed_at) => (
            intent.requested_at.is_some_and(|t| t >= closed_at),
            clock.now >= closed_at + RECONNECT_DELAY,
        ),
    };
    let mut probe_addrs: Vec<String> = probes
        .by_addr
        .iter()
        .filter_map(|(addr, outcome)| match outcome {
            ProbeOutcome::Failed { at, .. } if clock.now >= *at + FAILED_PROBE_TTL => {
                Some(addr.clone())
            }
            _ => None,
        })
        .collect();
    probe_addrs.sort();
    RetryDue {
        link_requested,
        link_backoff_elapsed,
        probe_addrs,
    }
}

#[drv::memo(single)]
pub fn link_action<'a, 'b, 'c, 'd>(
    desired: Arc<DesiredLink>,
    link: LinkStateInput<'a>,
    retry: RetryDue,
    discovery: DiscoveryInput<'b>,
    probes: ProbesInput<'c>,
    creds: CredentialsInput<'d>,
    pairing: PairingPhaseInput,
) -> LinkAction {
    if link.phase_closing {
        // Teardown in flight counts as actual state: the worker
        // owns the socket until it reports `Closed`.
        return LinkAction::Noop;
    }
    match desired.as_ref() {
        DesiredLink::Pairing { server_name } => {
            // Already pairing this exact one? Wait.
            if link.kind_pairing
                && link.phase_connected
                && link.target.map(|s| &**s) == Some(server_name.as_str())
            {
                return LinkAction::Noop;
            }
            // Mid-confirmation placeholder — don't disconnect.
            if pairing.awaiting_confirmation && server_name.is_empty() {
                return LinkAction::Noop;
            }
            // A pairing handshake that ended is not retried on its
            // own; only a fresh ask from the user dials again.
            if link.phase_closed && !retry.link_requested {
                return LinkAction::Noop;
            }
            // Need an mDNS sighting to know where to dial.
            let Some(ad) = discovery.servers.iter().find(|s| s.name == *server_name) else {
                return LinkAction::Noop;
            };
            let addr = format!("{}:{}", ad.addr, ad.port);
            LinkAction::ConnectPair {
                addr,
                server_name: server_name.clone(),
            }
        }
        DesiredLink::Client { server_name } => {
            // Already connected as a client (any addr — the legacy
            // didn't track per-addr re-targeting).
            if link.kind_client && link.phase_connected {
                return LinkAction::Noop;
            }
            // Dropped: wait out the backoff before dialing again,
            // unless the user asked for this link since the drop.
            if link.phase_closed && !retry.link_requested && !retry.link_backoff_elapsed {
                return LinkAction::Noop;
            }
            // Need an mDNS sighting to know where to dial. A server
            // that is gone from mDNS is simply waited for.
            let Some(ad) = discovery.servers.iter().find(|s| s.name == *server_name) else {
                return LinkAction::Noop;
            };
            let addr = format!("{}:{}", ad.addr, ad.port);

            match probes.by_addr.get(&addr) {
                None => LinkAction::Probe { addr },
                // A failure that has aged out is as good as no probe;
                // the re-probe's in-flight mark overwrites it.
                Some(ProbeOutcome::Failed { .. }) if retry.probe_addrs.contains(&addr) => {
                    LinkAction::Probe { addr }
                }
                Some(ProbeOutcome::InFlight) | Some(ProbeOutcome::Failed { .. }) => {
                    LinkAction::Noop
                }
                Some(ProbeOutcome::Fingerprint(fp)) => {
                    let Some(entry) = creds.entries.get(fp) else {
                        return LinkAction::Noop;
                    };
                    LinkAction::ConnectClient {
                        addr,
                        server_cert_pem: entry.server_cert_pem.clone(),
                        client_cert_pem: entry.client_cert_pem.clone(),
                        client_key_pem: entry.client_key_pem.clone(),
                        fingerprint: fp.clone(),
                    }
                }
            }
        }
        DesiredLink::Closed => {
            if link.phase_connected && !pairing.awaiting_confirmation {
                LinkAction::Disconnect
            } else {
                LinkAction::Noop
            }
        }
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn run(sources: &mut Sources, drivers: &Drivers) {
    // Order: apply_connect first (writes intent), then apply_link
    // (reads intent, drives the link driver). Other lifecycle
    // applies don't depend on link state for this tick.
    crate::lifecycle::connect::apply_connect(sources);
    apply_link(sources, drivers);
    // apply_backend runs after apply_link so it sees the current
    // link.phase / link.target post-execute (the link driver writes
    // intent state synchronously inside its trampoline).
    crate::lifecycle::backend::apply_backend(sources, drivers);
    crate::lifecycle::server_errors::apply_server_errors(sources);
    crate::lifecycle::search_reopen::apply_search_reopen(sources, drivers);
    crate::lifecycle::cursor_snap::apply_cursor_snap(sources);
    crate::lifecycle::cursor_clamp::apply_middle_cursor_clamp(sources);
    crate::lifecycle::cursor_clamp::apply_queue_cursor_clamp(sources);
    crate::lifecycle::cursor_clamp::apply_left_cursor_clamp(sources);
    crate::lifecycle::cursor_clamp::apply_action_modal_clamp(sources);
    crate::lifecycle::pending_add::apply_pending_add(sources, drivers);
    crate::lifecycle::lost_modal::apply_lost_modal(sources);
    crate::lifecycle::restore::apply_restore(sources, drivers);
    crate::lifecycle::playlists_refetch::apply_playlists_refetch(sources);
    crate::lifecycle::playlists_refetch::apply_playlist_tracks_refetch(sources);
    // Runs after restore so the just-applied `history.mode` gets
    // mirrored to disk on the same tick.
    crate::lifecycle::view_persist::apply_view_persist(sources, drivers);
    crate::lifecycle::last_add_persist::apply_last_add_persist(sources, drivers);
    crate::lifecycle::search_history_push::apply_search_history_push(sources, drivers);
    // Drain `clipboard.pending` into a worker Cmd; toast lifecycle
    // fires once per outcome on the next tick after ingest.
    drivers.clipboard.execute(&mut sources.clipboard);
    crate::lifecycle::clipboard_toast::apply_clipboard_toast(sources, drivers);
    drain_send_queue(sources, drivers);
}

fn apply_link(sources: &mut Sources, drivers: &Drivers) {
    // If the user asked to connect to a server but its probe already
    // revealed a fingerprint we have no creds for, transparently swap
    // the intent over to pairing — the picker's Enter binding routes
    // through `intent.target`, and we want the unpaired path to "just
    // work" without forcing the user to pick a separate keybinding.
    fallback_target_to_pair(sources);

    let desired = desired_link(
        IntentInput::new(&sources.intent),
        PairingPhaseInput::new(&sources.pairing),
    );
    let retry = retry_due(
        LinkClosedInput::new(&sources.link),
        IntentRequestInput::new(&sources.intent),
        ProbesInput::new(&sources.probes),
        ClockInput::new(&sources.clock),
    );
    let action = link_action(
        desired,
        LinkStateInput::new(&sources.link),
        retry,
        DiscoveryInput::new(&sources.discovery),
        ProbesInput::new(&sources.probes),
        CredentialsInput::new(&sources.credentials),
        PairingPhaseInput::new(&sources.pairing),
    );

    match action {
        LinkAction::Noop => {}
        LinkAction::Probe { addr } => {
            sources.probes.mark_in_flight(addr.clone());
            drivers.link.execute([&LinkCmd::ProbeFingerprint { addr }]);
        }
        LinkAction::ConnectClient {
            addr,
            server_cert_pem,
            client_cert_pem,
            client_key_pem,
            fingerprint,
        } => {
            sources.link.phase = LinkPhase::Connecting;
            sources.link.kind = Some(StateLinkKind::Client);
            sources.link.target = Some(Arc::from(fingerprint.as_str()));
            drivers.link.execute([&LinkCmd::ConnectClient {
                addr,
                server_cert_pem,
                client_cert_pem,
                client_key_pem,
                fingerprint,
            }]);
        }
        LinkAction::ConnectPair { addr, server_name } => {
            sources.link.phase = LinkPhase::Connecting;
            sources.link.kind = Some(StateLinkKind::Pairing);
            sources.link.target = Some(Arc::from(server_name.as_str()));
            drivers
                .link
                .execute([&LinkCmd::ConnectPair { addr, server_name }]);
        }
        LinkAction::Disconnect => {
            sources.link.phase = LinkPhase::Closing;
            drivers.link.execute([&LinkCmd::Disconnect]);
        }
    }
}

fn fallback_target_to_pair(sources: &mut Sources) {
    let Some(name) = sources.intent.target.clone() else {
        return;
    };
    let Some(ad) = sources
        .discovery
        .servers
        .iter()
        .find(|s| s.name.as_str() == &*name)
    else {
        return;
    };
    let addr = format!("{}:{}", ad.addr, ad.port);
    let Some(ProbeOutcome::Fingerprint(fp)) = sources.probes.by_addr.get(&addr) else {
        return;
    };
    if sources.credentials.entries.contains_key(fp.as_str()) {
        return;
    }
    sources.intent.target = None;
    sources.intent.pair_target = Some(name);
}

fn drain_send_queue(sources: &mut Sources, drivers: &Drivers) {
    if !matches!(sources.link.phase, LinkPhase::Connected)
        || !matches!(sources.link.kind, Some(StateLinkKind::Client))
    {
        return;
    }
    while let Some(p) = sources.requests.pop_front() {
        drivers.link.execute([&LinkCmd::Send {
            seq: p.seq,
            task_id: p.task_id,
            msg: p.msg,
        }]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    use mkpclient_state_credentials::PairingEntry;

    fn ad(name: &str) -> ServerAd {
        ServerAd {
            name: name.into(),
            host: format!("{name}.local"),
            addr: Ipv4Addr::new(127, 0, 0, 1),
            port: 4242,
        }
    }

    fn entry(fp: &str) -> PairingEntry {
        PairingEntry {
            fingerprint: fp.into(),
            host: "h".into(),
            server_cert_pem: String::new(),
            client_cert_pem: String::new(),
            client_key_pem: String::new(),
        }
    }

    #[test]
    fn fallback_swaps_target_to_pair_when_probe_has_no_creds() {
        let mut sources = Sources::default();
        sources.discovery.upsert(ad("Toy Machine"));
        sources.intent.target = Some(Arc::from("Toy Machine"));
        sources
            .probes
            .set_fingerprint("127.0.0.1:4242".into(), "fp-x".into());
        // No credentials for fp-x.

        fallback_target_to_pair(&mut sources);

        assert!(sources.intent.target.is_none());
        assert_eq!(sources.intent.pair_target.as_deref(), Some("Toy Machine"));
    }

    #[test]
    fn fallback_is_a_noop_when_creds_match_probe() {
        let mut sources = Sources::default();
        sources.discovery.upsert(ad("Toy Machine"));
        sources.intent.target = Some(Arc::from("Toy Machine"));
        sources
            .probes
            .set_fingerprint("127.0.0.1:4242".into(), "fp-x".into());
        sources.credentials.insert(entry("fp-x"));

        fallback_target_to_pair(&mut sources);

        assert_eq!(sources.intent.target.as_deref(), Some("Toy Machine"));
        assert!(sources.intent.pair_target.is_none());
    }

    #[test]
    fn fallback_is_a_noop_when_probe_not_yet_done() {
        let mut sources = Sources::default();
        sources.discovery.upsert(ad("Toy Machine"));
        sources.intent.target = Some(Arc::from("Toy Machine"));
        // No probe outcome yet — apply_link will issue Probe; fallback
        // must wait until the fingerprint lands before deciding.

        fallback_target_to_pair(&mut sources);

        assert_eq!(sources.intent.target.as_deref(), Some("Toy Machine"));
        assert!(sources.intent.pair_target.is_none());
    }

    // ─── reconnect after a drop ─────────────────────────────────────

    /// Sources for a paired client that wants "Toy Machine": the
    /// server is in mDNS, its probe is cached, and creds match.
    fn wanted_and_reachable() -> Sources {
        let mut sources = Sources::default();
        sources.discovery.upsert(ad("Toy Machine"));
        sources
            .probes
            .set_fingerprint("127.0.0.1:4242".into(), "fp-x".into());
        sources.credentials.insert(entry("fp-x"));
        sources.intent.target = Some(Arc::from("Toy Machine"));
        sources
    }

    fn action(sources: &Sources) -> LinkAction {
        let desired = desired_link(
            IntentInput::new(&sources.intent),
            PairingPhaseInput::new(&sources.pairing),
        );
        let retry = retry_due(
            LinkClosedInput::new(&sources.link),
            IntentRequestInput::new(&sources.intent),
            ProbesInput::new(&sources.probes),
            ClockInput::new(&sources.clock),
        );
        link_action(
            desired,
            LinkStateInput::new(&sources.link),
            retry,
            DiscoveryInput::new(&sources.discovery),
            ProbesInput::new(&sources.probes),
            CredentialsInput::new(&sources.credentials),
            PairingPhaseInput::new(&sources.pairing),
        )
    }

    fn drop_link(sources: &mut Sources) {
        sources.link.phase = LinkPhase::Closed;
        sources.link.kind = None;
        sources.link.closed_at = Some(sources.clock.now);
    }

    #[test]
    fn dropped_link_is_not_redialed_before_the_backoff() {
        let mut sources = wanted_and_reachable();
        drop_link(&mut sources);
        assert_eq!(action(&sources), LinkAction::Noop);

        sources
            .clock
            .tick(sources.clock.now + RECONNECT_DELAY - Duration::from_millis(1));
        assert_eq!(action(&sources), LinkAction::Noop);
    }

    #[test]
    fn dropped_link_is_redialed_once_the_backoff_has_passed() {
        let mut sources = wanted_and_reachable();
        drop_link(&mut sources);
        sources.clock.tick(sources.clock.now + RECONNECT_DELAY);
        assert!(matches!(
            action(&sources),
            LinkAction::ConnectClient { ref addr, ref fingerprint, .. }
                if addr == "127.0.0.1:4242" && fingerprint == "fp-x"
        ));
    }

    #[test]
    fn dropped_link_waits_for_the_server_to_reappear_in_mdns() {
        let mut sources = wanted_and_reachable();
        drop_link(&mut sources);
        sources.clock.tick(sources.clock.now + RECONNECT_DELAY);
        sources.discovery.remove("Toy Machine");
        assert_eq!(action(&sources), LinkAction::Noop);

        // Back, on a new port: the cached probe is for the old
        // address, so the new one gets probed first.
        sources.discovery.upsert(ServerAd {
            port: 4343,
            ..ad("Toy Machine")
        });
        assert_eq!(
            action(&sources),
            LinkAction::Probe {
                addr: "127.0.0.1:4343".into()
            }
        );
    }

    #[test]
    fn dropped_link_nobody_wants_stays_down() {
        let mut sources = wanted_and_reachable();
        drop_link(&mut sources);
        sources.clock.tick(sources.clock.now + RECONNECT_DELAY);
        sources.intent.target = None;
        assert_eq!(action(&sources), LinkAction::Noop);
    }

    #[test]
    fn closing_link_is_left_to_the_worker() {
        let mut sources = wanted_and_reachable();
        sources.link.phase = LinkPhase::Closing;
        sources.link.kind = Some(StateLinkKind::Client);
        assert_eq!(action(&sources), LinkAction::Noop);
    }

    #[test]
    fn ended_pairing_is_not_retried_on_its_own() {
        let mut sources = Sources::default();
        sources.discovery.upsert(ad("Toy Machine"));
        sources.intent.pair_target = Some(Arc::from("Toy Machine"));
        drop_link(&mut sources);
        sources.clock.tick(sources.clock.now + RECONNECT_DELAY);
        assert_eq!(action(&sources), LinkAction::Noop);
    }

    #[test]
    fn an_ask_since_the_drop_dials_without_waiting() {
        let mut sources = wanted_and_reachable();
        // Asked before the drop: the ask does not outrank the backoff.
        sources.intent.requested_at = Some(sources.clock.now);
        sources
            .clock
            .tick(sources.clock.now + Duration::from_millis(1));
        drop_link(&mut sources);
        assert_eq!(action(&sources), LinkAction::Noop);

        // Asked again after seeing the drop (same tick counts).
        sources.intent.requested_at = Some(sources.clock.now);
        assert!(matches!(action(&sources), LinkAction::ConnectClient { .. }));

        // Pairing is re-dialed the same way.
        sources.intent.target = None;
        sources.intent.pair_target = Some(Arc::from("Toy Machine"));
        assert!(matches!(action(&sources), LinkAction::ConnectPair { .. }));
    }

    #[test]
    fn failed_probe_is_reprobed_only_once_it_has_aged_out() {
        let mut sources = wanted_and_reachable();
        sources
            .probes
            .set_failed("127.0.0.1:4242".into(), "refused".into(), sources.clock.now);
        assert_eq!(action(&sources), LinkAction::Noop);

        sources
            .clock
            .tick(sources.clock.now + FAILED_PROBE_TTL - Duration::from_millis(1));
        assert_eq!(action(&sources), LinkAction::Noop);

        sources
            .clock
            .tick(sources.clock.now + Duration::from_millis(1));
        assert_eq!(
            action(&sources),
            LinkAction::Probe {
                addr: "127.0.0.1:4242".into()
            }
        );
    }
}
