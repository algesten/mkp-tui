//! External-fact source: the TLS link's lifecycle state.
//!
//! A single link at a time. The kind (`Client` vs `Pairing`) is
//! determined by the intent at `Connect` time and preserved here so
//! memos can distinguish a paired session (we can send `Request`s)
//! from an in-flight pairing handshake (we can only send
//! `PairClientMsg`s).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Authenticated session over ALPN `mkp-client`. `Request`s flow.
    Client,
    /// TOFU session over ALPN `mkp-pair`. Pairing-protocol frames only.
    Pairing,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinkPhase {
    #[default]
    Idle,
    /// TCP + TLS handshake in progress.
    Connecting,
    /// Handshake complete, ready for frames.
    Connected,
    /// Graceful teardown requested; worker is flushing + closing.
    Closing,
    /// Connection failed or was closed by the peer.
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Link {
    pub phase: LinkPhase,
    pub kind: Option<LinkKind>,
    /// The target the current link points at. For `Client`, a cred
    /// fingerprint; for `Pairing`, the mDNS server name. `None` when
    /// idle.
    pub target: Option<std::sync::Arc<str>>,
    /// The last error the worker reported. Cleared on a successful
    /// `Connected`. UI can surface this in a status line.
    pub last_err: Option<std::sync::Arc<str>>,
    /// When the worker last reported `Closed`, stamped from the
    /// runtime clock at ingest. `None` while idle or once a later
    /// `Connected` lands. The runtime's reconnect backoff is a query
    /// over this field and the clock: a still-wanted link is
    /// re-dialed once `closed_at + RECONNECT_DELAY` has passed.
    pub closed_at: Option<std::time::Instant>,
}
