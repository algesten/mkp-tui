//! Step 8 of the lifecycle: reopen the search modal when the first
//! page of search results arrives empty.
//!
//! Legacy parity: mkp2 `app/server.rs` re-pushes `NavEntry::SearchInput`
//! whenever a search reply lands with zero results AND the user is on
//! the SearchResults middle pane. The modal then sits on top of the
//! "No results" middle pane with the just-typed query in the input
//! and visible at the head of the Recent list.
//!
//! Spec §6: `desired_search_reopen()` says "should the modal be
//! reopened?", `search_reopen_action()` diffs against the
//! `empty_reopen_done` gate to return `Open` exactly once. The
//! trampoline writes `empty_reopen_done = true` synchronously
//! (intent), mutates `Screen::SearchInput`, and fires
//! `LoadSearchHistory` so the persisted history reloads in time
//! for the painter to show it under "Recent".
//!
//! The `empty_reopen_done` flag is reset in
//! `Search::begin()`, so a subsequent search that also turns up
//! empty fires the reopen again (correct legacy behaviour).

use std::sync::Arc;

use mkpclient_state_search::Search;
use mkpclient_state_ui_cursor::{ColumnFocus, Cursor};
use mkpclient_state_ui_history::{MiddleMode, UiHistory};
use mkpclient_state_ui_screen::{Screen, SearchState};
use mkpclient_state_ui_session::UiSession;
use mkproto::SearchType;

use crate::dispatch;
use crate::drivers::Drivers;
use crate::sources::Sources;
use crate::views::SearchKind;

#[derive(drv::Input)]
pub struct ReopenSearchInput<'a> {
    pub first_page_empty: bool,
    pub empty_reopen_done: bool,
    pub term: &'a Arc<str>,
    /// Mirror of `mkproto::SearchType` — mkproto stays drv-free
    /// (guideline 11), so the projection round-trips through the
    /// local enum.
    pub search_type: SearchKind,
}

impl<'a> ReopenSearchInput<'a> {
    pub fn new(s: &'a Search) -> Self {
        Self {
            first_page_empty: s.first_page_empty(),
            empty_reopen_done: s.empty_reopen_done,
            term: &s.term,
            search_type: s.search_type.into(),
        }
    }
}

#[derive(drv::Input)]
pub struct ReopenHistoryInput {
    pub middle_is_search_results: bool,
}

impl ReopenHistoryInput {
    pub fn new(h: &UiHistory) -> Self {
        Self {
            middle_is_search_results: matches!(h.mode, MiddleMode::SearchResults { .. }),
        }
    }
}

#[derive(drv::Input)]
pub struct ReopenCursorInput {
    pub middle_focused: bool,
}

impl ReopenCursorInput {
    pub fn new(c: &Cursor) -> Self {
        Self {
            middle_focused: matches!(c.focus, ColumnFocus::Middle),
        }
    }
}

#[derive(drv::Input)]
pub struct ReopenScreenInput {
    pub on_now_playing: bool,
}

impl ReopenScreenInput {
    pub fn new(s: &Screen) -> Self {
        Self {
            on_now_playing: matches!(s, Screen::NowPlaying),
        }
    }
}

#[derive(drv::Input)]
pub struct ReopenSessionInput<'a> {
    pub backend_name: Option<&'a std::sync::Arc<str>>,
}

impl<'a> ReopenSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
        }
    }
}

/// "Should the search modal be reopened on top of the empty
/// SearchResults middle pane right now?" — the desired-state half
/// of the legacy auto-reopen behaviour. Independent of the current
/// `Screen` so it doesn't churn on dispatches that mutate
/// unrelated screens.
#[drv::memo(single)]
pub fn desired_search_modal_reopen<'a>(
    search: ReopenSearchInput<'a>,
    history: ReopenHistoryInput,
    cursor: ReopenCursorInput,
) -> bool {
    !search.empty_reopen_done
        && search.first_page_empty
        && history.middle_is_search_results
        && cursor.middle_focused
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchReopenAction {
    Noop,
    Open {
        term: String,
        search_type: SearchKind,
        load_history_for: Option<String>,
    },
}

/// Diff `desired_search_modal_reopen` against the live `Screen`:
/// only emit `Open` when the user is currently on `NowPlaying`,
/// otherwise a more important modal (pairing, error, server-lost)
/// is on screen and the reopen would clobber it.
#[drv::memo(single)]
pub fn search_reopen_action<'a, 'b>(
    desired_open: bool,
    screen: ReopenScreenInput,
    search: ReopenSearchInput<'a>,
    session: ReopenSessionInput<'b>,
) -> SearchReopenAction {
    if !desired_open || !screen.on_now_playing {
        return SearchReopenAction::Noop;
    }
    SearchReopenAction::Open {
        term: search.term.to_string(),
        search_type: search.search_type,
        load_history_for: session.backend_name.map(|s| s.to_string()),
    }
}

pub fn apply_search_reopen(sources: &mut Sources, drivers: &Drivers) {
    let desired = desired_search_modal_reopen(
        ReopenSearchInput::new(&sources.search),
        ReopenHistoryInput::new(&sources.history),
        ReopenCursorInput::new(&sources.cursor),
    );
    let action = search_reopen_action(
        desired,
        ReopenScreenInput::new(&sources.screen),
        ReopenSearchInput::new(&sources.search),
        ReopenSessionInput::new(&sources.session),
    );
    let SearchReopenAction::Open {
        term,
        search_type,
        load_history_for,
    } = action
    else {
        return;
    };
    // Sync intent: flip the gate before the screen mutation so the
    // next tick's action memo returns Noop even before LoadHistory
    // completes.
    sources.search.empty_reopen_done = true;
    sources.screen = Screen::SearchInput(SearchState {
        input: Arc::from(term),
        last_type: kind_to_proto(search_type),
        history: imbl::Vector::new(),
        history_selected: None,
    });
    if let Some(backend) = load_history_for {
        dispatch::request_load_search_history(sources, drivers, backend);
    }
}

fn kind_to_proto(k: SearchKind) -> SearchType {
    match k {
        SearchKind::Song => SearchType::Song,
        SearchKind::Album => SearchType::Album,
        SearchKind::Artist => SearchType::Artist,
    }
}
