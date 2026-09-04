//! View model for the middle pane's `MiddleMode::ArtistDetail`
//! body.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The artist-detail
//! response lives in `state-responses` as `Arc<ServerMsg>` and the
//! similar / paged albums in `state-artist-detail` as
//! `imbl::HashMap<_, imbl::Vector<Arc<_>>>`. Both ride into the
//! memo via narrow `drv::Input` projections so cache-hits are
//! O(1).
//!
//! Legacy parity (mkp2 `nav/player/middle.rs::draw_artist_detail`):
//!
//! - Artist name + editorial notes live in an info `Paragraph`
//!   above the scrollable list — not as list rows. The memo
//!   exposes them on `ArtistDetailLoaded::info`.
//! - Section ordering: **Top Songs**, **Top Albums**,
//!   **Discography**, **Related Artists**. Streamed
//!   `ArtistAlbumsChunk` extras concatenate onto **Discography**;
//!   there is no "More Albums" section.
//! - Related Artists render as `" • "`-separated flow rows, not
//!   one artist per visual row. The memo materialises the flow
//!   layout (one `SimilarFlow` row per visual line) so cursor-stop
//!   ordinals stay one-per-artist while the painter still renders
//!   the line correctly.
//! - `SimilarLoading` is emitted only when the
//!   `SimilarArtists` broadcast has not yet arrived for this
//!   artist id (i.e. the extras map has no key). Loaded-but-empty
//!   shows the section header alone.

use std::sync::Arc;

use imbl::{HashMap as ImHashMap, Vector};

use mkpclient_state_artist_detail::ArtistDetailExtras;
use mkpclient_state_responses::Responses;
use mkproto::{Album, Artist, ServerMsg};

use super::util::format_duration;

/// Top-of-pane info block — rendered as a `Paragraph` above the
/// scrollable list. `Arc`-wrapped so cache-hit clones are refcount
/// bumps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtistInfo {
    pub name: Arc<str>,
    /// Pre-wrapped editorial notes lines, body-width-dependent.
    pub notes_lines: Vector<Arc<str>>,
}

/// One similar-artist entry inside a `SimilarFlow` row. Carries
/// its cursor-stop ordinal so the painter can highlight the
/// selected entry without re-deriving the cursor space.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SimilarArtistEntry {
    pub cursor_stop: u32,
    pub name: Arc<str>,
}

/// One painted row inside the artist-detail body's scrollable
/// list area.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ArtistDetailRow {
    /// Empty separator.
    Blank,
    /// Section header (cyan-bold): "Top Songs", "Top Albums",
    /// "Discography", "Related Artists".
    SectionHeader(Arc<str>),
    /// Cursor-stop. Top-songs row.
    SongItem {
        cursor_stop: u32,
        title: Arc<str>,
        album: Arc<str>,
        duration_str: Arc<str>,
    },
    /// Cursor-stop. Album row.
    AlbumItem {
        cursor_stop: u32,
        year: Arc<str>,
        name: Arc<str>,
        track_count: u32,
    },
    /// One visual line of similar artists, bullet-flowed. Each
    /// entry is its own cursor stop.
    SimilarFlow { artists: Vector<SimilarArtistEntry> },
    /// "Related Artists" body before the similar-artist broadcast
    /// arrives — shows `<spinner> Loading…`. Painter interpolates
    /// the spinner glyph at draw time.
    SimilarLoading,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtistDetailLoaded {
    pub info: Arc<ArtistInfo>,
    pub rows: Vector<ArtistDetailRow>,
    /// Visual-row index of every cursor-stop item, in order.
    /// `cursor.middle` indexes into this — the painter dereferences
    /// it to scroll the list to the right line. For similar
    /// artists in a flow row, multiple cursor-stops map to the
    /// same visual row.
    pub item_visual_indices: Vector<usize>,
    pub song_title_w: usize,
    pub song_album_w: usize,
    pub song_time_w: usize,
    pub album_year_w: usize,
    pub album_name_w: usize,
    pub album_tracks_w: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ArtistDetailState {
    Loading,
    Loaded(ArtistDetailLoaded),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtistDetailBodyModel {
    pub state: ArtistDetailState,
    pub selected_item: usize,
    pub focused: bool,
}

#[derive(drv::Input)]
pub struct ArtistDetailResponseInput {
    pub response: Option<Arc<ServerMsg>>,
}

impl ArtistDetailResponseInput {
    pub fn new(awaiting_seq: Option<u64>, responses: &Responses) -> Self {
        Self {
            response: awaiting_seq
                .and_then(|seq| responses.by_seq.get(&seq))
                .cloned(),
        }
    }
}

#[derive(drv::Input)]
pub struct ArtistDetailExtrasInput<'a> {
    pub similar: &'a ImHashMap<String, Vector<Arc<Artist>>>,
    pub paged_albums: &'a ImHashMap<String, Vector<Arc<Album>>>,
}

impl<'a> ArtistDetailExtrasInput<'a> {
    pub fn new(e: &'a ArtistDetailExtras) -> Self {
        Self {
            similar: &e.similar,
            paged_albums: &e.paged_albums,
        }
    }
}

#[drv::memo(single)]
pub fn artist_detail_body_model<'a>(
    response: ArtistDetailResponseInput,
    extras: ArtistDetailExtrasInput<'a>,
    middle_selected: usize,
    focused: bool,
    body_width: u16,
    time_w: usize,
) -> ArtistDetailBodyModel {
    let resp = match response.response.as_deref() {
        Some(r) => r,
        None => {
            return ArtistDetailBodyModel {
                state: ArtistDetailState::Loading,
                selected_item: middle_selected,
                focused,
            };
        }
    };
    let (artist, top_songs) = match resp {
        ServerMsg::ArtistDetail { artist, top_songs } => (artist, top_songs),
        _ => {
            return ArtistDetailBodyModel {
                state: ArtistDetailState::Loading,
                selected_item: middle_selected,
                focused,
            };
        }
    };
    let detail = artist.detail.clone().unwrap_or(mkproto::ArtistDetail {
        editorial_notes_short: None,
        top_albums: vec![],
        latest_albums: vec![],
    });
    let top_albums = detail.top_albums;
    let latest_albums = detail.latest_albums;

    let empty_albums: Vector<Arc<Album>> = Vector::new();
    let paged = extras.paged_albums.get(&artist.id).unwrap_or(&empty_albums);

    // Legacy folds streamed `ArtistAlbumsChunk` deliveries into the
    // Discography section (mkp2 `app/history.rs::update_artist_albums`
    // extends `latest_albums`). Mirror that here so the cursor-stop
    // space and visual layout match.
    let discography_len = latest_albums.len() + paged.len();

    let info = build_info(artist, body_width as usize);

    let w = body_width as usize;
    // Song columns: Title (55%) | Album (45%) | Time. No Artist
    // column on the artist-detail page (legacy parity).
    let song_time_w = time_w;
    let song_remaining = w.saturating_sub(song_time_w + 1);
    let song_title_w = song_remaining * 55 / 100;
    let song_album_w = song_remaining.saturating_sub(song_title_w);
    // Album columns: Year (6) | Name (rest) | Tracks (6).
    let album_year_w = 6usize;
    let album_tracks_w = 6usize;
    let album_name_w = w.saturating_sub(album_year_w + album_tracks_w + 1);

    let mut rows: Vector<ArtistDetailRow> = Vector::new();
    let mut item_visual_indices: Vector<usize> = Vector::new();
    // Running cursor-stop ordinal — incremented every time a
    // selectable item lands in `rows`. Cursor-stops are densely
    // numbered (no gaps for headers) so `cursor.middle` indexes
    // them directly via `item_visual_indices`.
    let mut cursor_stop: u32 = 0;

    if !top_songs.is_empty() {
        rows.push_back(ArtistDetailRow::SectionHeader(Arc::from("Top Songs")));
        for s in top_songs.iter() {
            item_visual_indices.push_back(rows.len());
            rows.push_back(ArtistDetailRow::SongItem {
                cursor_stop,
                title: Arc::from(s.title.as_str()),
                album: Arc::from(s.album_title.as_str()),
                duration_str: Arc::from(format_duration(s.duration).as_str()),
            });
            cursor_stop += 1;
        }
    }

    push_album_section(
        "Top Albums",
        top_albums.iter(),
        &mut rows,
        &mut item_visual_indices,
        &mut cursor_stop,
    );
    if discography_len > 0 {
        let merged = latest_albums.iter().chain(paged.iter().map(|a| &**a));
        push_album_section(
            "Discography",
            merged,
            &mut rows,
            &mut item_visual_indices,
            &mut cursor_stop,
        );
    }

    push_similar_section(
        artist,
        &extras,
        body_width as usize,
        &mut rows,
        &mut item_visual_indices,
        &mut cursor_stop,
    );

    ArtistDetailBodyModel {
        state: ArtistDetailState::Loaded(ArtistDetailLoaded {
            info: Arc::new(info),
            rows,
            item_visual_indices,
            song_title_w,
            song_album_w,
            song_time_w,
            album_year_w,
            album_name_w,
            album_tracks_w,
        }),
        selected_item: middle_selected,
        focused,
    }
}

fn build_info(artist: &Artist, body_width: usize) -> ArtistInfo {
    let notes_lines: Vector<Arc<str>> = artist
        .detail
        .as_ref()
        .and_then(|d| d.editorial_notes_short.as_ref())
        .filter(|s| !s.is_empty())
        .map(|s| {
            wrap_text(s, body_width)
                .into_iter()
                .map(Arc::from)
                .collect()
        })
        .unwrap_or_default();
    ArtistInfo {
        name: Arc::from(artist.name.as_str()),
        notes_lines,
    }
}

fn push_album_section<'a, I>(
    label: &str,
    albums: I,
    rows: &mut Vector<ArtistDetailRow>,
    ivi: &mut Vector<usize>,
    cursor_stop: &mut u32,
) where
    I: IntoIterator<Item = &'a Album>,
{
    let mut iter = albums.into_iter().peekable();
    if iter.peek().is_none() {
        return;
    }
    rows.push_back(ArtistDetailRow::Blank);
    rows.push_back(ArtistDetailRow::SectionHeader(Arc::from(label)));
    for a in iter {
        ivi.push_back(rows.len());
        let year = a
            .detail
            .as_ref()
            .and_then(|d| d.release_date.as_ref())
            .map(|d| d.split('-').next().unwrap_or(d.as_str()))
            .unwrap_or_default();
        rows.push_back(ArtistDetailRow::AlbumItem {
            cursor_stop: *cursor_stop,
            year: Arc::from(year),
            name: Arc::from(a.name.as_str()),
            track_count: a.track_count as u32,
        });
        *cursor_stop += 1;
    }
}

fn push_similar_section(
    artist: &Artist,
    extras: &ArtistDetailExtrasInput<'_>,
    body_width: usize,
    rows: &mut Vector<ArtistDetailRow>,
    ivi: &mut Vector<usize>,
    cursor_stop: &mut u32,
) {
    rows.push_back(ArtistDetailRow::Blank);
    rows.push_back(ArtistDetailRow::SectionHeader(Arc::from("Related Artists")));

    // Loaded-vs-not detection: legacy carries an explicit
    // `similar_artists_loaded` bool on the history entry; we infer
    // it from the extras map — `set_similar` is only called when
    // the broadcast lands, so a missing key means "still in
    // flight". Loaded-but-empty shows the section header alone.
    let Some(similar) = extras.similar.get(&artist.id) else {
        rows.push_back(ArtistDetailRow::SimilarLoading);
        return;
    };
    if similar.is_empty() {
        return;
    }

    // Pack names into flow rows that fit within `body_width`,
    // never breaking a name. Mirrors mkp2
    // `app/types.rs::compute_artist_flow_rows`.
    const FLOW_SEP_LEN: usize = 3; // " • "
    let mut current: Vector<SimilarArtistEntry> = Vector::new();
    let mut current_w: usize = 0;
    let flush = |current: &mut Vector<SimilarArtistEntry>, rows: &mut Vector<ArtistDetailRow>| {
        if !current.is_empty() {
            rows.push_back(ArtistDetailRow::SimilarFlow {
                artists: std::mem::take(current),
            });
        }
    };

    for a in similar.iter() {
        let name_len = a.name.chars().count();
        let needed = if current.is_empty() {
            name_len
        } else {
            FLOW_SEP_LEN + name_len
        };
        if !current.is_empty() && current_w + needed > body_width {
            flush(&mut current, rows);
            current_w = 0;
        }
        let needed = if current.is_empty() {
            name_len
        } else {
            FLOW_SEP_LEN + name_len
        };
        // The visual row this entry will land on is the row index
        // we'd push to right now; defer the ivi push until we
        // know the row index when we flush.
        let entry_row_index = rows.len();
        ivi.push_back(entry_row_index);
        current.push_back(SimilarArtistEntry {
            cursor_stop: *cursor_stop,
            name: Arc::from(a.name.as_str()),
        });
        *cursor_stop += 1;
        current_w += needed;
    }
    flush(&mut current, rows);
}

/// Word-wrap `text` to `width` columns. Mirrors
/// `mkp2/mkp/src/ui/format.rs::wrap_text` and the pre-rewrite
/// `render::wrap_text` (kept here so the model is self-contained
/// and the consumer doesn't need to fork a copy).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
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
            lines.push(current);
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
    use mkproto::{Album, AlbumDetail, Artist, ArtistDetail, Song};

    fn artist(id: &str, name: &str, notes: Option<&str>) -> Artist {
        Artist {
            id: id.into(),
            name: name.into(),
            detail: Some(ArtistDetail {
                editorial_notes_short: notes.map(Into::into),
                top_albums: vec![],
                latest_albums: vec![],
            }),
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn song(t: &str, alb: &str, dur: f32) -> Song {
        Song {
            id: t.into(),
            title: t.into(),
            artist_name: "x".into(),
            album_title: alb.into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn album(name: &str, year: Option<&str>, tracks: usize) -> Album {
        Album {
            id: name.into(),
            name: name.into(),
            artist_id: "x".into(),
            artist_name: "x".into(),
            track_count: tracks,
            detail: year.map(|y| AlbumDetail {
                release_date: Some(y.into()),
                record_label: None,
                editorial_notes_short: None,
                editorial_notes_long: None,
                copyright: None,
            }),
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn responses_with(seq: u64, artist: Artist, top_songs: Vec<Song>) -> Responses {
        let mut r = Responses::default();
        r.insert(seq, ServerMsg::ArtistDetail { artist, top_songs });
        r
    }

    fn run(
        responses: &Responses,
        extras: &ArtistDetailExtras,
        seq: Option<u64>,
        sel: usize,
        focused: bool,
        body_width: u16,
        time_w: usize,
    ) -> ArtistDetailBodyModel {
        artist_detail_body_model(
            ArtistDetailResponseInput::new(seq, responses),
            ArtistDetailExtrasInput::new(extras),
            sel,
            focused,
            body_width,
            time_w,
        )
    }

    #[test]
    fn no_seq_yields_loading_state() {
        let r = Responses::default();
        let e = ArtistDetailExtras::default();
        let m = run(&r, &e, None, 0, false, 100, 6);
        assert_eq!(m.state, ArtistDetailState::Loading);
    }

    #[test]
    fn no_response_yields_loading_state() {
        let r = Responses::default();
        let e = ArtistDetailExtras::default();
        let m = run(&r, &e, Some(99), 0, false, 100, 6);
        assert_eq!(m.state, ArtistDetailState::Loading);
    }

    #[test]
    fn loaded_artist_with_only_top_songs_has_no_title_or_note_rows() {
        let mut a = artist("a1", "Adele", None);
        a.detail = Some(ArtistDetail {
            editorial_notes_short: None,
            top_albums: vec![],
            latest_albums: vec![],
        });
        let r = responses_with(
            7,
            a,
            vec![song("Hello", "25", 60.0), song("Skyfall", "21", 70.0)],
        );
        let e = ArtistDetailExtras::default();
        let m = run(&r, &e, Some(7), 0, false, 100, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!("expected Loaded"),
        };
        // No Title / NoteLine variants exist anymore — info lives
        // on `loaded.info`. List rows: "Top Songs" + 2 songs +
        // Blank + "Related Artists" + Loading.
        assert_eq!(loaded.rows.len(), 6);
        assert!(matches!(loaded.rows[0], ArtistDetailRow::SectionHeader(_)));
        assert!(matches!(loaded.rows[1], ArtistDetailRow::SongItem { .. }));
        assert!(matches!(loaded.rows[2], ArtistDetailRow::SongItem { .. }));
        // 2 song items mapped to visual rows 1 and 2 (under the header).
        assert_eq!(loaded.item_visual_indices.len(), 2);
        assert_eq!(loaded.item_visual_indices[0], 1);
        assert_eq!(loaded.item_visual_indices[1], 2);
        // Info populated.
        assert_eq!(&*loaded.info.name, "Adele");
        assert!(loaded.info.notes_lines.is_empty());
        // No similar broadcast yet → trailing Loading row.
        assert!(matches!(
            loaded.rows[loaded.rows.len() - 1],
            ArtistDetailRow::SimilarLoading
        ));
    }

    #[test]
    fn editorial_notes_wrap_to_body_width_in_info() {
        let mut a = artist(
            "a1",
            "Adele",
            Some("The London singer and songwriter has dominated charts since 2008."),
        );
        a.detail = Some(ArtistDetail {
            editorial_notes_short: Some(
                "The London singer and songwriter has dominated charts since 2008.".into(),
            ),
            top_albums: vec![],
            latest_albums: vec![],
        });
        let r = responses_with(7, a, vec![]);
        let e = ArtistDetailExtras::default();
        let m = run(&r, &e, Some(7), 0, false, 30, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!(),
        };
        assert!(loaded.info.notes_lines.len() >= 2);
        // No NoteLine variant in rows — info carries them.
    }

    #[test]
    fn paged_albums_concatenate_into_discography_section() {
        let mut a = artist("a1", "Adele", None);
        a.detail = Some(ArtistDetail {
            editorial_notes_short: None,
            top_albums: vec![],
            latest_albums: vec![album("19", Some("2008"), 12)],
        });
        let r = responses_with(7, a, vec![]);
        let mut e = ArtistDetailExtras::default();
        e.append_albums("a1".into(), vec![album("21", Some("2011"), 11)]);
        let m = run(&r, &e, Some(7), 0, false, 100, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!(),
        };
        let section_labels: Vec<String> = loaded
            .rows
            .iter()
            .filter_map(|r| match r {
                ArtistDetailRow::SectionHeader(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        // No "More Albums" — only "Discography" + "Related Artists".
        assert!(section_labels.iter().any(|s| s == "Discography"));
        assert!(!section_labels.iter().any(|s| s == "More Albums"));
        let album_count = loaded
            .rows
            .iter()
            .filter(|r| matches!(r, ArtistDetailRow::AlbumItem { .. }))
            .count();
        assert_eq!(album_count, 2);
    }

    #[test]
    fn similar_loaded_empty_skips_loading_row() {
        let a = artist("a1", "Adele", None);
        let r = responses_with(7, a, vec![]);
        let mut e = ArtistDetailExtras::default();
        e.set_similar("a1".into(), vec![]);
        let m = run(&r, &e, Some(7), 0, false, 100, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!(),
        };
        assert!(!loaded
            .rows
            .iter()
            .any(|r| matches!(r, ArtistDetailRow::SimilarLoading)));
        assert!(!loaded
            .rows
            .iter()
            .any(|r| matches!(r, ArtistDetailRow::SimilarFlow { .. })));
    }

    #[test]
    fn similar_artists_pack_into_flow_rows() {
        let a = artist("a1", "X", None);
        let r = responses_with(7, a, vec![]);
        let mut e = ArtistDetailExtras::default();
        let names: Vec<&str> = vec!["Don Henley", "Steve Winwood", "Toto", "Asia"];
        let artists: Vec<Artist> = names
            .iter()
            .map(|n| Artist {
                id: (*n).into(),
                name: (*n).into(),
                detail: None,
                url: None,
                artwork_url_small: None,
                artwork_url_large: None,
            })
            .collect();
        e.set_similar("a1".into(), artists);
        // Tight width forces multi-row flow.
        let m = run(&r, &e, Some(7), 0, false, 25, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!(),
        };
        let flow_count = loaded
            .rows
            .iter()
            .filter(|r| matches!(r, ArtistDetailRow::SimilarFlow { .. }))
            .count();
        assert!(flow_count >= 2);
        // Sum of entries equals 4.
        let total_entries: usize = loaded
            .rows
            .iter()
            .filter_map(|r| match r {
                ArtistDetailRow::SimilarFlow { artists } => Some(artists.len()),
                _ => None,
            })
            .sum();
        assert_eq!(total_entries, 4);
    }

    #[test]
    fn cursor_stops_are_dense_and_match_item_visual_indices() {
        let mut a = artist("a1", "X", None);
        a.detail = Some(ArtistDetail {
            editorial_notes_short: None,
            top_albums: vec![album("Top1", Some("2010"), 9)],
            latest_albums: vec![album("Disco1", Some("2020"), 8)],
        });
        let r = responses_with(7, a, vec![song("S1", "Al", 30.0)]);
        let mut e = ArtistDetailExtras::default();
        e.set_similar(
            "a1".into(),
            vec![Artist {
                id: "B".into(),
                name: "B".into(),
                detail: None,
                url: None,
                artwork_url_small: None,
                artwork_url_large: None,
            }],
        );
        let m = run(&r, &e, Some(7), 0, false, 100, 6);
        let loaded = match m.state {
            ArtistDetailState::Loaded(l) => l,
            _ => panic!(),
        };
        // 1 song + 1 top album + 1 discography + 1 similar = 4
        // cursor stops.
        assert_eq!(loaded.item_visual_indices.len(), 4);
        // Check cursor_stop fields are 0..4.
        let mut cursor_stops: Vec<u32> = Vec::new();
        for row in loaded.rows.iter() {
            match row {
                ArtistDetailRow::SongItem { cursor_stop, .. }
                | ArtistDetailRow::AlbumItem { cursor_stop, .. } => cursor_stops.push(*cursor_stop),
                ArtistDetailRow::SimilarFlow { artists } => {
                    cursor_stops.extend(artists.iter().map(|e| e.cursor_stop))
                }
                _ => {}
            }
        }
        assert_eq!(cursor_stops, vec![0, 1, 2, 3]);
    }
}
