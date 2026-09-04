//! Search-history push lifecycle: append `(query, search_type, ts)`
//! to the per-backend `search_history` file once per submitted
//! search.
//!
//! Spec §5/§6: the action memo gates on `search.task_id` being
//! `Some` and different from `persist.last_pushed_search_task`. Each
//! `Search::begin()` allocates a fresh task id, so a fresh task id
//! is exactly the "user just submitted" signal — no string-equality
//! gymnastics on the term, no race against the first-page reply.
//!
//! Replaces the reactive `PersistPushSearchHistory` previously fired
//! from `tui::input::translate_search_input` after `SearchSubmit`.

use std::sync::Arc;

use mkpclient_driver_persist_core::{Persist, PersistCmd};
use mkpclient_state_search::Search;
use mkpclient_state_ui_session::UiSession;
use mkproto::{SearchType, TaskId};

use crate::drivers::Drivers;
use crate::queries::search_type_str;
use crate::sources::Sources;

// ─── inputs ────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct PushSearchInput<'a> {
    pub task_id: Option<TaskId>,
    pub term: &'a Arc<str>,
    pub search_type: SearchTypeProj,
}

/// drv-friendly mirror of `SearchType`. mkproto stays drv-free
/// (guideline 11), so the projection round-trips through a local
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, drv::Input)]
pub enum SearchTypeProj {
    Song,
    Album,
    Artist,
}

impl From<SearchType> for SearchTypeProj {
    fn from(t: SearchType) -> Self {
        match t {
            SearchType::Song => Self::Song,
            SearchType::Album => Self::Album,
            SearchType::Artist => Self::Artist,
        }
    }
}

impl From<SearchTypeProj> for SearchType {
    fn from(t: SearchTypeProj) -> Self {
        match t {
            SearchTypeProj::Song => Self::Song,
            SearchTypeProj::Album => Self::Album,
            SearchTypeProj::Artist => Self::Artist,
        }
    }
}

impl<'a> PushSearchInput<'a> {
    pub fn new(s: &'a Search) -> Self {
        Self {
            task_id: s.task_id,
            term: &s.term,
            search_type: s.search_type.into(),
        }
    }
}

#[derive(drv::Input)]
pub struct PushSessionInput<'a> {
    pub backend_name: Option<&'a Arc<str>>,
}

impl<'a> PushSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct PushPersistInput {
    pub last_pushed_task: Option<TaskId>,
}

impl PushPersistInput {
    pub fn new(p: &Persist) -> Self {
        Self {
            last_pushed_task: p.last_pushed_search_task,
        }
    }
}

// ─── memos ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushAction {
    Noop,
    Push {
        task_id: TaskId,
        query: String,
        search_type: String,
    },
}

#[drv::memo(single)]
pub fn push_action<'a, 'b>(
    search: PushSearchInput<'a>,
    session: PushSessionInput<'b>,
    persist: PushPersistInput,
) -> PushAction {
    if session.backend_name.is_none() {
        return PushAction::Noop;
    }
    let Some(task_id) = search.task_id else {
        return PushAction::Noop;
    };
    if persist.last_pushed_task == Some(task_id) {
        return PushAction::Noop;
    }
    if search.term.is_empty() {
        return PushAction::Noop;
    }
    PushAction::Push {
        task_id,
        query: search.term.to_string(),
        search_type: search_type_str(search.search_type.into()).to_string(),
    }
}

// ─── trampoline ────────────────────────────────────────────────────

pub fn apply_search_history_push(sources: &mut Sources, drivers: &Drivers) {
    let action = push_action(
        PushSearchInput::new(&sources.search),
        PushSessionInput::new(&sources.session),
        PushPersistInput::new(&sources.persist),
    );
    let PushAction::Push {
        task_id,
        query,
        search_type,
    } = action
    else {
        return;
    };
    let Some(backend) = sources.session.backend_name.as_ref().map(|s| s.to_string()) else {
        return;
    };
    sources.persist.last_pushed_search_task = Some(task_id);
    drivers.persist.execute([&PersistCmd::PushSearchHistory {
        backend,
        query,
        search_type,
    }]);
}
