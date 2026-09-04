//! Sync core of the TLS link driver.
//!
//! Pairing is folded in rather than living in a separate driver: it
//! is the same TCP + TLS machinery with a different ALPN and a
//! different verifier, and there is at most one link at a time
//! anyway. The runtime decides *which* kind of link to open by
//! picking between `ConnectClient` and `ConnectPair`. Events that
//! only make sense in one mode (`PairingReady`, `PairFailed`) are
//! simply never emitted in the other mode.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use mkproto::{ClientMsg, Response, TaskId};

pub use mkpclient_state_link::LinkKind;

/// Commands the runtime ships to the worker.
#[derive(Clone, Debug)]
pub enum LinkCmd {
    /// Open an authenticated (mTLS) client link.
    ConnectClient {
        /// `ip:port` — resolved address from discovery.
        addr: String,
        /// PEM-encoded server cert captured at pairing time. Used
        /// to build a pinned verifier so the link fails closed if
        /// the server presents a different cert.
        server_cert_pem: String,
        /// PEM-encoded client cert the server signed for us.
        client_cert_pem: String,
        /// PEM-encoded client private key (the other half of the
        /// keypair whose CSR was signed).
        client_key_pem: String,
        /// Human-facing identifier for traces; the runtime already
        /// knows the fingerprint from its own bookkeeping.
        fingerprint: String,
    },
    /// Open a TOFU pairing link. The worker captures the server
    /// cert during handshake, sends a `PairRequest` containing the
    /// CSR, and emits `PairingReady` once the server responds with
    /// a signed cert.
    ConnectPair {
        addr: String,
        /// The mDNS instance name, only used for traces. The TOFU
        /// verifier accepts any cert.
        server_name: String,
    },
    /// One-shot TOFU probe: connect with ALPN `mkp-client`, capture
    /// the server cert, close. The worker emits `ProbeResult` but
    /// does not touch the `Link` lifecycle — `state-link` stays in
    /// whatever phase it was in (usually `Idle`) throughout.
    ProbeFingerprint { addr: String },
    /// Tear down the active link (or cancel an in-flight handshake).
    Disconnect,
    /// Ship a client-mode `Request` frame. Only valid on a
    /// `LinkKind::Client` link; ignored otherwise.
    Send {
        seq: u64,
        task_id: Option<TaskId>,
        msg: ClientMsg,
    },
    /// Ship `PairClientMsg::PairConfirm`. Only valid during a
    /// pairing session after `PairingReady` has been emitted.
    ConfirmPair,
    /// Ship `PairClientMsg::PairReject`.
    RejectPair,
}

/// Events the worker posts back for the runtime's ingest phase.
#[derive(Debug)]
pub enum LinkEvent {
    /// TLS handshake completed. For `Client`, the worker is ready
    /// to relay `Send`s; for `Pairing`, it has already shipped the
    /// `PairRequest` and is waiting for the server's response.
    Connected { kind: LinkKind },
    /// Client-mode frame decoded off the wire. Seq-zero frames are
    /// broadcasts; non-zero ones are replies correlated to a prior
    /// `Send`. Boxed because `Response` is the wire-protocol enum
    /// and ~344 bytes — keeping it inline would inflate every
    /// `LinkEvent` variant the runtime ingests.
    Frame(Box<Response>),
    /// Pairing-mode cert exchange is done and the six-digit
    /// verification code has been computed from the TLS EKM +
    /// signed client cert. The runtime stashes this into
    /// `state-pairing` and waits for a user-dispatched
    /// `ConfirmPair` / `RejectPair`.
    PairingReady {
        server_cert_pem: String,
        client_cert_pem: String,
        client_key_pem: String,
        fingerprint: String,
        code: String,
    },
    /// The pairing-mode server returned `PairError`.
    PairFailed { message: String },
    /// The link closed. `error` is `None` for a clean disconnect
    /// (user asked or server shut down cleanly).
    Closed { error: Option<String> },
    /// One-shot probe completed. On `Ok`, the fingerprint is a
    /// hex-encoded SHA-256 of the server's cert DER; on `Err`, a
    /// human-readable reason. Orthogonal to the Link lifecycle.
    ProbeResult {
        addr: String,
        result: Result<String, String>,
    },
}

pub trait Trace: Send + Sync {
    fn link_connect(&self, addr: &str, kind: LinkKind);
    fn link_connected(&self, kind: LinkKind);
    fn link_send(&self, seq: u64);
    fn link_recv(&self, seq: u64);
    fn link_closed(&self, error: Option<&str>);
    fn pairing_ready(&self, fingerprint: &str, code: &str);
    fn pair_failed(&self, message: &str);
    fn probe(&self, addr: &str);
    fn probe_result(&self, addr: &str, result: &Result<String, String>);
}

pub struct NoopTrace;
impl Trace for NoopTrace {
    fn link_connect(&self, _: &str, _: LinkKind) {}
    fn link_connected(&self, _: LinkKind) {}
    fn link_send(&self, _: u64) {}
    fn link_recv(&self, _: u64) {}
    fn link_closed(&self, _: Option<&str>) {}
    fn pairing_ready(&self, _: &str, _: &str) {}
    fn pair_failed(&self, _: &str) {}
    fn probe(&self, _: &str) {}
    fn probe_result(&self, _: &str, _: &Result<String, String>) {}
}

pub struct LinkDriver {
    cmd_tx: Sender<LinkCmd>,
    event_rx: Receiver<LinkEvent>,
    trace: Arc<dyn Trace>,
}

impl LinkDriver {
    pub fn new(
        cmd_tx: Sender<LinkCmd>,
        event_rx: Receiver<LinkEvent>,
        trace: Arc<dyn Trace>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            trace,
        }
    }

    pub fn execute<'a, I>(&self, cmds: I)
    where
        I: IntoIterator<Item = &'a LinkCmd>,
    {
        for cmd in cmds {
            match cmd {
                LinkCmd::ConnectClient { addr, .. } => {
                    self.trace.link_connect(addr, LinkKind::Client)
                }
                LinkCmd::ConnectPair { addr, .. } => {
                    self.trace.link_connect(addr, LinkKind::Pairing)
                }
                LinkCmd::Send { seq, .. } => self.trace.link_send(*seq),
                LinkCmd::ProbeFingerprint { addr } => self.trace.probe(addr),
                LinkCmd::Disconnect | LinkCmd::ConfirmPair | LinkCmd::RejectPair => {}
            }
            if self.cmd_tx.send(cmd.clone()).is_err() {
                return;
            }
        }
    }

    pub fn process(&self) -> Vec<LinkEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                LinkEvent::Connected { kind } => self.trace.link_connected(*kind),
                LinkEvent::Frame(resp) => self.trace.link_recv(resp.seq),
                LinkEvent::PairingReady {
                    fingerprint, code, ..
                } => self.trace.pairing_ready(fingerprint, code),
                LinkEvent::PairFailed { message } => self.trace.pair_failed(message),
                LinkEvent::Closed { error } => self.trace.link_closed(error.as_deref()),
                LinkEvent::ProbeResult { addr, result } => self.trace.probe_result(addr, result),
            }
            out.push(ev);
        }
        out
    }
}
