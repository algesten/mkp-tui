//! User-decision source: optimistic-update breadcrumbs for
//! playlist mutations in flight.
//!
//! Per EXAMPLE-ARCH §3 ("Shadow sources"), the user's optimistic view
//! of the playlist list (and one playlist's tracks) lives in a
//! separate source from the server-mirrors (`state-playlists`,
//! `state-playlist-tracks`). View memos for the left column and the
//! middle pane merge the two. Reconciliation lives in ingest, keyed
//! on the protocol's existing identity (request seq for response
//! matching, playlist id for broadcast matching):
//!
//!   - `creating[seq]` ← `PlaylistCreated` response (success) /
//!     `Error` reply (rollback).
//!   - `deleting[id]` ← `PlaylistMutation::Deleted` broadcast
//!     (success) / `Error` reply (rollback by seq).
//!   - `renaming[id]` ← `PlaylistMutation::Renamed` broadcast
//!     (success) / `Error` reply (rollback by seq).
//!   - `adding[playlist_id]` ← `PlaylistMutation::SongAdded`
//!     broadcast (success — FIFO drop) / `Error` reply (rollback).
//!   - `removing[song_id]` ← `PlaylistMutation::SongRemoved`
//!     broadcast (per-song drop; entry GC'd when its song_id list
//!     empties) / `Error` reply (rollback all song_ids).
//!
//! Cleared on link disconnect (no pending writes against an absent
//! server are meaningful).

use imbl::Vector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCreate {
    /// Seq of the in-flight `CreatePlaylist` request. Identity for
    /// reconciliation (`PlaylistCreated` response or `Error` reply).
    pub seq: u64,
    /// What the user typed; rendered in the placeholder row.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDelete {
    /// Seq of the in-flight `DeletePlaylist` request.
    pub seq: u64,
    /// Server-side playlist id we asked to delete. Used by the view
    /// memo to filter the row out of the visible list, and by ingest
    /// to clear the entry on the matching `Deleted` broadcast.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRename {
    pub seq: u64,
    pub id: String,
    pub new_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAdd {
    pub seq: u64,
    pub playlist_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRemove {
    pub seq: u64,
    pub playlist_id: String,
    /// Song ids the user asked to remove. Each id is dropped as the
    /// matching `SongRemoved` broadcast lands; when the list empties
    /// the whole entry is GC'd.
    pub song_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPlaylists {
    /// Optimistic creates awaiting confirmation. Vector (not HashMap)
    /// because order matters — pending entries render at the bottom
    /// of the playlist list in submission order.
    pub creating: Vector<PendingCreate>,
    /// Optimistic deletes. Filter applied against `playlists.items`
    /// in the view memo.
    pub deleting: Vector<PendingDelete>,
    /// Optimistic renames keyed by playlist id. View memo overrides
    /// the visible name with `new_name` while pending.
    pub renaming: Vector<PendingRename>,
    /// Optimistic adds — one entry per in-flight `AddToPlaylist`
    /// request. View memo on `playlist_tracks` appends a placeholder
    /// "+ Adding…" row when this list has any entry for the loaded
    /// playlist.
    pub adding: Vector<PendingAdd>,
    /// Optimistic removes — view memo filters matching song ids out
    /// of the visible track list.
    pub removing: Vector<PendingRemove>,
}

impl PendingPlaylists {
    pub fn add_creating(&mut self, seq: u64, name: String) {
        self.creating.push_back(PendingCreate { seq, name });
    }

    pub fn add_deleting(&mut self, seq: u64, id: String) {
        self.deleting.push_back(PendingDelete { seq, id });
    }

    /// Drop a creating entry whose seq matches. Returns whether one
    /// was found.
    pub fn remove_creating_by_seq(&mut self, seq: u64) -> bool {
        let before = self.creating.len();
        self.creating.retain(|c| c.seq != seq);
        self.creating.len() != before
    }

    /// Drop a deleting entry whose id matches. Returns whether one
    /// was found.
    pub fn remove_deleting_by_id(&mut self, id: &str) -> bool {
        let before = self.deleting.len();
        self.deleting.retain(|d| d.id != id);
        self.deleting.len() != before
    }

    /// Drop a deleting entry whose seq matches. Used on `Error`
    /// reply to roll back the optimistic removal.
    pub fn remove_deleting_by_seq(&mut self, seq: u64) -> Option<PendingDelete> {
        let pos = self.deleting.iter().position(|d| d.seq == seq)?;
        Some(self.deleting.remove(pos))
    }

    pub fn is_deleting(&self, id: &str) -> bool {
        self.deleting.iter().any(|d| d.id == id)
    }

    // ─── rename ─────────────────────────────────────────────────────

    pub fn add_renaming(&mut self, seq: u64, id: String, new_name: String) {
        // Replace any earlier in-flight rename for the same id — the
        // user's most recent intent wins.
        self.renaming.retain(|r| r.id != id);
        self.renaming.push_back(PendingRename { seq, id, new_name });
    }

    pub fn remove_renaming_by_id(&mut self, id: &str) -> bool {
        let before = self.renaming.len();
        self.renaming.retain(|r| r.id != id);
        self.renaming.len() != before
    }

    pub fn remove_renaming_by_seq(&mut self, seq: u64) -> bool {
        let before = self.renaming.len();
        self.renaming.retain(|r| r.seq != seq);
        self.renaming.len() != before
    }

    /// Optimistic display name for `id`, if a rename is in flight.
    pub fn name_override_for(&self, id: &str) -> Option<&str> {
        self.renaming
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.new_name.as_str())
    }

    // ─── add ────────────────────────────────────────────────────────

    pub fn add_adding(&mut self, seq: u64, playlist_id: String) {
        self.adding.push_back(PendingAdd { seq, playlist_id });
    }

    /// FIFO drop the oldest adding entry for `playlist_id` — used
    /// when a `SongAdded` broadcast lands. Multiple in-flight adds
    /// against the same playlist are processed serially server-side,
    /// so order matches.
    pub fn drop_oldest_adding_for(&mut self, playlist_id: &str) -> bool {
        if let Some(pos) = self
            .adding
            .iter()
            .position(|a| a.playlist_id == playlist_id)
        {
            self.adding.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn remove_adding_by_seq(&mut self, seq: u64) -> bool {
        let before = self.adding.len();
        self.adding.retain(|a| a.seq != seq);
        self.adding.len() != before
    }

    pub fn is_adding_to(&self, playlist_id: &str) -> bool {
        self.adding.iter().any(|a| a.playlist_id == playlist_id)
    }

    // ─── remove ─────────────────────────────────────────────────────

    pub fn add_removing(&mut self, seq: u64, playlist_id: String, song_ids: Vec<String>) {
        if song_ids.is_empty() {
            return;
        }
        self.removing.push_back(PendingRemove {
            seq,
            playlist_id,
            song_ids,
        });
    }

    /// Drop `song_id` from any pending-remove for `playlist_id`. GC
    /// the entry once its list empties. Called on each `SongRemoved`
    /// broadcast.
    pub fn drop_removing_song(&mut self, playlist_id: &str, song_id: &str) {
        for r in self.removing.iter_mut() {
            if r.playlist_id == playlist_id {
                r.song_ids.retain(|s| s != song_id);
            }
        }
        self.removing.retain(|r| !r.song_ids.is_empty());
    }

    pub fn remove_removing_by_seq(&mut self, seq: u64) -> bool {
        let before = self.removing.len();
        self.removing.retain(|r| r.seq != seq);
        self.removing.len() != before
    }

    pub fn is_removing_song(&self, playlist_id: &str, song_id: &str) -> bool {
        self.removing
            .iter()
            .any(|r| r.playlist_id == playlist_id && r.song_ids.iter().any(|s| s == song_id))
    }

    /// Drop everything. Called on link disconnect — pending writes
    /// to an absent server are meaningless.
    pub fn clear(&mut self) {
        *self = Default::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_remove_creating_by_seq() {
        let mut p = PendingPlaylists::default();
        p.add_creating(1, "foo".into());
        p.add_creating(2, "bar".into());
        assert_eq!(p.creating.len(), 2);
        assert!(p.remove_creating_by_seq(1));
        assert_eq!(p.creating.len(), 1);
        assert_eq!(p.creating[0].name, "bar");
        assert!(!p.remove_creating_by_seq(1));
    }

    #[test]
    fn add_and_remove_deleting_by_id_and_seq() {
        let mut p = PendingPlaylists::default();
        p.add_deleting(7, "p1".into());
        p.add_deleting(8, "p2".into());
        assert!(p.is_deleting("p1"));
        assert!(p.remove_deleting_by_id("p1"));
        assert!(!p.is_deleting("p1"));
        let removed = p.remove_deleting_by_seq(8).expect("p2 still pending");
        assert_eq!(removed.id, "p2");
        assert!(p.deleting.is_empty());
    }

    #[test]
    fn clear_resets_all_lists() {
        let mut p = PendingPlaylists::default();
        p.add_creating(1, "a".into());
        p.add_deleting(2, "b".into());
        p.add_renaming(3, "id".into(), "new".into());
        p.add_adding(4, "pl".into());
        p.add_removing(5, "pl".into(), vec!["s1".into()]);
        p.clear();
        assert!(p.creating.is_empty());
        assert!(p.deleting.is_empty());
        assert!(p.renaming.is_empty());
        assert!(p.adding.is_empty());
        assert!(p.removing.is_empty());
    }

    #[test]
    fn rename_replaces_earlier_pending_for_same_id() {
        let mut p = PendingPlaylists::default();
        p.add_renaming(1, "id".into(), "first".into());
        p.add_renaming(2, "id".into(), "second".into());
        assert_eq!(p.renaming.len(), 1);
        assert_eq!(p.name_override_for("id"), Some("second"));
    }

    #[test]
    fn rename_removable_by_id_or_seq() {
        let mut p = PendingPlaylists::default();
        p.add_renaming(1, "a".into(), "A".into());
        p.add_renaming(2, "b".into(), "B".into());
        assert!(p.remove_renaming_by_id("a"));
        assert_eq!(p.renaming.len(), 1);
        assert!(p.remove_renaming_by_seq(2));
        assert!(p.renaming.is_empty());
    }

    #[test]
    fn adding_fifo_per_playlist() {
        let mut p = PendingPlaylists::default();
        p.add_adding(1, "p1".into());
        p.add_adding(2, "p1".into());
        p.add_adding(3, "p2".into());
        assert!(p.drop_oldest_adding_for("p1"));
        assert_eq!(p.adding.len(), 2);
        // The remaining p1 is the seq=2 one, p2 untouched.
        let remaining: Vec<u64> = p.adding.iter().map(|a| a.seq).collect();
        assert_eq!(remaining, vec![2, 3]);
    }

    #[test]
    fn removing_drops_song_then_gcs_empty_entry() {
        let mut p = PendingPlaylists::default();
        p.add_removing(1, "pl".into(), vec!["s1".into(), "s2".into()]);
        assert!(p.is_removing_song("pl", "s1"));
        p.drop_removing_song("pl", "s1");
        assert!(!p.is_removing_song("pl", "s1"));
        assert!(p.is_removing_song("pl", "s2"));
        p.drop_removing_song("pl", "s2");
        assert!(p.removing.is_empty(), "entry GC'd when last song dropped");
    }

    #[test]
    fn removing_by_seq_rolls_back_all_song_ids() {
        let mut p = PendingPlaylists::default();
        p.add_removing(1, "pl".into(), vec!["s1".into(), "s2".into()]);
        assert!(p.remove_removing_by_seq(1));
        assert!(p.removing.is_empty());
    }

    #[test]
    fn add_removing_with_empty_song_list_is_noop() {
        let mut p = PendingPlaylists::default();
        p.add_removing(1, "pl".into(), vec![]);
        assert!(p.removing.is_empty());
    }
}
