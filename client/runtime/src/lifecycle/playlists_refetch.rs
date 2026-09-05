//! Refetch lifecycle: when a `PlaylistMutation::Modified` broadcast
//! has flipped `playlists.stale` (or `playlist_tracks.stale` for the
//! currently-loaded playlist), re-issue the appropriate request.
//!
//! Spec §5 / §6: each refetch is a `desired_*` + `*_action` pair. The
//! "in-flight" gate uses the protocol's existing identity rather than
//! parallel bookkeeping:
//!
//! - `GetPlaylists` correlates its initial response by `seq`, then
//!   streams track counts under `task_id`. The source carries both
//!   pending identities; the action waits for both phases to finish.
//! - `GetPlaylist` streams via `ListBegin` / `ListChunk` broadcasts
//!   correlated by `task_id`. The source carries
//!   `pending_task: Option<TaskId>`; the action diffs against that.
//!
//! The trampolines write intent synchronously (clear `stale`, set the
//! pending field) before enqueuing the request, matching the
//! execute-pattern (§4 "The execute pattern").

use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_playlists::Playlists;
use mkproto::{ClientMsg, TaskId};

use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct PlaylistsRefetchInput {
    pub stale: bool,
    pub pending_request: Option<u64>,
    pub pending_task: Option<TaskId>,
}

impl PlaylistsRefetchInput {
    pub fn new(p: &Playlists) -> Self {
        Self {
            stale: p.stale,
            pending_request: p.pending_request,
            pending_task: p.pending_task,
        }
    }
}

#[derive(drv::Input)]
pub struct PlaylistTracksRefetchInput<'a> {
    pub stale: bool,
    pub playlist_id: Option<&'a std::sync::Arc<str>>,
    pub focus: usize,
    pub pending_task: Option<TaskId>,
}

impl<'a> PlaylistTracksRefetchInput<'a> {
    pub fn new(t: &'a PlaylistTracks) -> Self {
        Self {
            stale: t.stale,
            playlist_id: t.playlist_id.as_ref(),
            focus: t.focus,
            pending_task: t.pending_task,
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistsRefetchAction {
    Noop,
    /// Fire a fresh `GetPlaylists`.
    Fire,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistTracksRefetchAction {
    Noop,
    /// Fire a fresh `GetPlaylist { id, focus }`.
    Fire {
        id: String,
        focus: usize,
    },
}

#[drv::memo(single)]
pub fn playlists_refetch_action(input: PlaylistsRefetchInput) -> PlaylistsRefetchAction {
    if input.stale && input.pending_request.is_none() && input.pending_task.is_none() {
        PlaylistsRefetchAction::Fire
    } else {
        PlaylistsRefetchAction::Noop
    }
}

#[drv::memo(single)]
pub fn playlist_tracks_refetch_action<'a>(
    input: PlaylistTracksRefetchInput<'a>,
) -> PlaylistTracksRefetchAction {
    let Some(id) = input.playlist_id else {
        return PlaylistTracksRefetchAction::Noop;
    };
    if !input.stale || input.pending_task.is_some() {
        return PlaylistTracksRefetchAction::Noop;
    }
    PlaylistTracksRefetchAction::Fire {
        id: id.to_string(),
        focus: input.focus,
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_playlists_refetch(sources: &mut Sources) {
    let action = playlists_refetch_action(PlaylistsRefetchInput::new(&sources.playlists));
    if !matches!(action, PlaylistsRefetchAction::Fire) {
        return;
    }
    let task_id = sources.requests.alloc_task_id();
    let seq = sources
        .requests
        .push(ClientMsg::GetPlaylists, Some(task_id));
    sources.playlists.pending_request = Some(seq);
    sources.playlists.pending_task = Some(task_id);
    sources.playlists.stale = false;
}

pub fn apply_playlist_tracks_refetch(sources: &mut Sources) {
    let action =
        playlist_tracks_refetch_action(PlaylistTracksRefetchInput::new(&sources.playlist_tracks));
    let PlaylistTracksRefetchAction::Fire { id, focus } = action else {
        return;
    };
    let task_id = sources.requests.alloc_task_id();
    sources.playlist_tracks.pending_task = Some(task_id);
    sources.playlist_tracks.stale = false;
    sources
        .requests
        .push(ClientMsg::GetPlaylist { id, focus }, Some(task_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlists_refetch_fires_when_stale_and_no_pending() {
        let p = Playlists {
            stale: true,
            ..Default::default()
        };
        let action = playlists_refetch_action(PlaylistsRefetchInput::new(&p));
        assert_eq!(action, PlaylistsRefetchAction::Fire);
    }

    #[test]
    fn playlists_refetch_noop_when_pending() {
        let p = Playlists {
            stale: true,
            pending_request: Some(7),
            ..Default::default()
        };
        let action = playlists_refetch_action(PlaylistsRefetchInput::new(&p));
        assert_eq!(action, PlaylistsRefetchAction::Noop);
    }

    #[test]
    fn playlists_refetch_noop_while_count_stream_is_pending() {
        let p = Playlists {
            stale: true,
            pending_task: Some(7),
            ..Default::default()
        };
        let action = playlists_refetch_action(PlaylistsRefetchInput::new(&p));
        assert_eq!(action, PlaylistsRefetchAction::Noop);
    }

    #[test]
    fn playlists_refetch_noop_when_not_stale() {
        let p = Playlists::default();
        let action = playlists_refetch_action(PlaylistsRefetchInput::new(&p));
        assert_eq!(action, PlaylistsRefetchAction::Noop);
    }

    #[test]
    fn playlist_tracks_refetch_fires_with_id_and_focus() {
        let t = PlaylistTracks {
            playlist_id: Some("p1".into()),
            focus: 4,
            stale: true,
            ..Default::default()
        };
        let action = playlist_tracks_refetch_action(PlaylistTracksRefetchInput::new(&t));
        assert_eq!(
            action,
            PlaylistTracksRefetchAction::Fire {
                id: "p1".into(),
                focus: 4
            }
        );
    }

    #[test]
    fn playlist_tracks_refetch_noop_when_no_loaded_playlist() {
        let t = PlaylistTracks {
            stale: true,
            ..Default::default()
        };
        let action = playlist_tracks_refetch_action(PlaylistTracksRefetchInput::new(&t));
        assert_eq!(action, PlaylistTracksRefetchAction::Noop);
    }

    #[test]
    fn playlist_tracks_refetch_noop_when_pending_task() {
        let t = PlaylistTracks {
            playlist_id: Some("p1".into()),
            stale: true,
            pending_task: Some(9),
            ..Default::default()
        };
        let action = playlist_tracks_refetch_action(PlaylistTracksRefetchInput::new(&t));
        assert_eq!(action, PlaylistTracksRefetchAction::Noop);
    }
}

// ─── search ─────────────────────────────────────────────────────────
//
// A search interrupted by a dropped link has the same shape as an
// interrupted track list: rows may already be on screen, but the reply
// that would have completed it is gone. Unlike the track list, the
// search pane's spinner reads `first_page_received` rather than the
// task handle, so simply forgetting the handle leaves it reading
// "Searching…" for the life of the process.

#[derive(drv::Input)]
pub struct SearchRefetchInput<'a> {
    pub stale: bool,
    pub term: &'a std::sync::Arc<str>,
    /// Mirror of `mkproto::SearchType` — mkproto stays drv-free, so
    /// the projection round-trips through the local enum, as
    /// `search_reopen` does.
    pub search_type: crate::views::SearchKind,
    pub pending_task: Option<TaskId>,
}

impl<'a> SearchRefetchInput<'a> {
    pub fn new(s: &'a mkpclient_state_search::Search) -> Self {
        Self {
            stale: s.stale,
            term: &s.term,
            search_type: s.search_type.into(),
            pending_task: s.task_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchRefetchAction {
    Noop,
    Fire {
        term: String,
        search_type: crate::views::SearchKind,
    },
}

#[drv::memo(single)]
pub fn search_refetch_action<'a>(input: SearchRefetchInput<'a>) -> SearchRefetchAction {
    if !input.stale || input.pending_task.is_some() {
        return SearchRefetchAction::Noop;
    }
    if input.term.is_empty() {
        return SearchRefetchAction::Noop;
    }
    SearchRefetchAction::Fire {
        term: input.term.to_string(),
        search_type: input.search_type,
    }
}

pub fn apply_search_refetch(sources: &mut Sources) {
    let action = search_refetch_action(SearchRefetchInput::new(&sources.search));
    let SearchRefetchAction::Fire { term, search_type } = action else {
        return;
    };
    let search_type: mkproto::SearchType = search_type.into();
    let task_id = sources.requests.alloc_task_id();
    sources
        .search
        .begin(task_id, std::sync::Arc::from(term.as_str()), search_type);
    sources
        .requests
        .push(ClientMsg::Search { term, search_type }, Some(task_id));
    if let mkpclient_state_ui_history::MiddleMode::SearchResults {
        task_id: mode_task, ..
    } = &mut sources.history.mode
    {
        *mode_task = Some(task_id);
    }
}
