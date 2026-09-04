//! External-fact source: server-side playlist list.
//!
//! Populated by the response to `ClientMsg::GetPlaylists` (auto-fired
//! on connect, plus refired by the playlists-refetch lifecycle when
//! `stale` flips) and kept in sync via broadcast `PlaylistMutated` /
//! `PlaylistCreated` frames. Exact MusicKit track counts arrive as
//! task-scoped `PlaylistTrackCount` continuations after the initial
//! list is already renderable.

use std::sync::Arc;

use imbl::Vector;
use mkproto::{Playlist, TaskId};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Playlists {
    pub items: Vector<Arc<Playlist>>,
    /// False until the first `GetPlaylists` reply (success *or* error)
    /// lands. On error during the first load we flip `loaded = true`
    /// anyway so the rest of the UI proceeds.
    pub loaded: bool,
    /// Seq of any in-flight `GetPlaylists` — connect-time or refetch.
    /// Set when the request is enqueued, cleared when the matching
    /// response arrives. The lifecycle's refetch action diffs against
    /// this so we don't re-issue while one is in flight, and ingest
    /// uses it to suppress the otherwise-`ErrorModal`-bound error
    /// reply.
    pub pending_request: Option<u64>,
    /// Task correlating streamed `PlaylistTrackCount` continuations
    /// after the initial `Playlists` response has made the list usable.
    /// Refetches wait for this task to finish so two enrichment streams
    /// cannot race each other.
    pub pending_task: Option<TaskId>,
    /// Marked true when a `PlaylistMutation::Modified` broadcast
    /// arrives. The playlists-refetch lifecycle reads this and fires
    /// a fresh `GetPlaylists`; ingest clears it once the matching
    /// response lands.
    pub stale: bool,
}

impl Playlists {
    pub fn set_all(&mut self, items: Vec<Playlist>) {
        self.items = items.into_iter().map(Arc::new).collect();
        self.loaded = true;
    }

    pub fn upsert(&mut self, p: Playlist) {
        if let Some(existing) = self.items.iter_mut().find(|x| x.id == p.id) {
            *existing = Arc::new(p);
        } else {
            self.items.push_back(Arc::new(p));
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|p| p.id != id);
    }

    pub fn rename(&mut self, id: &str, new_name: String) {
        if let Some(slot) = self.items.iter_mut().find(|p| p.id == id) {
            let mut updated = (**slot).clone();
            updated.name = new_name;
            *slot = Arc::new(updated);
        }
    }

    /// Replace the exact track count for a playlist. No-op if the id
    /// is unknown or the value has not changed.
    pub fn set_track_count(&mut self, id: &str, track_count: usize) {
        if let Some(slot) = self.items.iter_mut().find(|p| p.id == id) {
            if slot.track_count == track_count {
                return;
            }
            let mut updated = (**slot).clone();
            updated.track_count = track_count;
            *slot = Arc::new(updated);
        }
    }

    /// Adjust `track_count` for the playlist with the given id by
    /// `delta`. Saturates at 0 on negative deltas. No-op if id is
    /// unknown.
    pub fn adjust_track_count(&mut self, id: &str, delta: i32) {
        if let Some(slot) = self.items.iter_mut().find(|p| p.id == id) {
            let current = slot.track_count as i64;
            let next = (current + delta as i64).max(0) as usize;
            if next == slot.track_count {
                return;
            }
            let mut updated = (**slot).clone();
            updated.track_count = next;
            *slot = Arc::new(updated);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(id: &str, name: &str, count: usize) -> Playlist {
        Playlist {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            track_count: count,
        }
    }

    #[test]
    fn upsert_inserts_new_then_replaces_existing() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 0));
        assert_eq!(p.items.len(), 1);
        p.upsert(pl("a", "A renamed", 0));
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].name, "A renamed");
    }

    #[test]
    fn rename_updates_name_for_matching_id() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 0));
        p.upsert(pl("b", "B", 0));
        p.rename("a", "A renamed".into());
        assert_eq!(p.items[0].name, "A renamed");
        assert_eq!(p.items[1].name, "B");
    }

    #[test]
    fn rename_noop_when_unknown_id() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 0));
        p.rename("missing", "X".into());
        assert_eq!(p.items[0].name, "A");
    }

    #[test]
    fn adjust_track_count_increments_and_decrements() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 5));
        p.adjust_track_count("a", 3);
        assert_eq!(p.items[0].track_count, 8);
        p.adjust_track_count("a", -2);
        assert_eq!(p.items[0].track_count, 6);
    }

    #[test]
    fn set_track_count_replaces_the_exact_value() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 0));
        p.set_track_count("a", 17);
        assert_eq!(p.items[0].track_count, 17);
    }

    #[test]
    fn adjust_track_count_saturates_at_zero() {
        let mut p = Playlists::default();
        p.upsert(pl("a", "A", 1));
        p.adjust_track_count("a", -10);
        assert_eq!(p.items[0].track_count, 0);
    }
}
