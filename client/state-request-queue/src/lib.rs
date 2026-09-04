//! User-decision source: outbound requests waiting to be shipped.
//!
//! Dispatch allocates a fresh `seq` with [`RequestQueue::push`] and
//! pushes a `Pending` onto the back. The `send_action` memo reads
//! the front entry when the link is connected and the execute phase
//! drains it into the link driver, which ships it on the wire.
//!
//! The `seq` returned from `push` is the handle a UI component uses
//! to correlate with a later response in `state-responses`.

use imbl::Vector;
use mkproto::{ClientMsg, TaskId};

#[derive(Debug, Clone)]
pub struct Pending {
    pub seq: u64,
    pub task_id: Option<TaskId>,
    pub msg: ClientMsg,
}

#[derive(Debug, Clone, Default)]
pub struct RequestQueue {
    pub next_seq: u64,
    pub next_task_id: TaskId,
    pub pending: Vector<Pending>,
}

impl RequestQueue {
    /// Allocate a seq, enqueue the request, return the allocated seq.
    /// Seqs start at 1 — seq 0 is reserved for broadcasts on the wire.
    pub fn push(&mut self, msg: ClientMsg, task_id: Option<TaskId>) -> u64 {
        self.next_seq = self.next_seq.saturating_add(1).max(1);
        let seq = self.next_seq;
        self.pending.push_back(Pending { seq, task_id, msg });
        seq
    }

    /// Allocate a fresh `task_id`. Used by long-running operations
    /// (search, queue load, …) that want to correlate streamed
    /// follow-up frames (seq=0) with the originating request.
    pub fn alloc_task_id(&mut self) -> TaskId {
        self.next_task_id = self.next_task_id.saturating_add(1).max(1);
        self.next_task_id
    }

    /// Pop the front pending request if any. Used by execute when the
    /// link is ready to take one.
    pub fn pop_front(&mut self) -> Option<Pending> {
        self.pending.pop_front()
    }

    /// Drop everything queued. Used when the link disconnects —
    /// pending requests to an absent server are meaningless.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_allocates_monotonic_seqs_starting_at_one() {
        let mut q = RequestQueue::default();
        let a = q.push(ClientMsg::Ping, None);
        let b = q.push(ClientMsg::Ping, None);
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(q.pending.len(), 2);
    }

    #[test]
    fn alloc_task_id_is_monotonic_and_starts_at_one() {
        let mut q = RequestQueue::default();
        assert_eq!(q.alloc_task_id(), 1);
        assert_eq!(q.alloc_task_id(), 2);
    }

    #[test]
    fn pop_front_drains_in_order() {
        let mut q = RequestQueue::default();
        let _ = q.push(ClientMsg::GetState, None);
        let _ = q.push(ClientMsg::GetPlaylists, None);
        let first = q.pop_front().unwrap();
        let second = q.pop_front().unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn push_carries_task_id() {
        let mut q = RequestQueue::default();
        let _ = q.push(ClientMsg::Ping, Some(42));
        assert_eq!(q.pending.front().unwrap().task_id, Some(42));
    }

    #[test]
    fn clear_drops_pending_but_leaves_seq_counter_alone() {
        let mut q = RequestQueue::default();
        let _ = q.push(ClientMsg::Ping, None);
        let _ = q.push(ClientMsg::Ping, None);
        q.clear();
        assert!(q.pending.is_empty());
        // Next seq is still monotonic so a stale reply can never
        // match a future request.
        assert_eq!(q.push(ClientMsg::Ping, None), 3);
    }
}
