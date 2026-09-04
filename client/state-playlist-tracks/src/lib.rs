//! External-fact source: streaming tracks of the actively-viewed
//! playlist.
//!
//! Wire protocol: when the user picks a playlist, the client sends
//! `ClientMsg::GetPlaylist { id, focus }` as a regular request. The
//! server replies with broadcast (seq=0) frames:
//!
//!   - `ServerMsg::ListBegin { target: Playlist{id}, total, focus }`
//!     — sizes a sparse slot vector and records the initially-focused
//!     row (usually the currently-playing song, so the UI can jump
//!     the cursor there).
//!   - `ServerMsg::ListChunk { target: Playlist{id}, offset, songs }`
//!     — fills song slots starting at `offset`.
//!
//! Ingest drops chunks whose `target.id` doesn't match the currently
//! viewed playlist (old chunks from a previous selection).

use std::sync::Arc;

use imbl::Vector;
use mkproto::{Song, TaskId};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaylistTracks {
    /// id of the playlist currently viewed; `None` means "no playlist
    /// picked yet, middle pane shows placeholder".
    pub playlist_id: Option<Arc<str>>,
    /// Size announced by `ListBegin`. May be 0 before the first frame
    /// arrives.
    pub total: usize,
    /// Server-suggested initial cursor row (usually the now-playing
    /// track's index inside this playlist).
    pub focus: usize,
    /// Sparse slot vector: `songs[i] == None` means "chunk not yet
    /// received for index i". Length equals `total` once `ListBegin`
    /// has been folded.
    pub songs: Vector<Option<Arc<Song>>>,
    /// `task_id` of the in-flight `GetPlaylist` whose `ListBegin` /
    /// `ListChunk` broadcasts we're awaiting. The streaming protocol
    /// correlates frames via `task_id` (seq=0 on the broadcast); this
    /// lets the refetch lifecycle gate "is a load already in flight?"
    /// without inventing parallel bookkeeping. Also used by
    /// `is_ready()` to derive "ListBegin has landed."
    pub pending_task: Option<TaskId>,
    /// Marked true when a `PlaylistMutation::Modified` broadcast
    /// arrives whose `playlist_id` matches `self.playlist_id`. The
    /// per-playlist refetch lifecycle reads this; `begin()` clears it
    /// once the fresh `ListBegin` lands.
    pub stale: bool,
}

impl PlaylistTracks {
    /// Start a new streaming load. Resets everything, sizes `songs`
    /// to `total` empty slots, and clears `pending_task` / `stale` —
    /// the data is now arriving, so the refetch gate is closed.
    /// Clearing `pending_task` is also what flips `is_ready()` true.
    pub fn begin(&mut self, playlist_id: Arc<str>, total: usize, focus: usize) {
        self.playlist_id = Some(playlist_id);
        self.total = total;
        self.focus = focus;
        self.songs = std::iter::repeat_with(|| None).take(total).collect();
        self.pending_task = None;
        self.stale = false;
    }

    /// True iff `ListBegin` has landed for the loaded playlist —
    /// derived from "playlist picked AND no in-flight load."
    /// Replaces a former stored `ready: bool` field.
    pub fn is_ready(&self) -> bool {
        self.playlist_id.is_some() && self.pending_task.is_none()
    }

    /// Apply a chunk starting at `offset`. Clamps overruns; trailing
    /// slots past `total` are ignored (they'd be a server bug).
    pub fn chunk(&mut self, offset: usize, songs: Vec<Song>) {
        for (i, song) in songs.into_iter().enumerate() {
            let idx = offset + i;
            if idx < self.songs.len() {
                self.songs[idx] = Some(Arc::new(song));
            }
        }
    }

    /// Splice out the song at `index` if `id` matches the loaded
    /// playlist. No-op otherwise. Adjusts `total` and `focus` to stay
    /// in bounds.
    pub fn remove_at(&mut self, id: &str, index: usize) {
        if self.playlist_id.as_deref() != Some(id) {
            return;
        }
        if index >= self.songs.len() {
            return;
        }
        self.songs.remove(index);
        self.total = self.total.saturating_sub(1);
        if self.focus > index {
            self.focus = self.focus.saturating_sub(1);
        }
        if self.focus >= self.total && self.total > 0 {
            self.focus = self.total - 1;
        }
    }

    /// Append `songs` to the loaded playlist if `id` matches. No-op
    /// otherwise. Bumps `total` accordingly.
    pub fn extend(&mut self, id: &str, songs: Vec<Song>) {
        if self.playlist_id.as_deref() != Some(id) {
            return;
        }
        for song in songs {
            self.songs.push_back(Some(Arc::new(song)));
        }
        self.total = self.songs.len();
    }

    /// Drop everything — called when the link closes or the backend
    /// swaps.
    pub fn clear(&mut self) {
        *self = Default::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            title: id.into(),
            artist_name: String::new(),
            album_title: String::new(),
            duration: 0.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn loaded(id: &str, ids: &[&str]) -> PlaylistTracks {
        let mut t = PlaylistTracks::default();
        t.begin(id.into(), ids.len(), 0);
        t.chunk(0, ids.iter().map(|i| song(i)).collect());
        t
    }

    #[test]
    fn remove_at_splices_when_id_matches() {
        let mut t = loaded("p1", &["a", "b", "c"]);
        t.remove_at("p1", 1);
        assert_eq!(t.total, 2);
        assert_eq!(t.songs[0].as_ref().unwrap().id, "a");
        assert_eq!(t.songs[1].as_ref().unwrap().id, "c");
    }

    #[test]
    fn remove_at_noop_when_id_mismatch() {
        let mut t = loaded("p1", &["a", "b", "c"]);
        t.remove_at("other", 1);
        assert_eq!(t.total, 3);
    }

    #[test]
    fn remove_at_clamps_focus() {
        let mut t = loaded("p1", &["a", "b", "c"]);
        t.focus = 2;
        t.remove_at("p1", 0);
        assert_eq!(t.focus, 1);
        t.remove_at("p1", 1);
        assert_eq!(t.focus, 0);
    }

    #[test]
    fn extend_appends_when_id_matches() {
        let mut t = loaded("p1", &["a"]);
        t.extend("p1", vec![song("b"), song("c")]);
        assert_eq!(t.total, 3);
        assert_eq!(t.songs[2].as_ref().unwrap().id, "c");
    }

    #[test]
    fn extend_noop_when_id_mismatch() {
        let mut t = loaded("p1", &["a"]);
        t.extend("other", vec![song("b")]);
        assert_eq!(t.total, 1);
    }
}
