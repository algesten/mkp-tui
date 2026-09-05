//! User-decision source: the target server(s) for connect / pair.
//!
//! Two independent fields rather than a single enum: the user can
//! legitimately be connected to server A (via `target`) while
//! initiating a pair with server B (via `pair_target`). The link
//! driver's action memo picks which to honour when both are set.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Intent {
    /// mDNS instance name of the server the user wants to be
    /// connected to — the same identity as `session.backend_name`,
    /// which is what lets the two be compared. `None` means "no
    /// active connection wanted".
    pub target: Option<std::sync::Arc<str>>,
    /// mDNS instance name of a server the user is pairing with.
    /// Cleared once pairing completes (success or failure).
    pub pair_target: Option<std::sync::Arc<str>>,
}

impl Intent {
    /// Test helper: an intent that names `server` as the client target.
    pub fn default_wanting(server: &str) -> Self {
        Self {
            target: Some(std::sync::Arc::from(server)),
            ..Default::default()
        }
    }
}
