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
    /// Earliest instant the next reconnect attempt may fire. `None`
    /// when no backoff is pending. Folded into the runtime's
    /// `nearest_deadline` so the loop wakes exactly when it lapses
    /// rather than polling for it.
    pub retry_at: Option<std::time::Instant>,
    /// Consecutive failed connect attempts, indexing `RETRY_BACKOFF`.
    /// Reset on a successful connect.
    pub retry_attempts: u32,
}

/// Reconnect backoff schedule. A dropped link retries on a widening
/// delay so a server that stays down doesn't spin the loop, while a
/// laptop waking from sleep is back within a second.
///
/// The runtime never sleeps a fixed tick to check these
/// (`EXAMPLE-ARCH.md` § "Wake on event, don't spin"): `retry_at` is
/// folded into `nearest_deadline`, so the loop wakes at exactly the
/// instant the next attempt becomes allowed.
pub const RETRY_BACKOFF: &[std::time::Duration] = &[
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(10),
];

impl Link {
    /// Schedule the next reconnect attempt, widening the delay each
    /// time. Called when a connect attempt fails.
    pub fn schedule_retry(&mut self, now: std::time::Instant) {
        let idx = (self.retry_attempts as usize).min(RETRY_BACKOFF.len() - 1);
        self.retry_at = Some(now + RETRY_BACKOFF[idx]);
        self.retry_attempts = self.retry_attempts.saturating_add(1);
    }

    /// Clear the backoff. Called on a successful connect and whenever
    /// the user picks a server explicitly — an explicit action should
    /// never sit behind a backoff earned by earlier failures.
    pub fn clear_retry(&mut self) {
        self.retry_at = None;
        self.retry_attempts = 0;
    }

    /// May a reconnect be attempted at `now`? `None` means no backoff
    /// is pending.
    pub fn retry_allowed(&self, now: std::time::Instant) -> bool {
        match self.retry_at {
            Some(t) => now >= t,
            None => true,
        }
    }
}
