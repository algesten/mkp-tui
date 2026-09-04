//! In-flight peer task activity, keyed by `task_id`.
//!
//! Server broadcasts `TaskStarted { task_id, peer, activity }` when
//! a peer kicks off a long-running operation (search, playlist
//! load, …) and `TaskCompleted/Failed { task_id }` when it ends.
//! We mirror that here so the TUI can render a status line.

use std::sync::Arc;
use std::time::{Duration, Instant};

use imbl::HashMap;
use mkproto::{Peer, TaskActivity, TaskId};

/// How long a task may sit unacknowledged before the GC considers
/// it stale. Mirrors `state-ui-toast` expiry semantics — without
/// this, a peer that crashes or never replies leaves "Searching…"
/// hanging on the now-playing bar forever.
pub const STALE_TASK_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTask {
    pub peer: Peer,
    pub activity: TaskActivity,
    /// Wall-clock instant the task was started (peer-broadcast
    /// `TaskStarted` arrival on this client). Used by
    /// [`Activity::reap_stale`].
    pub started_at: Instant,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    pub tasks: HashMap<TaskId, Arc<ActiveTask>>,
}

impl Activity {
    pub fn started(&mut self, task_id: TaskId, peer: Peer, activity: TaskActivity, now: Instant) {
        self.tasks.insert(
            task_id,
            Arc::new(ActiveTask {
                peer,
                activity,
                started_at: now,
            }),
        );
    }

    pub fn completed(&mut self, task_id: TaskId) {
        self.tasks.remove(&task_id);
    }

    pub fn most_recent(&self) -> Option<&Arc<ActiveTask>> {
        // imbl HashMap doesn't preserve insertion order; pick any.
        self.tasks.values().next()
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
    }

    /// Drop tasks whose `started_at + ttl` has already passed —
    /// peers that broadcast `TaskStarted` but never sent the
    /// matching `TaskCompleted` shouldn't pin the activity overlay
    /// forever.
    pub fn reap_stale(&mut self, now: Instant, ttl: Duration) {
        self.tasks.retain(|_, t| {
            now.checked_duration_since(t.started_at)
                .is_none_or(|age| age < ttl)
        });
    }

    /// Earliest instant at which a task in flight will become
    /// stale. Folded into `nearest_deadline` so the runtime wakes
    /// to reap.
    pub fn next_reap_at(&self, ttl: Duration) -> Option<Instant> {
        self.tasks.values().map(|t| t.started_at + ttl).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> Peer {
        Peer {
            user: "u".into(),
            host: "h".into(),
        }
    }

    #[test]
    fn started_then_completed_clears_the_task() {
        let mut a = Activity::default();
        a.started(
            7,
            peer(),
            TaskActivity::Searching { term: "x".into() },
            Instant::now(),
        );
        assert_eq!(a.tasks.len(), 1);
        a.completed(7);
        assert!(a.tasks.is_empty());
    }

    #[test]
    fn completed_with_unknown_task_id_is_noop() {
        let mut a = Activity::default();
        a.started(7, peer(), TaskActivity::Skipping, Instant::now());
        a.completed(99);
        assert_eq!(a.tasks.len(), 1);
    }

    #[test]
    fn most_recent_returns_an_active_task() {
        let mut a = Activity::default();
        assert!(a.most_recent().is_none());
        a.started(7, peer(), TaskActivity::Skipping, Instant::now());
        assert!(a.most_recent().is_some());
    }
}
