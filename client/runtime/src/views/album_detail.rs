//! View model for the middle pane's `MiddleMode::AlbumDetail`
//! body.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The album-detail
//! response lives in `state-responses` as `Arc<ServerMsg>`, so the
//! input projection just hands the memo the relevant `Arc` —
//! cache-hits ptr-eq the `Arc` and run no work.
//!
//! Album-detail repeats the same artist/album on every row, so
//! unlike the playlist view it does NOT use the shared
//! `Title / Artist / Album / Time` column layout. The legacy
//! renderer (mkp2 `nav/player/middle.rs::draw_album_detail`) paints:
//!
//! - Top metadata: album name (bold), artist (cyan), year (dim),
//!   blank, wrapped editorial notes (dim), blank.
//! - Single-line "    Title    Time" header.
//! - Numbered tracks ("nn  Title …  mm:ss").
//! - Footer: blank, record label (dim), copyright (dim).
//!
//! All payload strings are `Arc<str>` so cache-hit clones are
//! refcount bumps; the row vector is `imbl::Vector` so structural
//! sharing survives the clone.
//!
//! The album header is wrapped in `Arc<AlbumHeader>` so model
//! clones don't deep-copy the metadata block on every cache hit.

use std::sync::Arc;

use imbl::{OrdSet, Vector};

use mkpclient_state_responses::Responses;
use mkproto::ServerMsg;

use super::util::format_duration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AlbumHeader {
    pub name: Arc<str>,
    pub artist: Arc<str>,
    pub year: Option<Arc<str>>,
    /// Editorial notes pre-wrapped to body width by the memo. Empty
    /// when the response carries no notes. Kept as
    /// `Vector<Arc<str>>` so cache-hit clones are refcount bumps and
    /// the painter can iterate without re-wrapping per frame.
    pub notes_lines: Vector<Arc<str>>,
    pub record_label: Option<Arc<str>>,
    pub copyright: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AlbumDetailRow {
    pub orig_index: usize,
    pub track_number: Option<u32>,
    pub title: Arc<str>,
    pub duration_str: Arc<str>,
    pub is_multi_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum AlbumDetailState {
    /// Awaiting the response (`awaiting_seq` is `None` or its
    /// response slot is empty).
    Loading,
    Tracks {
        header: Arc<AlbumHeader>,
        rows: Vector<AlbumDetailRow>,
        /// Any track exceeds 99:59 — duration column widens to
        /// `HH:MM:SS`. Computed in the memo so the painter doesn't
        /// re-scan rows every frame.
        use_hours: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AlbumDetailBodyModel {
    pub state: AlbumDetailState,
    pub selected_filtered: usize,
    pub focused: bool,
    /// `true` when the multi-select context targets the middle pane.
    /// The painter uses it to draw the magenta `❯` prefix on
    /// multi-selected rows and the pink cursor styling.
    pub in_selection: bool,
}

#[derive(drv::Input)]
pub struct AlbumDetailResponseInput {
    pub response: Option<Arc<ServerMsg>>,
}

impl AlbumDetailResponseInput {
    pub fn new(awaiting_seq: Option<u64>, responses: &Responses) -> Self {
        Self {
            response: awaiting_seq
                .and_then(|seq| responses.by_seq.get(&seq))
                .cloned(),
        }
    }
}

#[drv::memo(single)]
pub fn album_detail_body_model(
    response: AlbumDetailResponseInput,
    middle_filter: &Arc<str>,
    middle_selected: usize,
    focused: bool,
    body_width: u16,
    selection_in_middle: bool,
    selection_indices: &OrdSet<usize>,
) -> AlbumDetailBodyModel {
    let (album, songs) = match response.response.as_deref() {
        Some(ServerMsg::AlbumDetail { album, songs }) => (album, songs),
        _ => {
            return AlbumDetailBodyModel {
                state: AlbumDetailState::Loading,
                selected_filtered: middle_selected,
                focused,
                in_selection: selection_in_middle,
            };
        }
    };

    let notes_lines: Vector<Arc<str>> = album
        .detail
        .as_ref()
        .and_then(|d| d.editorial_notes_short.as_ref())
        .filter(|s| !s.is_empty())
        .map(|s| {
            wrap_text(s, body_width as usize)
                .into_iter()
                .map(Arc::from)
                .collect()
        })
        .unwrap_or_default();

    let header = Arc::new(AlbumHeader {
        name: Arc::from(album.name.as_str()),
        artist: Arc::from(album.artist_name.as_str()),
        year: album.detail.as_ref().and_then(|d| {
            d.release_date.as_ref().map(|date| {
                let y = date.split('-').next().unwrap_or(date);
                Arc::from(y)
            })
        }),
        notes_lines,
        record_label: album
            .detail
            .as_ref()
            .and_then(|d| d.record_label.as_ref().map(|s| Arc::from(s.as_str()))),
        copyright: album
            .detail
            .as_ref()
            .and_then(|d| d.copyright.as_ref().map(|s| Arc::from(s.as_str()))),
    });

    let use_hours = songs.iter().any(|s| s.duration as u64 > 5999);

    let filter_lower = middle_filter.to_lowercase();
    let all_match = filter_lower.is_empty();
    let mut rows: Vector<AlbumDetailRow> = Vector::new();
    for (orig_index, s) in songs.iter().enumerate() {
        if !all_match {
            let m = s.title.to_lowercase().contains(&filter_lower)
                || s.artist_name.to_lowercase().contains(&filter_lower);
            if !m {
                continue;
            }
        }
        rows.push_back(AlbumDetailRow {
            orig_index,
            track_number: s.track_number,
            title: Arc::from(s.title.as_str()),
            duration_str: Arc::from(format_duration(s.duration).as_str()),
            is_multi_selected: selection_in_middle && selection_indices.contains(&orig_index),
        });
    }

    AlbumDetailBodyModel {
        state: AlbumDetailState::Tracks {
            header,
            rows,
            use_hours,
        },
        selected_filtered: middle_selected,
        focused,
        in_selection: selection_in_middle,
    }
}

/// Word-wrap `text` to `width` columns. Mirrors
/// `mkp2/mkp/src/ui/format.rs::wrap_text` and the same helper in
/// `views::artist_detail` so cache-hit clones don't re-wrap.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w: usize = 0;
    for word in text.split_whitespace() {
        let word_w = word.chars().count();
        if current.is_empty() {
            current = word.to_string();
            current_w = word_w;
        } else if current_w + 1 + word_w <= width {
            current.push(' ');
            current.push_str(word);
            current_w += 1 + word_w;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
            current_w = word_w;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkproto::{Album, AlbumDetail, Song};

    fn s(title: &str, artist: &str, dur: f32) -> Song {
        Song {
            id: title.into(),
            title: title.into(),
            artist_name: artist.into(),
            album_title: "Al".into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn responses_with_album(seq: u64, album: Album, songs: Vec<Song>) -> Responses {
        let mut r = Responses::default();
        r.insert(seq, ServerMsg::AlbumDetail { album, songs });
        r
    }

    fn plain_album(name: &str) -> Album {
        Album {
            id: "a".into(),
            name: name.into(),
            artist_id: "x".into(),
            artist_name: "Artist".into(),
            track_count: 0,
            detail: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn run(
        responses: &Responses,
        seq: Option<u64>,
        filter: &str,
        sel: usize,
        focused: bool,
    ) -> AlbumDetailBodyModel {
        let filter_arc: Arc<str> = Arc::from(filter);
        album_detail_body_model(
            AlbumDetailResponseInput::new(seq, responses),
            &filter_arc,
            sel,
            focused,
            100,
            false,
            &OrdSet::new(),
        )
    }

    #[test]
    fn no_seq_yields_loading_state() {
        let r = Responses::default();
        let m = run(&r, None, "", 0, false);
        assert_eq!(m.state, AlbumDetailState::Loading);
    }

    #[test]
    fn missing_response_yields_loading() {
        let r = Responses::default();
        let m = run(&r, Some(99), "", 0, false);
        assert_eq!(m.state, AlbumDetailState::Loading);
    }

    #[test]
    fn three_songs_no_filter_yields_three_rows() {
        let r = responses_with_album(
            7,
            plain_album("Al"),
            vec![
                s("Alpha", "x", 60.0),
                s("Beta", "y", 30.0),
                s("Gamma", "z", 45.0),
            ],
        );
        let m = run(&r, Some(7), "", 0, true);
        if let AlbumDetailState::Tracks { rows, .. } = m.state {
            assert_eq!(rows.len(), 3);
            assert_eq!(&*rows[1].title, "Beta");
            assert_eq!(&*rows[2].duration_str, "00:45");
        } else {
            panic!("expected Tracks");
        }
    }

    #[test]
    fn filter_drops_non_matching() {
        let r = responses_with_album(
            7,
            plain_album("Al"),
            vec![
                s("Alpha", "x", 60.0),
                s("Beta", "y", 30.0),
                s("Gamma", "z", 45.0),
            ],
        );
        let m = run(&r, Some(7), "be", 0, false);
        if let AlbumDetailState::Tracks { rows, .. } = m.state {
            assert_eq!(rows.len(), 1);
            assert_eq!(&*rows[0].title, "Beta");
        } else {
            panic!("expected Tracks");
        }
    }

    #[test]
    fn filter_matches_artist() {
        let r = responses_with_album(
            7,
            plain_album("Al"),
            vec![
                s("Alpha", "John", 60.0),
                s("Beta", "Paul", 30.0),
                s("Gamma", "George", 45.0),
            ],
        );
        let m = run(&r, Some(7), "PAU", 0, false);
        if let AlbumDetailState::Tracks { rows, .. } = m.state {
            assert_eq!(rows.len(), 1);
            assert_eq!(&*rows[0].title, "Beta");
        }
    }

    #[test]
    fn header_extracts_year_and_notes() {
        let mut album = plain_album("Abbey Road");
        album.artist_name = "The Beatles".into();
        album.detail = Some(AlbumDetail {
            release_date: Some("2019-09-27".into()),
            record_label: Some("UMC".into()),
            editorial_notes_short: Some("Grand exit.".into()),
            editorial_notes_long: None,
            copyright: Some("\u{2117} 2019".into()),
        });
        let r = responses_with_album(7, album, vec![s("Come Together", "x", 260.0)]);
        let m = run(&r, Some(7), "", 0, false);
        if let AlbumDetailState::Tracks { header, .. } = m.state {
            assert_eq!(&*header.name, "Abbey Road");
            assert_eq!(&*header.artist, "The Beatles");
            assert_eq!(header.year.as_deref(), Some("2019"));
            assert_eq!(header.notes_lines.len(), 1);
            assert_eq!(&*header.notes_lines[0], "Grand exit.");
            assert_eq!(header.record_label.as_deref(), Some("UMC"));
            assert!(header.copyright.is_some());
        } else {
            panic!("expected Tracks");
        }
    }

    #[test]
    fn use_hours_set_when_any_song_exceeds_99_min() {
        let r = responses_with_album(
            7,
            plain_album("Long"),
            vec![s("Short", "x", 60.0), s("Epic", "y", 7200.0)],
        );
        let m = run(&r, Some(7), "", 0, false);
        if let AlbumDetailState::Tracks { use_hours, .. } = m.state {
            assert!(use_hours);
        } else {
            panic!("expected Tracks");
        }
    }

    #[test]
    fn empty_notes_string_collapses_to_none() {
        let mut album = plain_album("X");
        album.detail = Some(AlbumDetail {
            release_date: None,
            record_label: None,
            editorial_notes_short: Some(String::new()),
            editorial_notes_long: None,
            copyright: None,
        });
        let r = responses_with_album(7, album, vec![]);
        let m = run(&r, Some(7), "", 0, false);
        if let AlbumDetailState::Tracks { header, .. } = m.state {
            assert!(header.notes_lines.is_empty());
        } else {
            panic!("expected Tracks");
        }
    }
}
