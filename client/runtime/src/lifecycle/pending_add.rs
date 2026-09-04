//! Step 6 of the lifecycle: fire a deferred `AddToPlaylist` once the
//! `CreatePlaylist` response promotes the picker's
//! `pending_create_add` breadcrumb into a real playlist on the server.
//!
//! Spec §6: `desired_pending_add()` answers "given the breadcrumb +
//! the live playlist list, what add should be in flight?";
//! `pending_add_action()` diffs against the actual breadcrumb state
//! (the user-decision flag is itself the in-flight tracker — clear
//! it on apply and the next tick's desired returns Idle).

use std::sync::Arc;

use imbl::Vector;
use mkproto::Playlist;

use mkpclient_state_playlists::Playlists;
use mkpclient_state_ui_picker::UiPicker;

use crate::dispatch::fire_deferred_add_to_playlist_pub;
use crate::drivers::Drivers;
use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct PendingAddInput<'a> {
    pub pending_name: Option<&'a std::sync::Arc<str>>,
}

impl<'a> PendingAddInput<'a> {
    pub fn new(p: &'a UiPicker) -> Self {
        Self {
            pending_name: p.pending_create_add.as_ref().map(|p| &p.name),
        }
    }
}

#[derive(drv::Input)]
pub struct PlaylistsByNameInput<'a> {
    pub items: &'a Vector<Arc<Playlist>>,
}

impl<'a> PlaylistsByNameInput<'a> {
    pub fn new(p: &'a Playlists) -> Self {
        Self { items: &p.items }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredPendingAdd {
    /// No breadcrumb set, or the matching playlist hasn't landed yet.
    Idle,
    /// `CreatePlaylist` produced the matching playlist; the runtime
    /// should fire the deferred `AddToPlaylist` to this id.
    Ready { playlist_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAddAction {
    Noop,
    Fire { playlist_id: String },
}

#[drv::memo(single)]
pub fn desired_pending_add<'a, 'b>(
    pending: PendingAddInput<'a>,
    playlists: PlaylistsByNameInput<'b>,
) -> DesiredPendingAdd {
    let Some(name) = pending.pending_name else {
        return DesiredPendingAdd::Idle;
    };
    let Some(matched) = playlists.items.iter().find(|p| p.name.as_str() == &**name) else {
        return DesiredPendingAdd::Idle;
    };
    DesiredPendingAdd::Ready {
        playlist_id: matched.id.clone(),
    }
}

#[drv::memo(single)]
pub fn pending_add_action(desired: DesiredPendingAdd) -> PendingAddAction {
    match desired {
        DesiredPendingAdd::Idle => PendingAddAction::Noop,
        DesiredPendingAdd::Ready { playlist_id } => PendingAddAction::Fire { playlist_id },
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_pending_add(sources: &mut Sources, _drivers: &Drivers) {
    let desired = desired_pending_add(
        PendingAddInput::new(&sources.picker),
        PlaylistsByNameInput::new(&sources.playlists),
    );
    let action = pending_add_action(desired);
    let PendingAddAction::Fire { playlist_id } = action else {
        return;
    };
    // Sync intent write: take the breadcrumb so next tick's desired
    // is Idle. The last-add-persist lifecycle mirrors the resulting
    // `picker.last_add_playlist` to disk on the next tick.
    fire_deferred_add_to_playlist_pub(sources, playlist_id);
}
