//! User-decision source: the target server(s) for connect / pair.
//!
//! Two independent fields rather than a single enum: the user can
//! legitimately be connected to server A (via `target`) while
//! initiating a pair with server B (via `pair_target`). The link
//! driver's action memo picks which to honour when both are set.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Intent {
    /// When the user last asked for a link (`ConnectTo` / `BeginPair`),
    /// stamped from the runtime clock at dispatch. A closed link is
    /// otherwise re-dialed only after a backoff; an ask made since it
    /// closed is honoured right away.
    pub requested_at: Option<std::time::Instant>,
    /// mDNS instance name of the server the user wants to be
    /// connected to. `None` means "no active connection wanted".
    /// Survives the link dropping: a still-set target is what the
    /// runtime reconnects to.
    pub target: Option<std::sync::Arc<str>>,
    /// mDNS instance name of a server the user is pairing with.
    /// Cleared once pairing completes (success or failure).
    pub pair_target: Option<std::sync::Arc<str>>,
}
