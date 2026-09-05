//! Final step of the lifecycle: return a `Closed` link to `Idle` so
//! the runtime may dial again, and arm the reconnect backoff.
//!
//! `LinkPhase::Closed` is an *observation*, not a resting state. Two
//! steps need to see it — `apply_backend` (to clear `backend_name`
//! and stash `lost_server`) and `apply_lost_modal` (to raise the
//! modal) — after which it has served its purpose. Leaving the link
//! parked there is what made a dropped connection permanent: both
//! `connect_action` and `execute::apply_link` only act from `Idle`,
//! so nothing ever redialled. This step runs last in `execute::run`,
//! once those observers have had their tick.
//!
//! Spec §6: `desired_ack()` answers "should the link be released back
//! to Idle, and is a retry still wanted?"; `ack_action()` diffs
//! against the live phase; `apply_link_ack()` writes the phase and
//! the backoff synchronously.
//!
//! No polling: the backoff instant lands in `link.retry_at`, which
//! `nearest_deadline` folds into the loop's sleep. The loop wakes at
//! exactly the moment the next attempt becomes legal — and otherwise
//! stays blocked (`EXAMPLE-ARCH.md` § "Wake on event, don't spin").

use mkpclient_state_intent::Intent;
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_ui_session::UiSession;

use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct AckLinkInput {
    pub closed: bool,
}

impl AckLinkInput {
    pub fn new(l: &Link) -> Self {
        Self {
            closed: matches!(l.phase, LinkPhase::Closed),
        }
    }
}

/// Do we still want to be connected to something? A close that leaves
/// a target behind earns a retry; one the user asked for (give-up,
/// explicit disconnect) clears both and does not.
#[derive(drv::Input)]
pub struct AckWantInput {
    pub wants_connection: bool,
}

impl AckWantInput {
    pub fn new(s: &UiSession, i: &Intent) -> Self {
        Self {
            wants_connection: s.lost_server.is_some() || i.target.is_some(),
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredAck {
    /// Nothing to release — the link is not parked on `Closed`.
    Hold,
    /// Release to `Idle`. `retry` says whether a reconnect is still
    /// wanted, and so whether the backoff should be armed.
    Release { retry: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckAction {
    Noop,
    /// Flip `Closed` → `Idle`. `arm_backoff` widens `retry_at` so the
    /// next attempt waits; without it the ack would redial in the
    /// same tick and spin on an unreachable server.
    Ack {
        arm_backoff: bool,
    },
}

#[drv::memo(single)]
pub fn desired_ack(link: AckLinkInput, want: AckWantInput) -> DesiredAck {
    if !link.closed {
        return DesiredAck::Hold;
    }
    DesiredAck::Release {
        retry: want.wants_connection,
    }
}

#[drv::memo(single)]
pub fn ack_action(desired: DesiredAck) -> AckAction {
    match desired {
        DesiredAck::Hold => AckAction::Noop,
        DesiredAck::Release { retry } => AckAction::Ack { arm_backoff: retry },
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_link_ack(sources: &mut Sources) {
    let desired = desired_ack(
        AckLinkInput::new(&sources.link),
        AckWantInput::new(&sources.session, &sources.intent),
    );
    let AckAction::Ack { arm_backoff } = ack_action(desired) else {
        return;
    };

    sources.link.phase = LinkPhase::Idle;
    sources.link.kind = None;
    sources.link.target = None;

    if arm_backoff {
        sources.link.schedule_retry(sources.clock.now);
        // A probe that failed while the network was down is a stale
        // reachability fact, not a permanent verdict. `link_action`
        // treats `Failed` as "give up on this address", so without
        // this the very outage that dropped the link would poison the
        // address it needs to dial back. Successful fingerprints and
        // in-flight probes stay — only the failures are retried.
        sources.probes.retain_non_failed();
    } else {
        sources.link.clear_retry();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    use mkpclient_state_link::RETRY_BACKOFF;

    fn ack(link: &Link, session: &UiSession, intent: &Intent) -> AckAction {
        ack_action(desired_ack(
            AckLinkInput::new(link),
            AckWantInput::new(session, intent),
        ))
    }

    #[test]
    fn only_a_closed_link_is_released() {
        let session = UiSession::default();
        let intent = Intent::default();
        let mut link = Link::default();

        for phase in [
            LinkPhase::Idle,
            LinkPhase::Connecting,
            LinkPhase::Connected,
            LinkPhase::Closing,
        ] {
            link.phase = phase.clone();
            assert_eq!(ack(&link, &session, &intent), AckAction::Noop, "{phase:?}");
        }

        link.phase = LinkPhase::Closed;
        assert_eq!(
            ack(&link, &session, &intent),
            AckAction::Ack { arm_backoff: false }
        );
    }

    #[test]
    fn a_drop_with_a_server_still_wanted_arms_the_backoff() {
        let link = Link {
            phase: LinkPhase::Closed,
            ..Default::default()
        };
        let intent = Intent::default();

        // A lost server is one still wanted — this is the reconnect
        // case, and it must earn a backoff so the retry does not spin.
        let mut session = UiSession {
            lost_server: Some(std::sync::Arc::from("tower")),
            ..Default::default()
        };
        assert_eq!(
            ack(&link, &session, &intent),
            AckAction::Ack { arm_backoff: true }
        );

        // Giving up clears `lost_server`; nothing is wanted, so no
        // retry is scheduled and the link simply comes to rest.
        session.lost_server = None;
        assert_eq!(
            ack(&link, &session, &intent),
            AckAction::Ack { arm_backoff: false }
        );

        // An explicit target is equally a reason to retry.
        let intent_with_target = Intent {
            target: Some(std::sync::Arc::from("tower")),
            ..Default::default()
        };
        assert_eq!(
            ack(&link, &session, &intent_with_target),
            AckAction::Ack { arm_backoff: true }
        );
    }

    #[test]
    fn backoff_widens_then_holds_at_the_ceiling() {
        let t0 = Instant::now();
        let mut link = Link::default();

        for expected in RETRY_BACKOFF {
            link.schedule_retry(t0);
            assert_eq!(link.retry_at, Some(t0 + *expected));
        }
        // Past the end of the table the delay stops growing rather
        // than running away or panicking on the index.
        let ceiling = *RETRY_BACKOFF.last().unwrap();
        for _ in 0..5 {
            link.schedule_retry(t0);
            assert_eq!(link.retry_at, Some(t0 + ceiling));
        }
    }

    #[test]
    fn a_pending_backoff_withholds_permission_until_it_lapses() {
        let t0 = Instant::now();
        let mut link = Link::default();
        assert!(link.retry_allowed(t0), "no backoff means always allowed");

        link.schedule_retry(t0);
        let wait = RETRY_BACKOFF[0];
        assert!(!link.retry_allowed(t0));
        assert!(!link.retry_allowed(t0 + wait - Duration::from_millis(1)));
        assert!(link.retry_allowed(t0 + wait));

        // A successful connect wipes the schedule, so the next drop
        // starts again at the shortest delay rather than the ceiling.
        link.clear_retry();
        assert!(link.retry_allowed(t0));
        link.schedule_retry(t0);
        assert_eq!(link.retry_at, Some(t0 + RETRY_BACKOFF[0]));
    }
}
