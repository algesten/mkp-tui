//! Last-add-playlist persistence lifecycle: keep the on-disk
//! `last_add_playlist` in sync with `picker.last_add_playlist`.
//!
//! Spec §5/§6: `desired_last_add_id()` projects the current value;
//! `last_add_save_action()` diffs against `persist.last_add_playlist_saved`
//! and returns Save / Noop. The trampoline writes the new value
//! synchronously and ships `SaveLastAddPlaylist` to the persist
//! driver.
//!
//! Replaces the reactive saves that previously fired in
//! `tui::input::translate_playlist_picker` and
//! `lifecycle::pending_add::apply_pending_add`. The pattern matches
//! `view_persist`: any path that mutates the in-memory user-decision
//! gets mirrored to disk by the lifecycle, regardless of how it was
//! reached.

use std::sync::Arc;

use mkpclient_driver_persist_core::{Persist, PersistCmd};
use mkpclient_state_ui_picker::UiPicker;
use mkpclient_state_ui_session::UiSession;

use crate::drivers::Drivers;
use crate::sources::Sources;

// ─── inputs ────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct LastAddPickerInput<'a> {
    pub last_add: Option<&'a Arc<str>>,
}

impl<'a> LastAddPickerInput<'a> {
    pub fn new(p: &'a UiPicker) -> Self {
        Self {
            last_add: p.last_add_playlist.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct LastAddSessionInput<'a> {
    pub backend_name: Option<&'a Arc<str>>,
}

impl<'a> LastAddSessionInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            backend_name: s.backend_name.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct LastAddSavedInput<'a> {
    pub saved: Option<&'a String>,
}

impl<'a> LastAddSavedInput<'a> {
    pub fn new(p: &'a Persist) -> Self {
        Self {
            saved: p.last_add_playlist_saved.as_ref(),
        }
    }
}

// ─── memos ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastAddSaveAction {
    Noop,
    Save { playlist_id: String },
}

#[drv::memo(single)]
pub fn last_add_save_action<'a, 'b, 'c>(
    picker: LastAddPickerInput<'a>,
    session: LastAddSessionInput<'b>,
    saved: LastAddSavedInput<'c>,
) -> LastAddSaveAction {
    if session.backend_name.is_none() {
        return LastAddSaveAction::Noop;
    }
    let Some(current) = picker.last_add else {
        return LastAddSaveAction::Noop;
    };
    let cur_str: &str = current;
    if saved.saved.map(|s| s.as_str()) == Some(cur_str) {
        LastAddSaveAction::Noop
    } else {
        LastAddSaveAction::Save {
            playlist_id: cur_str.to_string(),
        }
    }
}

// ─── trampoline ────────────────────────────────────────────────────

pub fn apply_last_add_persist(sources: &mut Sources, drivers: &Drivers) {
    let action = last_add_save_action(
        LastAddPickerInput::new(&sources.picker),
        LastAddSessionInput::new(&sources.session),
        LastAddSavedInput::new(&sources.persist),
    );
    let LastAddSaveAction::Save { playlist_id } = action else {
        return;
    };
    let Some(backend) = sources.session.backend_name.as_ref().map(|s| s.to_string()) else {
        return;
    };
    sources.persist.last_add_playlist_saved = Some(playlist_id.clone());
    drivers.persist.execute([&PersistCmd::SaveLastAddPlaylist {
        backend,
        playlist_id,
    }]);
}
