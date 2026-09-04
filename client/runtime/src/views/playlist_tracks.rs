//! View model for the middle pane's `MiddleMode::PlaylistSongs`
//! body.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The body's primary
//! input is `imbl::Vector<Option<Arc<Song>>>` from
//! `state-playlist-tracks`; the Arc / `imbl` pointer-equality fast
//! path keeps cache-hit checks cheap even on long playlists.

use std::sync::Arc;

use imbl::{OrdSet, Vector};
use mkproto::Song;

use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_ui_playlists_pending::{PendingAdd, PendingPlaylists, PendingRemove};

use super::util::format_duration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum PlaylistTrackRow {
    /// Resolved song row.
    Song {
        orig_index: usize,
        title: String,
        artist: String,
        album: String,
        duration_str: String,
        is_multi_selected: bool,
    },
    /// `…` placeholder for a slot whose chunk hasn't arrived yet.
    Pending,
    /// Optimistic-add placeholder shown while a `SongAdded`
    /// broadcast is in flight. Painter renders a spinner glyph and
    /// "Adding…" label. Not cursor-targetable (sits below all real
    /// rows).
    PendingAdd,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum PlaylistTracksState {
    /// `playlist_id == None` — no playlist picked. Renderer paints
    /// blank (legacy parity).
    Empty,
    /// `playlist_id` set but `ListBegin` hasn't landed yet. Renderer
    /// paints `<spinner> Loading…`.
    Loading,
    Tracks {
        rows: Vector<PlaylistTrackRow>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistTracksBodyModel {
    pub state: PlaylistTracksState,
    /// Cursor row in the *filtered* row list. Renderer turns it into
    /// the visual highlight via `row_style_combined`.
    pub selected_filtered: usize,
    pub focused: bool,
    pub in_selection: bool,
}

#[derive(drv::Input)]
pub struct PlaylistTracksInput<'a> {
    pub playlist_id_present: bool,
    pub playlist_id: Option<&'a std::sync::Arc<str>>,
    pub ready: bool,
    pub songs: &'a Vector<Option<Arc<Song>>>,
}

impl<'a> PlaylistTracksInput<'a> {
    pub fn new(t: &'a PlaylistTracks) -> Self {
        Self {
            playlist_id_present: t.playlist_id.is_some(),
            playlist_id: t.playlist_id.as_ref(),
            ready: t.is_ready(),
            songs: &t.songs,
        }
    }
}

#[derive(drv::Input)]
pub struct PlaylistTracksPendingInput<'a> {
    pub adding: &'a Vector<PendingAdd>,
    pub removing: &'a Vector<PendingRemove>,
}

impl<'a> PlaylistTracksPendingInput<'a> {
    pub fn new(p: &'a PendingPlaylists) -> Self {
        Self {
            adding: &p.adding,
            removing: &p.removing,
        }
    }
}

#[drv::memo(single)]
pub fn playlist_tracks_body_model<'a, 'b>(
    tracks: PlaylistTracksInput<'a>,
    pending: PlaylistTracksPendingInput<'b>,
    middle_filter: &Arc<str>,
    selection_in_middle: bool,
    selection_indices: &OrdSet<usize>,
    middle_selected: usize,
    focused: bool,
) -> PlaylistTracksBodyModel {
    if !tracks.playlist_id_present {
        return PlaylistTracksBodyModel {
            state: PlaylistTracksState::Empty,
            selected_filtered: middle_selected,
            focused,
            in_selection: selection_in_middle,
        };
    }

    if !tracks.ready && tracks.songs.is_empty() {
        return PlaylistTracksBodyModel {
            state: PlaylistTracksState::Loading,
            selected_filtered: middle_selected,
            focused,
            in_selection: selection_in_middle,
        };
    }

    let filter_lower = middle_filter.to_lowercase();
    let all_match = filter_lower.is_empty();
    let loaded_id: &str = tracks.playlist_id.map(|s| &**s).unwrap_or("");
    // Optimistic remove: a song id appearing in any pending-remove
    // for this playlist is treated as already gone from the list.
    let is_removing = |song_id: &str| {
        pending
            .removing
            .iter()
            .any(|r| r.playlist_id == loaded_id && r.song_ids.iter().any(|s| s == song_id))
    };
    let mut rows: Vector<PlaylistTrackRow> = Vector::new();
    for (orig_index, slot) in tracks.songs.iter().enumerate() {
        match slot {
            Some(s) if is_removing(&s.id) => continue,
            Some(s) if !all_match => {
                let matches = s.title.to_lowercase().contains(&filter_lower)
                    || s.artist_name.to_lowercase().contains(&filter_lower)
                    || s.album_title.to_lowercase().contains(&filter_lower);
                if !matches {
                    continue;
                }
                rows.push_back(PlaylistTrackRow::Song {
                    orig_index,
                    title: s.title.clone(),
                    artist: s.artist_name.clone(),
                    album: s.album_title.clone(),
                    duration_str: format_duration(s.duration),
                    is_multi_selected: selection_in_middle
                        && selection_indices.contains(&orig_index),
                });
            }
            Some(s) => {
                rows.push_back(PlaylistTrackRow::Song {
                    orig_index,
                    title: s.title.clone(),
                    artist: s.artist_name.clone(),
                    album: s.album_title.clone(),
                    duration_str: format_duration(s.duration),
                    is_multi_selected: selection_in_middle
                        && selection_indices.contains(&orig_index),
                });
            }
            None if all_match => {
                rows.push_back(PlaylistTrackRow::Pending);
            }
            None => {}
        }
    }

    // Optimistic add: append a "+ Adding…" placeholder per in-flight
    // request against this playlist. Painter renders with spinner;
    // dispatch ignores the rows (they're below the cursor space —
    // see middle_row_count in queries.rs).
    let pending_add_count = pending
        .adding
        .iter()
        .filter(|a| a.playlist_id == loaded_id)
        .count();
    for _ in 0..pending_add_count {
        rows.push_back(PlaylistTrackRow::PendingAdd);
    }

    PlaylistTracksBodyModel {
        state: PlaylistTracksState::Tracks { rows },
        selected_filtered: middle_selected,
        focused,
        in_selection: selection_in_middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkproto::Song;

    fn s(id: &str, title: &str, artist: &str, album: &str, dur: f32) -> Song {
        Song {
            id: id.into(),
            title: title.into(),
            artist_name: artist.into(),
            album_title: album.into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn tracks_with(slots: Vec<Option<Song>>) -> PlaylistTracks {
        // pending_task None + playlist_id Some makes is_ready() true.
        PlaylistTracks {
            playlist_id: Some("p1".into()),
            total: slots.len(),
            songs: slots
                .into_iter()
                .map(|o| o.map(std::sync::Arc::new))
                .collect(),
            ..Default::default()
        }
    }

    fn no_pending() -> PendingPlaylists {
        PendingPlaylists::default()
    }

    #[test]
    fn no_playlist_yields_empty_state() {
        let t = PlaylistTracks::default();
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        assert_eq!(m.state, PlaylistTracksState::Empty);
    }

    #[test]
    fn pending_load_yields_loading_state() {
        // pending_task Some + empty songs = "load in flight,
        // ListBegin not yet folded" = the loading-spinner state.
        let t = PlaylistTracks {
            playlist_id: Some("p1".into()),
            pending_task: Some(1),
            ..Default::default()
        };
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        assert_eq!(m.state, PlaylistTracksState::Loading);
    }

    #[test]
    fn empty_filter_includes_pending_slots_as_placeholders() {
        let t = tracks_with(vec![
            Some(s("1", "A", "x", "y", 60.0)),
            None,
            Some(s("3", "C", "x", "y", 30.0)),
        ]);
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            assert_eq!(rows.len(), 3);
            assert!(matches!(rows[1], PlaylistTrackRow::Pending));
        } else {
            panic!("expected Tracks state");
        }
    }

    #[test]
    fn filter_drops_pending_and_unmatched_songs() {
        let t = tracks_with(vec![
            Some(s("1", "Alpha", "x", "y", 60.0)),
            None,
            Some(s("3", "Beta", "x", "y", 30.0)),
            Some(s("4", "Gamma", "x", "y", 40.0)),
        ]);
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from("be"),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            assert_eq!(rows.len(), 1);
            if let PlaylistTrackRow::Song { title, .. } = &rows[0] {
                assert_eq!(title, "Beta");
            } else {
                panic!("expected Song row");
            }
        } else {
            panic!("expected Tracks state");
        }
    }

    #[test]
    fn filter_matches_artist_and_album_too() {
        let t = tracks_with(vec![
            Some(s("1", "A", "rolling stones", "y", 60.0)),
            Some(s("2", "B", "x", "abbey road", 30.0)),
            Some(s("3", "C", "x", "y", 40.0)),
        ]);
        let p = no_pending();
        let by_artist = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from("rolling"),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = by_artist.state {
            assert_eq!(rows.len(), 1);
        } else {
            panic!("expected Tracks state");
        }
        let by_album = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from("abbey"),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = by_album.state {
            assert_eq!(rows.len(), 1);
        } else {
            panic!("expected Tracks state");
        }
    }

    #[test]
    fn multi_selection_marks_rows_when_context_is_middle() {
        let t = tracks_with(vec![
            Some(s("1", "A", "x", "y", 60.0)),
            Some(s("2", "B", "x", "y", 30.0)),
            Some(s("3", "C", "x", "y", 40.0)),
        ]);
        let mut sel = OrdSet::new();
        sel.insert(0);
        sel.insert(2);
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            true,
            &sel,
            1,
            true,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            if let PlaylistTrackRow::Song {
                is_multi_selected, ..
            } = &rows[0]
            {
                assert!(is_multi_selected);
            } else {
                panic!("song row");
            }
            if let PlaylistTrackRow::Song {
                is_multi_selected, ..
            } = &rows[1]
            {
                assert!(!is_multi_selected);
            }
            if let PlaylistTrackRow::Song {
                is_multi_selected, ..
            } = &rows[2]
            {
                assert!(is_multi_selected);
            }
        } else {
            panic!("expected Tracks");
        }
    }

    #[test]
    fn multi_selection_not_marked_when_context_not_middle() {
        let t = tracks_with(vec![Some(s("1", "A", "x", "y", 60.0))]);
        let mut sel = OrdSet::new();
        sel.insert(0);
        let p = no_pending();
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &sel,
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            if let PlaylistTrackRow::Song {
                is_multi_selected, ..
            } = &rows[0]
            {
                assert!(!is_multi_selected);
            }
        }
    }

    #[test]
    fn removing_song_id_filters_row() {
        let t = tracks_with(vec![
            Some(s("1", "Alpha", "x", "y", 60.0)),
            Some(s("2", "Beta", "x", "y", 30.0)),
        ]);
        let mut p = PendingPlaylists::default();
        p.add_removing(7, "p1".into(), vec!["1".into()]);
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            assert_eq!(rows.len(), 1);
            if let PlaylistTrackRow::Song { title, .. } = &rows[0] {
                assert_eq!(title, "Beta");
            }
        } else {
            panic!("expected Tracks state");
        }
    }

    #[test]
    fn adding_appends_pendingadd_placeholder() {
        let t = tracks_with(vec![Some(s("1", "Alpha", "x", "y", 60.0))]);
        let mut p = PendingPlaylists::default();
        p.add_adding(9, "p1".into());
        p.add_adding(10, "p1".into());
        let m = playlist_tracks_body_model(
            PlaylistTracksInput::new(&t),
            PlaylistTracksPendingInput::new(&p),
            &Arc::from(""),
            false,
            &OrdSet::new(),
            0,
            false,
        );
        if let PlaylistTracksState::Tracks { rows } = m.state {
            assert_eq!(rows.len(), 3);
            assert!(matches!(rows[1], PlaylistTrackRow::PendingAdd));
            assert!(matches!(rows[2], PlaylistTrackRow::PendingAdd));
        } else {
            panic!("expected Tracks state");
        }
    }
}
