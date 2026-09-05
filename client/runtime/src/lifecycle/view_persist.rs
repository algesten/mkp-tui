//! View-persist lifecycle: keep the on-disk `last_view` continuously
//! in sync with the current middle pane.
//!
//! Spec §5/§6: `desired_view_key()` projects the user's current
//! middle-pane location into a mode-only fingerprint (a `SavedViewKey`
//! — no cursor, no offset, no per-request `awaiting_seq`);
//! `view_save_action()` diffs that against `persist.last_view_saved_key`
//! and returns Save / Noop. The trampoline writes the new key
//! synchronously and ships `SaveView` to the persist driver.
//!
//! Cursor moves don't trigger saves — the diff is mode-only by
//! design, so j/k presses don't burn a disk write each. The cursor
//! value that ends up on disk is whatever was current at the moment
//! the mode last changed (typically 0 after a fresh drill, the
//! restored value right after restore applies).
//!
//! The `auto_restored_view` gate keeps us from saving stale `mode`
//! state to a freshly-set backend before `apply_restore` has had a
//! chance to overwrite it.

use std::sync::Arc;

use mkpclient_driver_persist_core::{Persist, PersistCmd, SavedViewKey};
use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_ui_history::{MiddleMode, UiHistory};
use mkpclient_state_ui_session::UiSession;

use crate::drivers::Drivers;
use crate::sources::Sources;

// ─── projections ───────────────────────────────────────────────────

/// Drv-friendly projection of just the parts of `MiddleMode` that
/// participate in the saved-view identity. Drops `awaiting_seq` and
/// `task_id` (request bookkeeping that churns when re-issuing the
/// same logical view) so the memo cache only invalidates on real
/// navigation.
#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum ModeIdentity {
    PlaylistSongs,
    Search { query: String, search_type: String },
    AlbumDetail { album_id: String },
    ArtistDetail { artist_id: String },
}

impl ModeIdentity {
    pub fn new(m: &MiddleMode) -> Self {
        match m {
            MiddleMode::PlaylistSongs => Self::PlaylistSongs,
            MiddleMode::SearchResults {
                term, search_type, ..
            } => Self::Search {
                query: term.clone(),
                search_type: crate::queries::search_type_str(*search_type).to_string(),
            },
            MiddleMode::AlbumDetail { album_id, .. } => Self::AlbumDetail {
                album_id: album_id.clone(),
            },
            MiddleMode::ArtistDetail { artist_id, .. } => Self::ArtistDetail {
                artist_id: artist_id.clone(),
            },
        }
    }
}

#[derive(drv::Input)]
pub struct ViewModeInput {
    pub mode: ModeIdentity,
}

impl ViewModeInput {
    pub fn new(h: &UiHistory) -> Self {
        Self {
            mode: ModeIdentity::new(&h.mode),
        }
    }
}

#[derive(drv::Input)]
pub struct ViewPlaylistInput<'a> {
    pub playlist_id: Option<&'a Arc<str>>,
}

impl<'a> ViewPlaylistInput<'a> {
    pub fn new(p: &'a PlaylistTracks) -> Self {
        Self {
            playlist_id: p.playlist_id.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct ViewSessionInput<'a> {
    pub backend_name: Option<&'a Arc<str>>,
    pub auto_restored_view: bool,
}

impl<'a> ViewSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
            auto_restored_view: s.auto_restored_view,
        }
    }
}

#[derive(drv::Input)]
pub struct ViewLastSavedInput<'a> {
    pub last: Option<&'a SavedViewKey>,
}

impl<'a> ViewLastSavedInput<'a> {
    pub fn new(p: &'a Persist) -> Self {
        Self {
            last: p.last_view_saved_key.as_ref(),
        }
    }
}

// ─── memos ─────────────────────────────────────────────────────────

#[drv::memo(single)]
pub fn desired_view_key<'a>(
    mode: ViewModeInput,
    playlist: ViewPlaylistInput<'a>,
) -> Option<SavedViewKey> {
    match mode.mode {
        ModeIdentity::PlaylistSongs => playlist.playlist_id.map(|id| SavedViewKey::Playlist {
            playlist_id: id.to_string(),
        }),
        ModeIdentity::Search { query, search_type } => {
            Some(SavedViewKey::Search { query, search_type })
        }
        ModeIdentity::AlbumDetail { album_id } => Some(SavedViewKey::AlbumDetail { album_id }),
        ModeIdentity::ArtistDetail { artist_id } => Some(SavedViewKey::ArtistDetail { artist_id }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewSaveAction {
    Noop,
    Save,
}

#[drv::memo(single)]
pub fn view_save_action<'a, 'b>(
    desired: Option<SavedViewKey>,
    session: ViewSessionInput<'a>,
    last: ViewLastSavedInput<'b>,
) -> ViewSaveAction {
    // Don't save while disconnected (no backend to write under) or
    // before restore has had a chance to overwrite stale `mode`.
    if session.backend_name.is_none() || !session.auto_restored_view {
        return ViewSaveAction::Noop;
    }
    let Some(d) = desired else {
        return ViewSaveAction::Noop;
    };
    if last.last == Some(&d) {
        ViewSaveAction::Noop
    } else {
        ViewSaveAction::Save
    }
}

// ─── trampoline ────────────────────────────────────────────────────

pub fn apply_view_persist(sources: &mut Sources, drivers: &Drivers) {
    let action = view_save_action(
        desired_view_key(
            ViewModeInput::new(&sources.history),
            ViewPlaylistInput::new(&sources.playlist_tracks),
        ),
        ViewSessionInput::new(&sources.session),
        ViewLastSavedInput::new(&sources.persist),
    );
    if !matches!(action, ViewSaveAction::Save) {
        return;
    }
    let Some(view) = crate::dispatch::build_saved_view(sources) else {
        return;
    };
    let Some(key) = crate::dispatch::current_view_key(sources) else {
        return;
    };
    // Sync intent: write the key before firing so the next tick's
    // memo returns Noop.
    sources.persist.last_view_saved_key = Some(view.key());
    drivers
        .persist
        .execute([&PersistCmd::SaveView { key, view }]);
}
