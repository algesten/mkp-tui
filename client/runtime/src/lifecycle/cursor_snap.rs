//! Step 5 of the lifecycle: snap `cursor.middle` to the row whose
//! `song_id` matches `session.pending_cursor_song_id` once the data
//! lands.
//!
//! Spec §6: `desired_cursor_snap()` answers "given the pending target
//! id + the live data sources for the current middle mode, what row
//! should the cursor be at?"; `cursor_snap_action()` diffs against
//! the actual `cursor.middle` and returns Snap / ClearOnly / Noop.
//!
//! The `Resolution` enum carries enough state to distinguish three
//! cases the trampoline treats differently: rows ready + match found,
//! rows ready + no match (clear pending so we don't keep scanning),
//! and "still waiting for chunks." The data sources flow through input
//! projections so the memo cache hits between chunks.

use std::sync::Arc;

use imbl::Vector;

use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_responses::Responses;
use mkpclient_state_search::Search;
use mkpclient_state_ui_cursor::Cursor;
use mkpclient_state_ui_history::{MiddleMode, UiHistory};
use mkpclient_state_ui_session::UiSession;
use mkproto::{Album, Artist, SearchType, ServerMsg, Song};

use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum ModeProj {
    PlaylistSongs,
    SearchSongs,
    SearchAlbums,
    SearchArtists,
    AlbumDetail {
        awaiting_seq: Option<u64>,
    },
    /// Modes the snap doesn't apply to (ArtistDetail in the legacy
    /// behaviour). Trampoline still clears the pending flag — the
    /// memo just needs to know it can give up.
    Other,
}

impl ModeProj {
    pub fn new(m: &MiddleMode) -> Self {
        match m {
            MiddleMode::PlaylistSongs => Self::PlaylistSongs,
            MiddleMode::SearchResults { search_type, .. } => match search_type {
                SearchType::Song => Self::SearchSongs,
                SearchType::Album => Self::SearchAlbums,
                SearchType::Artist => Self::SearchArtists,
            },
            MiddleMode::AlbumDetail { awaiting_seq, .. } => Self::AlbumDetail {
                awaiting_seq: *awaiting_seq,
            },
            MiddleMode::ArtistDetail { .. } => Self::Other,
        }
    }
}

#[derive(drv::Input)]
pub struct CursorSnapInput<'a> {
    pub pending: Option<&'a std::sync::Arc<str>>,
    pub mode: ModeProj,
    pub plt_songs: &'a Vector<Option<Arc<Song>>>,
    pub search_songs: &'a Vector<Arc<Song>>,
    pub search_albums: &'a Vector<Arc<Album>>,
    pub search_artists: &'a Vector<Arc<Artist>>,
    /// Album-detail responses are looked up by `awaiting_seq` and
    /// projected as a single Arc — only this one entry matters,
    /// avoiding cache churn from unrelated responses landing.
    pub album_resp: Option<Arc<ServerMsg>>,
}

impl<'a> CursorSnapInput<'a> {
    pub fn new(
        session: &'a UiSession,
        history: &'a UiHistory,
        plt: &'a PlaylistTracks,
        search: &'a Search,
        responses: &'a Responses,
    ) -> Self {
        let mode = ModeProj::new(&history.mode);
        let album_resp = match &mode {
            ModeProj::AlbumDetail {
                awaiting_seq: Some(seq),
            } => responses.by_seq.get(seq).cloned(),
            _ => None,
        };
        Self {
            pending: session.pending_cursor_song_id.as_ref(),
            mode,
            plt_songs: &plt.songs,
            search_songs: &search.songs,
            search_albums: &search.albums,
            search_artists: &search.artists,
            album_resp,
        }
    }
}

#[derive(drv::Input)]
pub struct CursorMiddleInput {
    pub middle: usize,
}

impl CursorMiddleInput {
    pub fn new(c: &Cursor) -> Self {
        Self { middle: c.middle }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredCursorSnap {
    /// No pending target, nothing to do.
    NoTarget,
    /// Pending target set, but the rows for the current mode haven't
    /// arrived yet. Wait.
    AwaitRows,
    /// Pending target set, current mode doesn't support snap. Clear
    /// the pending flag so we don't keep scanning.
    Unsupported,
    /// Rows are loaded; target was found at this row.
    Found { row: usize },
    /// Rows are loaded; target wasn't in the list. Clear the flag.
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorSnapAction {
    Noop,
    /// `cursor.middle = row`, then clear `pending_cursor_song_id`.
    SnapAndClear {
        row: usize,
    },
    /// Just clear `pending_cursor_song_id` — either the cursor is
    /// already at the target row, or there's no target row to land on.
    ClearOnly,
}

#[drv::memo(single)]
pub fn desired_cursor_snap<'a>(input: CursorSnapInput<'a>) -> DesiredCursorSnap {
    let Some(target) = input.pending else {
        return DesiredCursorSnap::NoTarget;
    };
    match input.mode {
        ModeProj::PlaylistSongs => {
            if input.plt_songs.is_empty() {
                return DesiredCursorSnap::AwaitRows;
            }
            find_row_in_optional_arc(input.plt_songs, target)
        }
        ModeProj::SearchSongs => find_row_in_arc(input.search_songs, target, |s| &s.id),
        ModeProj::SearchAlbums => find_row_in_arc(input.search_albums, target, |a| &a.id),
        ModeProj::SearchArtists => find_row_in_arc(input.search_artists, target, |a| &a.id),
        ModeProj::AlbumDetail { .. } => match input.album_resp.as_deref() {
            // Once the AlbumDetail response is in, rows are "ready"
            // even if the songs list is empty — the legacy cleared
            // pending in that case rather than waiting.
            Some(ServerMsg::AlbumDetail { songs, .. }) => {
                match songs.iter().position(|s| s.id.as_str() == &**target) {
                    Some(row) => DesiredCursorSnap::Found { row },
                    None => DesiredCursorSnap::NotFound,
                }
            }
            Some(_) | None => DesiredCursorSnap::AwaitRows,
        },
        ModeProj::Other => DesiredCursorSnap::Unsupported,
    }
}

/// Playlist rows arrive as `ListBegin` (which sizes the list with
/// empty slots) followed by `ListChunk`s that fill them in. A target
/// that is not in the slots filled so far may still be in a chunk
/// on its way, so "not found" is only final once every slot has
/// landed.
fn find_row_in_optional_arc(rows: &Vector<Option<Arc<Song>>>, target: &str) -> DesiredCursorSnap {
    match rows
        .iter()
        .enumerate()
        .find(|(_, slot)| slot.as_ref().map(|s| s.id == target).unwrap_or(false))
        .map(|(i, _)| i)
    {
        Some(row) => DesiredCursorSnap::Found { row },
        None if rows.iter().any(|slot| slot.is_none()) => DesiredCursorSnap::AwaitRows,
        None => DesiredCursorSnap::NotFound,
    }
}

fn find_row_in_arc<T, F>(rows: &Vector<Arc<T>>, target: &str, id_of: F) -> DesiredCursorSnap
where
    F: Fn(&T) -> &String,
{
    if rows.is_empty() {
        return DesiredCursorSnap::AwaitRows;
    }
    match rows
        .iter()
        .enumerate()
        .find(|(_, item)| id_of(item.as_ref()) == target)
        .map(|(i, _)| i)
    {
        Some(row) => DesiredCursorSnap::Found { row },
        None => DesiredCursorSnap::NotFound,
    }
}

#[drv::memo(single)]
pub fn cursor_snap_action(
    desired: DesiredCursorSnap,
    cursor: CursorMiddleInput,
) -> CursorSnapAction {
    match desired {
        DesiredCursorSnap::NoTarget | DesiredCursorSnap::AwaitRows => CursorSnapAction::Noop,
        DesiredCursorSnap::Unsupported | DesiredCursorSnap::NotFound => CursorSnapAction::ClearOnly,
        DesiredCursorSnap::Found { row } => {
            if row == cursor.middle {
                CursorSnapAction::ClearOnly
            } else {
                CursorSnapAction::SnapAndClear { row }
            }
        }
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_cursor_snap(sources: &mut Sources) {
    let desired = desired_cursor_snap(CursorSnapInput::new(
        &sources.session,
        &sources.history,
        &sources.playlist_tracks,
        &sources.search,
        &sources.responses,
    ));
    let action = cursor_snap_action(desired, CursorMiddleInput::new(&sources.cursor));
    match action {
        CursorSnapAction::Noop => {}
        CursorSnapAction::SnapAndClear { row } => {
            sources.cursor.middle = row;
            sources.session.pending_cursor_song_id = None;
        }
        CursorSnapAction::ClearOnly => {
            sources.session.pending_cursor_song_id = None;
        }
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
    fn playlist_snap_waits_for_the_chunk_that_may_hold_the_target() {
        // ListBegin sized the list; nothing has landed yet.
        let mut rows: Vector<Option<Arc<Song>>> = std::iter::repeat_with(|| None).take(3).collect();
        assert_eq!(
            find_row_in_optional_arc(&rows, "c"),
            DesiredCursorSnap::AwaitRows
        );

        // First chunk landed without the target; more is coming.
        rows[0] = Some(Arc::new(song("a")));
        assert_eq!(
            find_row_in_optional_arc(&rows, "c"),
            DesiredCursorSnap::AwaitRows
        );

        // Target landed.
        rows[2] = Some(Arc::new(song("c")));
        assert_eq!(
            find_row_in_optional_arc(&rows, "c"),
            DesiredCursorSnap::Found { row: 2 }
        );

        // Everything landed and the target is not there: give up.
        rows[1] = Some(Arc::new(song("b")));
        assert_eq!(
            find_row_in_optional_arc(&rows, "zz"),
            DesiredCursorSnap::NotFound
        );
    }
}
