//! View model for the middle pane's `MiddleMode::SearchResults`
//! body. Three subshapes (Song / Album / Artist) plus searching/
//! no-results status.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The search source
//! holds `imbl::Vector<Arc<_>>` payloads, so cache-hit checks are
//! O(1) on stable rows.

use std::sync::Arc;

use imbl::{OrdSet, Vector};
use mkproto::{Album, Artist, Song};

use mkpclient_state_search::Search;

use super::util::format_duration;

/// drv-friendly mirror of `mkproto::SearchType`.
pub use super::middle_header::SearchKind;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchSongRow {
    pub orig_index: usize,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_str: String,
    pub is_multi_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchAlbumRow {
    pub orig_index: usize,
    pub name: String,
    pub artist: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchArtistRow {
    pub orig_index: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SearchResultsState {
    Searching,
    NoResults,
    Songs { rows: Vector<SearchSongRow> },
    Albums { rows: Vector<SearchAlbumRow> },
    Artists { rows: Vector<SearchArtistRow> },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResultsBodyModel {
    pub state: SearchResultsState,
    pub selected_filtered: usize,
    pub focused: bool,
    /// `true` when the multi-select context targets the middle pane.
    /// The painter uses it to decide whether to draw the magenta `❯`
    /// prefix and the pink cursor styling.
    pub in_selection: bool,
}

#[derive(drv::Input)]
pub struct SearchResultsInput<'a> {
    pub first_page_received: bool,
    pub search_kind: SearchKind,
    pub songs: &'a Vector<Arc<Song>>,
    pub albums: &'a Vector<Arc<Album>>,
    pub artists: &'a Vector<Arc<Artist>>,
}

impl<'a> SearchResultsInput<'a> {
    pub fn new(s: &'a Search) -> Self {
        Self {
            first_page_received: s.first_page_received,
            search_kind: s.search_type.into(),
            songs: &s.songs,
            albums: &s.albums,
            artists: &s.artists,
        }
    }
}

#[drv::memo(single)]
pub fn search_results_body_model<'a>(
    search: SearchResultsInput<'a>,
    middle_filter: &Arc<str>,
    middle_selected: usize,
    focused: bool,
    selection_in_middle: bool,
    selection_indices: &OrdSet<usize>,
) -> SearchResultsBodyModel {
    if !search.first_page_received {
        return SearchResultsBodyModel {
            state: SearchResultsState::Searching,
            selected_filtered: middle_selected,
            focused,
            in_selection: selection_in_middle,
        };
    }

    let filter_lower = middle_filter.to_lowercase();
    let all_match = filter_lower.is_empty();

    let state = match search.search_kind {
        SearchKind::Song => {
            let mut rows: Vector<SearchSongRow> = Vector::new();
            for (orig_index, s) in search.songs.iter().enumerate() {
                if !all_match {
                    let m = s.title.to_lowercase().contains(&filter_lower)
                        || s.artist_name.to_lowercase().contains(&filter_lower)
                        || s.album_title.to_lowercase().contains(&filter_lower);
                    if !m {
                        continue;
                    }
                }
                rows.push_back(SearchSongRow {
                    orig_index,
                    title: s.title.clone(),
                    artist: s.artist_name.clone(),
                    album: s.album_title.clone(),
                    duration_str: format_duration(s.duration),
                    is_multi_selected: selection_in_middle
                        && selection_indices.contains(&orig_index),
                });
            }
            if rows.is_empty() {
                SearchResultsState::NoResults
            } else {
                SearchResultsState::Songs { rows }
            }
        }
        SearchKind::Album => {
            let mut rows: Vector<SearchAlbumRow> = Vector::new();
            for (orig_index, a) in search.albums.iter().enumerate() {
                if !all_match {
                    let m = a.name.to_lowercase().contains(&filter_lower)
                        || a.artist_name.to_lowercase().contains(&filter_lower);
                    if !m {
                        continue;
                    }
                }
                rows.push_back(SearchAlbumRow {
                    orig_index,
                    name: a.name.clone(),
                    artist: a.artist_name.clone(),
                    track_count: a.track_count as u32,
                });
            }
            if rows.is_empty() {
                SearchResultsState::NoResults
            } else {
                SearchResultsState::Albums { rows }
            }
        }
        SearchKind::Artist => {
            let mut rows: Vector<SearchArtistRow> = Vector::new();
            for (orig_index, a) in search.artists.iter().enumerate() {
                if !all_match && !a.name.to_lowercase().contains(&filter_lower) {
                    continue;
                }
                rows.push_back(SearchArtistRow {
                    orig_index,
                    name: a.name.clone(),
                });
            }
            if rows.is_empty() {
                SearchResultsState::NoResults
            } else {
                SearchResultsState::Artists { rows }
            }
        }
    };

    SearchResultsBodyModel {
        state,
        selected_filtered: middle_selected,
        focused,
        in_selection: selection_in_middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkproto::{Album, Artist, SearchResults, SearchType, Song};

    fn song(t: &str, a: &str, dur: f32) -> Song {
        Song {
            id: t.into(),
            title: t.into(),
            artist_name: a.into(),
            album_title: "Al".into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn album(name: &str, artist: &str, tracks: usize) -> Album {
        Album {
            id: name.into(),
            name: name.into(),
            artist_id: "x".into(),
            artist_name: artist.into(),
            track_count: tracks,
            detail: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn artist(name: &str) -> Artist {
        Artist {
            id: name.into(),
            name: name.into(),
            detail: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn make_search<F>(t: SearchType, fill: F) -> Search
    where
        F: FnOnce(&mut Search),
    {
        let mut s = Search::default();
        s.begin(1, "q".into(), t);
        fill(&mut s);
        s.first_page_received = true;
        s
    }

    fn run(s: &Search, filter: &str, sel: usize, focused: bool) -> SearchResultsBodyModel {
        let filter_arc: Arc<str> = Arc::from(filter);
        search_results_body_model(
            SearchResultsInput::new(s),
            &filter_arc,
            sel,
            focused,
            false,
            &OrdSet::new(),
        )
    }

    #[test]
    fn searching_when_first_page_not_received() {
        let mut s = Search::default();
        s.begin(1, "q".into(), SearchType::Song);
        let m = run(&s, "", 0, false);
        assert_eq!(m.state, SearchResultsState::Searching);
    }

    #[test]
    fn empty_results_after_first_page_yields_no_results() {
        let s = make_search(SearchType::Song, |_| {});
        let m = run(&s, "", 0, false);
        assert_eq!(m.state, SearchResultsState::NoResults);
    }

    #[test]
    fn song_search_renders_song_rows() {
        let s = make_search(SearchType::Song, |s| {
            s.set_first_page(
                1,
                SearchResults::Songs {
                    songs: vec![song("Alpha", "x", 60.0), song("Beta", "y", 30.0)],
                },
            );
        });
        let m = run(&s, "", 0, false);
        if let SearchResultsState::Songs { rows } = m.state {
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].title, "Alpha");
            assert_eq!(rows[1].duration_str, "00:30");
        } else {
            panic!("expected Songs");
        }
    }

    #[test]
    fn album_search_filters_by_name_or_artist() {
        let s = make_search(SearchType::Album, |s| {
            s.set_first_page(
                1,
                SearchResults::Albums {
                    albums: vec![
                        album("Abbey Road", "Beatles", 17),
                        album("Tapestry", "King", 12),
                        album("Beatles 1962", "Beatles", 27),
                    ],
                },
            );
        });
        let m = run(&s, "beatles", 0, false);
        if let SearchResultsState::Albums { rows } = m.state {
            assert_eq!(rows.len(), 2);
        } else {
            panic!("expected Albums");
        }
    }

    #[test]
    fn artist_search_renders_simple_rows() {
        let s = make_search(SearchType::Artist, |s| {
            s.set_first_page(
                1,
                SearchResults::Artists {
                    artists: vec![artist("Beatles"), artist("Stones"), artist("Zeppelin")],
                },
            );
        });
        let m = run(&s, "", 0, false);
        if let SearchResultsState::Artists { rows } = m.state {
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].name, "Beatles");
        } else {
            panic!("expected Artists");
        }
    }

    #[test]
    fn filter_to_zero_yields_no_results() {
        let s = make_search(SearchType::Song, |s| {
            s.set_first_page(
                1,
                SearchResults::Songs {
                    songs: vec![song("Alpha", "x", 0.0)],
                },
            );
        });
        let m = run(&s, "zzz", 0, false);
        assert_eq!(m.state, SearchResultsState::NoResults);
    }
}
