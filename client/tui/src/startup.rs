//! TRACE-only, payload-free observations taken after a successful terminal draw.
//! This reports facts rather than guessing that a restored navigation state is loaded.
use crate::app::AppState;
use mkpclient_runtime::Sources;
use mkpclient_state_ui_history::MiddleMode;
use mkproto::ServerMsg;
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
pub struct Observation {
    pub frame: u32,
    pub playback_queue: bool,
    pub sidebar: bool,
    pub visible_view: bool,
    pub complete_view: bool,
    pub restored: bool,
    pub view_kind: &'static str,
    pub playlists: usize,
    pub queue_rows: usize,
    pub queue_expected: Option<usize>,
    pub view_rows: usize,
    pub view_total: usize,
    pub viewport_offset: usize,
    pub viewport_rows: usize,
    pub playing: bool,
    pub has_song: bool,
    pub failed: bool,
}

pub fn observe(s: &Sources, app: &AppState) -> Observation {
    let (kind, first, complete, rows, total) = match &s.history.mode {
        MiddleMode::PlaylistSongs => {
            let p = &s.playlist_tracks;
            let received = p.playlist_id.is_some() && p.pending_task.is_none();
            let empty_library =
                s.playlists.loaded && s.playlists.items.is_empty() && p.playlist_id.is_none();
            let rows = p.songs.iter().filter(|v| v.is_some()).count();
            let offset = app.middle_offset.get().min(p.total);
            let end = offset.saturating_add(app.middle_height.get()).min(p.total);
            let visible = received
                && (p.total == 0
                    || (offset < end
                        && (offset..end).all(|i| p.songs.get(i).is_some_and(Option::is_some))));
            (
                "playlist",
                empty_library || visible,
                empty_library || (received && rows == p.total),
                rows,
                p.total,
            )
        }
        MiddleMode::SearchResults { .. } => {
            let rows = s.search.songs.len() + s.search.albums.len() + s.search.artists.len();
            (
                "search",
                s.search.first_page_received,
                s.search.first_page_received && s.search.completed,
                rows,
                rows,
            )
        }
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            let songs = awaiting_seq
                .and_then(|seq| s.responses.by_seq.get(&seq))
                .and_then(|r| match &**r {
                    ServerMsg::AlbumDetail { songs, .. } => Some(songs.len()),
                    _ => None,
                });
            (
                "album",
                songs.is_some(),
                songs.is_some(),
                songs.unwrap_or(0),
                songs.unwrap_or(0),
            )
        }
        MiddleMode::ArtistDetail {
            awaiting_seq,
            artist_id,
            ..
        } => {
            let songs = awaiting_seq
                .and_then(|seq| s.responses.by_seq.get(&seq))
                .and_then(|r| match &**r {
                    ServerMsg::ArtistDetail { top_songs, .. } => Some(top_songs.len()),
                    _ => None,
                });
            let extras = s
                .artist_extras
                .paged_albums_for(artist_id)
                .map_or(0, |v| v.len())
                + s.artist_extras
                    .similar_for(artist_id)
                    .map_or(0, |v| v.len());
            (
                "artist",
                songs.is_some(),
                songs.is_some() && s.artist_extras.similar_for(artist_id).is_some(),
                songs.unwrap_or(0) + extras,
                songs.unwrap_or(0) + extras,
            )
        }
    };
    let connected = matches!(s.link.phase, mkpclient_state_link::LinkPhase::Connected);
    let failed = matches!(
        s.screen,
        mkpclient_state_ui_screen::Screen::ErrorModal { .. }
    );
    let restored = s.session.auto_restored_view;
    let rendered = connected && !failed && app.middle_height.get() > 0;
    Observation {
        frame: app.tick,
        playback_queue: rendered
            && s.server.play.is_some()
            && s.queue.queue_id.is_some()
            && s.queue
                .expected_total
                .is_some_and(|n| s.queue.items.len() >= n),
        sidebar: rendered && s.playlists.loaded,
        visible_view: rendered && restored && first,
        complete_view: rendered && restored && complete,
        restored,
        view_kind: kind,
        playlists: s.playlists.items.len(),
        queue_rows: s.queue.items.len(),
        queue_expected: s.queue.expected_total,
        view_rows: rows,
        view_total: total,
        viewport_offset: app.middle_offset.get(),
        viewport_rows: app.middle_height.get(),
        playing: s
            .server
            .play
            .as_ref()
            .is_some_and(|p| p.playback == mkproto::PlaybackState::Playing),
        has_song: s
            .server
            .play
            .as_ref()
            .is_some_and(|p| p.now_playing.is_some()),
        failed,
    }
}

pub fn trace_after_draw(s: &Sources, app: &AppState) {
    if log::log_enabled!(target: "mkp_startup", log::Level::Trace) {
        let observation = observe(s, app);
        if let Ok(json) = serde_json::to_string(&observation) {
            log::trace!(target: "mkp_startup", "event=startup_observation json={json}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> (Sources, AppState) {
        let mut s = Sources::default();
        s.link.phase = mkpclient_state_link::LinkPhase::Connected;
        s.session.auto_restored_view = true;
        s.playlists.loaded = true;
        let app = AppState::default();
        app.middle_height.set(2);
        (s, app)
    }
    #[test]
    fn empty_library_is_ready_but_unknown_playback_is_not() {
        let (mut s, app) = ready();
        assert!(observe(&s, &app).complete_view);
        assert!(!observe(&s, &app).playback_queue);
        s.server.play = Some(Default::default());
        s.queue.reset(1);
        s.queue.expected_total = Some(0);
        assert!(observe(&s, &app).playback_queue);
    }
    #[test]
    fn navigation_restore_and_list_begin_are_not_loaded_rows() {
        let (mut s, app) = ready();
        s.playlist_tracks.begin("playlist".into(), 3, 0);
        assert!(!observe(&s, &app).visible_view);
        assert!(!observe(&s, &app).complete_view);
        s.playlist_tracks.begin("playlist".into(), 0, 0);
        assert!(observe(&s, &app).complete_view);
        s.playlist_tracks.pending_task = Some(4);
        assert!(!observe(&s, &app).complete_view);
    }
    #[test]
    fn loaded_viewport_is_distinct_from_complete_playlist() {
        let (mut s, app) = ready();
        s.playlist_tracks.begin("playlist".into(), 3, 0);
        let song = |id: &str| mkproto::Song {
            unavailable: false,
            id: id.into(),
            title: id.into(),
            artist_name: String::new(),
            album_title: String::new(),
            duration: 0.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        };
        s.playlist_tracks.chunk(0, vec![song("one"), song("two")]);
        let observation = observe(&s, &app);
        assert!(observation.visible_view);
        assert!(!observation.complete_view);
        app.middle_offset.set(2);
        assert!(!observe(&s, &app).visible_view);
        s.playlist_tracks.chunk(2, vec![song("three")]);
        assert!(observe(&s, &app).complete_view);
        app.middle_height.set(0);
        assert!(!observe(&s, &app).visible_view);
    }

    #[test]
    fn artist_metadata_with_only_pending_related_rows_is_not_visible_ready() {
        let (mut s, app) = ready();
        s.history.mode = MiddleMode::ArtistDetail {
            artist_id: "a".into(),
            artist_name: "Artist".into(),
            awaiting_seq: Some(9),
        };
        s.responses.insert(
            9,
            ServerMsg::ArtistDetail {
                artist: mkproto::Artist {
                    id: "a".into(),
                    name: "Artist".into(),
                    detail: None,
                    url: None,
                    artwork_url_small: None,
                    artwork_url_large: None,
                },
                top_songs: vec![],
            },
        );
        // The actual render model contains only the related-artists loading
        // placeholder: neither content rows nor a confirmed empty result.
        use mkpclient_runtime::views::{
            artist_detail_body_model, ArtistDetailExtrasInput, ArtistDetailResponseInput,
            ArtistDetailRow, ArtistDetailState,
        };
        let model = artist_detail_body_model(
            ArtistDetailResponseInput::new(Some(9), &s.responses),
            ArtistDetailExtrasInput::new(&s.artist_extras),
            0,
            true,
            70,
            5,
        );
        let ArtistDetailState::Loaded(loaded) = model.state else {
            panic!("artist metadata should render");
        };
        assert!(loaded.item_visual_indices.is_empty());
        assert!(loaded
            .rows
            .iter()
            .any(|r| matches!(r, ArtistDetailRow::SimilarLoading)));
        assert!(
            !observe(&s, &app).visible_view,
            "metadata plus Loading… must not latch the visible-row milestone"
        );

        s.artist_extras.set_similar("a".into(), vec![]);
        assert!(
            observe(&s, &app).visible_view,
            "confirmed empty artist view is ready"
        );
    }

    #[test]
    fn missing_response_is_not_an_empty_album() {
        let (mut s, app) = ready();
        s.history.mode = MiddleMode::AlbumDetail {
            album_id: "a".into(),
            album_title: "a".into(),
            awaiting_seq: Some(9),
        };
        assert!(!observe(&s, &app).complete_view);
        s.responses.insert(
            9,
            ServerMsg::Error {
                message: "missing".into(),
            },
        );
        assert!(!observe(&s, &app).complete_view);
    }
}
