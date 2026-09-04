//! User-decision source: the transient yellow toast that overlays
//! the now-playing bar's bottom-left line.
//!
//! `expires_at` is wall-clock — `drop_if_expired` is called from the
//! runtime's per-tick ingest sweep so by the time view-model memos
//! / consumers run, `message.is_some()` ⇔ "toast is live".

use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct UiToast {
    pub message: Option<String>,
    pub expires_at: Option<Instant>,
}

impl UiToast {
    /// Show a toast that auto-clears at `expires_at`. Caller
    /// computes the absolute deadline from the runtime's clock
    /// source (`EXAMPLE-ARCH.md` § "Time is a source field").
    pub fn show(&mut self, msg: impl Into<String>, expires_at: Instant) {
        self.message = Some(msg.into());
        self.expires_at = Some(expires_at);
    }

    pub fn clear(&mut self) {
        self.message = None;
        self.expires_at = None;
    }

    pub fn drop_if_expired(&mut self, now: Instant) {
        if let Some(deadline) = self.expires_at {
            if now >= deadline {
                self.clear();
            }
        }
    }

    /// For the loop's `nearest_deadline` fold.
    pub fn nearest_deadline(&self) -> Option<Instant> {
        self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    #[test]
    fn show_then_drop_when_expired() {
        let mut t = UiToast::default();
        let t0 = Instant::now();
        t.show("x", t0);
        t.drop_if_expired(t0 + Duration::from_millis(1));
        assert!(t.message.is_none());
        assert!(t.expires_at.is_none());
    }

    #[test]
    fn live_toast_survives() {
        let mut t = UiToast::default();
        let t0 = Instant::now();
        t.show("x", t0 + Duration::from_secs(10));
        t.drop_if_expired(t0);
        assert_eq!(t.message.as_deref(), Some("x"));
    }
}
