//! External-fact source: streaming search results.
//!
//! The server replies to a `Search` request with a `Search(...)` on
//! the originating seq carrying the first page, then streams further
//! pages as seq=0 `SearchMore(...)` broadcasts that share the same
//! `task_id`. We fold both into one `Search` instance keyed by
//! `task_id` so the UI can render a single growing list.
//!
//! Only one search is tracked at a time. Starting a new search
//! replaces the previous one (matching legacy behaviour where the
//! results pane only ever shows the most recent query).

use std::sync::Arc;

use imbl::Vector;
use mkproto::{Album, Artist, SearchResults, SearchType, Song, TaskId};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub task_id: Option<TaskId>,
    pub term: Arc<str>,
    pub search_type: SearchType,
    pub songs: Vector<Arc<Song>>,
    pub albums: Vector<Arc<Album>>,
    pub artists: Vector<Arc<Artist>>,
    /// First-page reply has been observed. Until then the UI shows
    /// a "searching…" spinner.
    pub first_page_received: bool,
    /// The request this search is waiting on died with the connection.
    /// Set on close while a first page was still outstanding; the
    /// refetch lifecycle re-issues the search once the link is back.
    /// Cleared by `begin` and by the re-issue.
    pub stale: bool,
    /// Server reported it has streamed everything for this task.
    pub completed: bool,
    /// One-shot gate for the legacy "reopen the search modal when
    /// the first page arrives empty" behaviour (mkp2
    /// `app/server.rs::ServerMsg::Search` handler). Reset in
    /// `begin()`; set to `true` by the search-reopen lifecycle
    /// trampoline once it has reopened the modal so dismissing
    /// (Esc) doesn't loop us back into a re-open.
    pub empty_reopen_done: bool,
}

impl Search {
    /// Begin a new search. Clears any prior accumulated results.
    pub fn begin(&mut self, task_id: TaskId, term: Arc<str>, search_type: SearchType) {
        self.stale = false;
        *self = Self {
            task_id: Some(task_id),
            term,
            search_type,
            ..Default::default()
        };
    }

    /// Fold the first-page reply (the one tied to a non-zero seq).
    pub fn set_first_page(&mut self, task_id: TaskId, results: SearchResults) {
        if self.task_id != Some(task_id) {
            return;
        }
        self.absorb(results);
        self.first_page_received = true;
    }

    /// Fold a streamed `SearchMore` page. Ignored when the task id
    /// doesn't match the current search.
    pub fn append(&mut self, task_id: TaskId, results: SearchResults) {
        if self.task_id != Some(task_id) {
            return;
        }
        self.absorb(results);
    }

    fn absorb(&mut self, results: SearchResults) {
        match results {
            SearchResults::Songs { songs } => {
                self.songs.extend(songs.into_iter().map(Arc::new));
            }
            SearchResults::Albums { albums } => {
                self.albums.extend(albums.into_iter().map(Arc::new));
            }
            SearchResults::Artists { artists } => {
                self.artists.extend(artists.into_iter().map(Arc::new));
            }
        }
    }

    pub fn mark_completed(&mut self, task_id: TaskId) {
        if self.task_id == Some(task_id) {
            self.completed = true;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_active(&self) -> bool {
        self.task_id.is_some()
    }

    /// `true` once the first page has arrived and *all* result
    /// collections are empty — the trigger condition for the
    /// "reopen modal on empty results" lifecycle.
    pub fn first_page_empty(&self) -> bool {
        self.first_page_received
            && self.songs.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
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

    #[test]
    fn begin_resets_state() {
        let mut s = Search::default();
        s.songs.push_back(Arc::new(song("a")));
        s.first_page_received = true;
        s.completed = true;
        s.begin(7, "term".into(), SearchType::Song);
        assert_eq!(s.task_id, Some(7));
        assert_eq!(&*s.term, "term");
        assert!(s.songs.is_empty());
        assert!(!s.first_page_received);
        assert!(!s.completed);
    }

    #[test]
    fn set_first_page_marks_received() {
        let mut s = Search::default();
        s.begin(1, "x".into(), SearchType::Song);
        s.set_first_page(
            1,
            SearchResults::Songs {
                songs: vec![song("a"), song("b")],
            },
        );
        assert!(s.first_page_received);
        assert_eq!(s.songs.len(), 2);
    }

    #[test]
    fn append_grows_when_task_id_matches() {
        let mut s = Search::default();
        s.begin(1, "x".into(), SearchType::Song);
        s.set_first_page(
            1,
            SearchResults::Songs {
                songs: vec![song("a")],
            },
        );
        s.append(
            1,
            SearchResults::Songs {
                songs: vec![song("b"), song("c")],
            },
        );
        assert_eq!(s.songs.len(), 3);
    }

    #[test]
    fn append_ignores_mismatched_task_id() {
        let mut s = Search::default();
        s.begin(1, "x".into(), SearchType::Song);
        s.set_first_page(
            1,
            SearchResults::Songs {
                songs: vec![song("a")],
            },
        );
        // Stale page from a previous search.
        s.append(
            99,
            SearchResults::Songs {
                songs: vec![song("b")],
            },
        );
        assert_eq!(s.songs.len(), 1);
    }

    #[test]
    fn mark_completed_only_for_matching_task_id() {
        let mut s = Search::default();
        s.begin(1, "x".into(), SearchType::Song);
        s.mark_completed(99);
        assert!(!s.completed);
        s.mark_completed(1);
        assert!(s.completed);
    }

    #[test]
    fn clear_resets_everything() {
        let mut s = Search::default();
        s.begin(1, "x".into(), SearchType::Song);
        s.set_first_page(
            1,
            SearchResults::Songs {
                songs: vec![song("a")],
            },
        );
        s.clear();
        assert!(!s.is_active());
        assert!(s.songs.is_empty());
        assert!(!s.first_page_received);
    }
}
