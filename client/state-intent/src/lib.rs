//! User-decision source: the target server(s) for connect / pair.
//!
//! Two independent fields rather than a single enum: the user can
//! legitimately be connected to server A (via `target`) while
//! initiating a pair with server B (via `pair_target`). The link
//! driver's action memo picks which to honour when both are set.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Intent {
    /// SHA-256 fingerprint of a server cert the user wants to be
    /// connected to. `None` means "no active connection wanted".
    pub target: Option<std::sync::Arc<str>>,
    /// mDNS instance name of a server the user is pairing with.
    /// Cleared once pairing completes (success or failure).
    pub pair_target: Option<std::sync::Arc<str>>,
}
