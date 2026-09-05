//! View model for the middle column's title block + (optionally)
//! the shared `Title / Artist / Album / Time` row header.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The header memo only
//! depends on small projected scalars (lengths, optional durations,
//! mode + history flags), so per-source `drv::Input`s are the right
//! shape: heavy payloads stay on their own per-body memos.

use std::sync::Arc;

use imbl::Vector;
use mkproto::{SearchType, ServerMsg, Song};

use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_responses::Responses;
use mkpclient_state_search::Search;

use super::util::format_duration;

/// drv-friendly mirror of `mkproto::SearchType`. Foreign enums
/// don't carry `drv::Input` (per guideline 11 mkproto stays drv-
/// free), so we round-trip through a local copy with the same
/// shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, drv::Input)]
pub enum SearchKind {
    Song,
    Album,
    Artist,
}

impl From<SearchType> for SearchKind {
    fn from(s: SearchType) -> Self {
        match s {
            SearchType::Song => SearchKind::Song,
            SearchType::Album => SearchKind::Album,
            SearchType::Artist => SearchKind::Artist,
        }
    }
}

/// Which middle mode is showing — drives title text, total-secs
/// computation, and whether the row header is rendered.
///
/// `SearchResults` carries the search `term` so the title can render
/// `Search: {term} ({type}) — {count}` matching legacy parity. The
/// term flows through from `state-ui-history::MiddleMode` at the
/// projection site (`render::run_render` / `tui::render::draw`).
#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum MiddleMode {
    PlaylistSongs,
    SearchResults {
        search_type: SearchKind,
        term: Arc<str>,
    },
    AlbumDetail {
        awaiting_seq: Option<u64>,
    },
    ArtistDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ColumnWidths {
    pub title: usize,
    pub artist: usize,
    pub album: usize,
    pub time: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MiddleHeaderModel {
    pub focused: bool,
    pub title: Arc<str>,
    pub in_selection: bool,
    pub can_back: bool,
    pub can_fwd: bool,
    /// `true` when the renderer should show the bottom-left
    /// "Shift-F unfilter" hint (focused AND filter not empty).
    pub show_unfilter_hint: bool,
    /// Pre-formatted "{duration}" badge for the bottom-right, or
    /// `None` to omit.
    pub total_duration: Option<Arc<str>>,
    /// Whether the standard `Title / Artist / Album / Time` row
    /// should render. Only PlaylistSongs / AlbumDetail / SearchResults-Songs
    /// use it.
    pub render_track_header: bool,
}

#[derive(drv::Input)]
pub struct SearchCountsInput {
    pub songs_len: usize,
    pub albums_len: usize,
    pub artists_len: usize,
    pub first_page_received: bool,
}

impl SearchCountsInput {
    pub fn new(s: &Search) -> Self {
        Self {
            songs_len: s.songs.len(),
            albums_len: s.albums.len(),
            artists_len: s.artists.len(),
            first_page_received: s.first_page_received,
        }
    }
}

#[derive(drv::Input)]
pub struct PlaylistTracksDurationInput<'a> {
    pub songs: &'a Vector<Option<Arc<Song>>>,
}

impl<'a> PlaylistTracksDurationInput<'a> {
    pub fn new(t: &'a PlaylistTracks) -> Self {
        Self { songs: &t.songs }
    }
}

/// User-decision UI knobs the middle-header memo reads. Bundled
/// per the "always bundle, never `#[allow(too_many_arguments)]`"
/// discipline.
#[derive(drv::Input)]
pub struct MiddleHeaderUiInput {
    pub focused: bool,
    pub in_selection: bool,
    pub middle_filter_empty: bool,
    pub history_back_count: usize,
    pub history_fwd_count: usize,
}

#[drv::memo(single)]
pub fn middle_header_model<'a>(
    mode: MiddleMode,
    search: SearchCountsInput,
    tracks: PlaylistTracksDurationInput<'a>,
    album_detail_total_secs: f32,
    ui: MiddleHeaderUiInput,
) -> MiddleHeaderModel {
    let title: Arc<str> = match &mode {
        MiddleMode::PlaylistSongs => Arc::from("Playlist"),
        MiddleMode::SearchResults { search_type, term } => {
            let count = match search_type {
                SearchKind::Song => search.songs_len,
                SearchKind::Album => search.albums_len,
                SearchKind::Artist => search.artists_len,
            };
            let type_label = match search_type {
                SearchKind::Song => "song",
                SearchKind::Album => "album",
                SearchKind::Artist => "artist",
            };
            if search.first_page_received && count == 0 {
                Arc::from(format!("Search: {term} ({type_label}) — No results"))
            } else if search.first_page_received {
                Arc::from(format!("Search: {term} ({type_label}) — {count}"))
            } else {
                // Still streaming — first page hasn't landed yet.
                Arc::from(format!("Search: {term} ({type_label})"))
            }
        }
        MiddleMode::AlbumDetail { .. } => Arc::from("Album"),
        MiddleMode::ArtistDetail => Arc::from("Artist"),
    };

    let total_secs: f32 = match &mode {
        MiddleMode::PlaylistSongs => tracks.songs.iter().flatten().map(|s| s.duration).sum(),
        MiddleMode::AlbumDetail { .. } => album_detail_total_secs,
        _ => 0.0,
    };
    let total_duration: Option<Arc<str>> = if total_secs > 0.0 {
        Some(Arc::from(format_duration(total_secs)))
    } else {
        None
    };

    // Only PlaylistSongs uses the shared `Title / Artist / Album /
    // Time` header above the list. AlbumDetail paints its own
    // numbered-track header; SearchResults bodies (Song / Album /
    // Artist) each own their layout so they can suppress the
    // header in Searching / NoResults states (legacy parity).
    let render_track_header = matches!(&mode, MiddleMode::PlaylistSongs);

    MiddleHeaderModel {
        focused: ui.focused,
        title,
        in_selection: ui.in_selection,
        can_back: ui.history_back_count > 0,
        can_fwd: ui.history_fwd_count > 0,
        show_unfilter_hint: ui.focused && !ui.middle_filter_empty,
        total_duration,
        render_track_header,
    }
}

/// Helper for callers: read the album-detail response and sum song
/// durations, returning 0 when there's no matching response.
pub fn album_detail_total_secs(awaiting_seq: Option<u64>, responses: &Responses) -> f32 {
    awaiting_seq
        .and_then(|seq| responses.by_seq.get(&seq))
        .and_then(|r| match r.as_ref() {
            ServerMsg::AlbumDetail { songs, .. } => {
                Some(songs.iter().map(|s| s.duration).sum::<f32>())
            }
            _ => None,
        })
        .unwrap_or(0.0)
}

/// Compute the standard column widths from `inner.width` for modes
/// that use the `Title / Artist / Album / Time` layout. Mirrors the
/// pre-rewrite arithmetic in `draw_tracks_col`.
pub fn column_widths(inner_w: u16) -> ColumnWidths {
    let w = inner_w as usize;
    let time = 6usize;
    let rest = w.saturating_sub(time + 2);
    let title = rest * 35 / 100;
    let artist = rest * 25 / 100;
    let album = rest.saturating_sub(title + artist);
    ColumnWidths {
        title,
        artist,
        album,
        time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mkpclient_state_playlist_tracks::PlaylistTracks;
    use mkpclient_state_search::Search;

    fn run(
        mode: MiddleMode,
        focused: bool,
        in_selection: bool,
        filter_empty: bool,
        back: usize,
        fwd: usize,
        first_page: bool,
    ) -> MiddleHeaderModel {
        let search = Search {
            first_page_received: first_page,
            ..Default::default()
        };
        let tracks = PlaylistTracks::default();
        middle_header_model(
            mode,
            SearchCountsInput::new(&search),
            PlaylistTracksDurationInput::new(&tracks),
            0.0,
            MiddleHeaderUiInput {
                focused,
                in_selection,
                middle_filter_empty: filter_empty,
                history_back_count: back,
                history_fwd_count: fwd,
            },
        )
    }

    #[test]
    fn playlist_songs_basic() {
        let m = run(MiddleMode::PlaylistSongs, true, false, true, 0, 0, false);
        assert_eq!(&*m.title, "Playlist");
        assert!(m.render_track_header);
        assert_eq!(m.total_duration, None);
        assert!(!m.show_unfilter_hint);
    }

    #[test]
    fn search_no_results_after_first_page_says_no_results() {
        let m = run(
            MiddleMode::SearchResults {
                search_type: SearchKind::Song,
                term: Arc::from("love"),
            },
            true,
            false,
            true,
            0,
            0,
            true,
        );
        assert_eq!(&*m.title, "Search: love (song) — No results");
    }

    #[test]
    fn search_with_results_includes_term_type_and_count() {
        // Three songs in flight, first page received → full title.
        let search = Search {
            first_page_received: true,
            songs: imbl::Vector::from(vec![
                std::sync::Arc::new(Song {
                    id: "a".into(),
                    title: "Alpha".into(),
                    artist_name: "x".into(),
                    album_title: "y".into(),
                    duration: 0.0,
                    track_number: None,
                    url: None,
                    artwork_url_small: None,
                    artwork_url_large: None,
                }),
                std::sync::Arc::new(Song {
                    id: "b".into(),
                    title: "Bravo".into(),
                    artist_name: "x".into(),
                    album_title: "y".into(),
                    duration: 0.0,
                    track_number: None,
                    url: None,
                    artwork_url_small: None,
                    artwork_url_large: None,
                }),
                std::sync::Arc::new(Song {
                    id: "c".into(),
                    title: "Charlie".into(),
                    artist_name: "x".into(),
                    album_title: "y".into(),
                    duration: 0.0,
                    track_number: None,
                    url: None,
                    artwork_url_small: None,
                    artwork_url_large: None,
                }),
            ]),
            ..Default::default()
        };
        let tracks = PlaylistTracks::default();
        let m = middle_header_model(
            MiddleMode::SearchResults {
                search_type: SearchKind::Song,
                term: Arc::from("love"),
            },
            SearchCountsInput::new(&search),
            PlaylistTracksDurationInput::new(&tracks),
            0.0,
            MiddleHeaderUiInput {
                focused: true,
                in_selection: false,
                middle_filter_empty: true,
                history_back_count: 0,
                history_fwd_count: 0,
            },
        );
        assert_eq!(&*m.title, "Search: love (song) — 3");
    }

    #[test]
    fn search_streaming_drops_count_until_first_page() {
        let m = run(
            MiddleMode::SearchResults {
                search_type: SearchKind::Album,
                term: Arc::from("foo"),
            },
            true,
            false,
            true,
            0,
            0,
            false,
        );
        assert_eq!(&*m.title, "Search: foo (album)");
    }

    #[test]
    fn artist_detail_skips_track_header() {
        let m = run(MiddleMode::ArtistDetail, false, false, true, 0, 0, false);
        assert_eq!(&*m.title, "Artist");
        assert!(!m.render_track_header);
    }

    #[test]
    fn unfilter_hint_only_when_focused_and_filter_active() {
        let focused_active = run(MiddleMode::PlaylistSongs, true, false, false, 0, 0, false);
        assert!(focused_active.show_unfilter_hint);

        let unfocused_active = run(MiddleMode::PlaylistSongs, false, false, false, 0, 0, false);
        assert!(!unfocused_active.show_unfilter_hint);

        let focused_empty = run(MiddleMode::PlaylistSongs, true, false, true, 0, 0, false);
        assert!(!focused_empty.show_unfilter_hint);
    }

    #[test]
    fn column_widths_sum_under_inner_width() {
        let w = column_widths(120);
        let sum = w.title + 1 + w.artist + 1 + w.album + w.time;
        assert_eq!(sum, 120);
    }

    #[test]
    fn history_arrows_track_counts() {
        let neither = run(MiddleMode::PlaylistSongs, false, false, true, 0, 0, false);
        assert!(!neither.can_back && !neither.can_fwd);
        let only_back = run(MiddleMode::PlaylistSongs, false, false, true, 1, 0, false);
        assert!(only_back.can_back && !only_back.can_fwd);
        let both = run(MiddleMode::PlaylistSongs, false, false, true, 1, 1, false);
        assert!(both.can_back && both.can_fwd);
    }
}
