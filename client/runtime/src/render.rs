//! Render phase — Phase 4 of the main loop per EXAMPLE-ARCH.md
//! §"The main loop".
//!
//! Runs each tracked view memo, encodes the output via MessagePack,
//! and hands the payload off to the UI bridge driver. The driver
//! dedups against its in-flight source and ships changed pushes
//! asynchronously to its native worker.
//!
//! The runtime crate orchestrates this (it has both the view memos
//! and the `rmp-serde` dep). The driver crate (which has neither)
//! exposes the typed `execute` boundary. This is the same split used
//! everywhere else: runtime computes, driver acts.
//!
//! Modals (action menu, search input, confirms, etc.) push
//! `Option<Model>` — `Some(model)` while their matching `Screen`
//! variant is active, `None` otherwise. SwiftUI binds
//! `.sheet(isPresented:)` to payload presence so the modal flips
//! closed when the runtime emits `None`.

use mkpclient_driver_ui_bridge_core::{UiBridgeDriver, UiBridgeState, ViewKind};
use mkpclient_state_ui_cursor::ColumnFocus;
use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_screen::Screen;
use mkpclient_state_ui_selection::SelectionContext;

use crate::sources::Sources;
use crate::views::{
    self, ActionModalInput, ActivityInput, AlbumDetailResponseInput, ArtistDetailExtrasInput,
    ArtistDetailResponseInput, ConfirmDeletePlaylistInput, ConfirmRemoveInput, ErrorModalInput,
    FilterStateInput, InputModalInput, InputModalKind, LeftUiInput, MiddleHeaderUiInput,
    MiddleModeView, PairingModalInput, PeerIdInput, PendingPlaylistsInput, PickerOverride,
    PlaylistPickerHintInput, PlaylistTracksDurationInput, PlaylistTracksFocusInput,
    PlaylistTracksInput, PlaylistTracksPendingInput, PlaylistsInput, PreConnectInput, QueueInput,
    SearchCountsInput, SearchInputModalInput, SearchResultsInput, SelectionActionModalInput,
    SelectionBarContext, SelectionBarSongsInput, ServerLabelInput, ServerLostModalInput,
    ServerNowPlayingInput, ServerPickerModalInput, ServerPositionInput, UiPreviewInput,
};
use crate::PeerIdentity;

/// Run every tracked view memo and push changed payloads into the
/// bridge driver. Per EXAMPLE-ARCH.md §"The main loop" Phase 4.
///
/// New views are added by extending `ViewKind` (in the driver core)
/// and adding a block here that runs the memo, encodes, and calls
/// `driver.execute`.
pub fn run_render(
    sources: &Sources,
    peer: &PeerIdentity,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    push_now_playing(sources, peer, driver, state);
    push_left_column(sources, driver, state);
    push_middle_header(sources, driver, state);
    push_queue(sources, driver, state);
    push_playlist_tracks(sources, driver, state);
    push_search_results(sources, driver, state);
    push_album_detail(sources, driver, state);
    push_artist_detail(sources, driver, state);
    push_selection_bar(sources, driver, state);
    push_pre_connect(sources, driver, state);
    // Modals — each pushes Option<Model>.
    push_action_modal(sources, driver, state);
    push_confirm_delete(sources, driver, state);
    push_confirm_remove(sources, driver, state);
    push_error_modal(sources, driver, state);
    push_filter_input(sources, driver, state);
    push_help_overlay(sources, driver, state);
    push_input_modal(sources, driver, state);
    push_pairing_modal(sources, driver, state);
    push_playlist_action_modal(sources, driver, state);
    push_playlist_picker_hint(sources, driver, state);
    push_search_input_modal(sources, driver, state);
    push_selection_action_modal(sources, driver, state);
    push_server_lost_modal(sources, driver, state);
    push_server_picker_modal(sources, driver, state);
}

// ─── helpers ────────────────────────────────────────────────────────

/// iOS doesn't have a per-column keyboard focus the way the TUI
/// does — every "is the column focused?" memo input collapses to
/// `false` for bridge-shipped models. That keeps the model output
/// stable and lets SwiftUI render its own focus/selection scheme.
const IOS_FOCUSED: bool = false;

/// Layout-dependent params (note wrapping width, etc.) collapse to
/// "no constraint" for bridge models. SwiftUI does its own layout.
const IOS_BODY_WIDTH: u16 = u16::MAX;
const IOS_TIME_W: usize = 5;

fn push<T: serde::Serialize>(
    kind: ViewKind,
    model: &T,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    match rmp_serde::to_vec_named(model) {
        Ok(payload) => driver.execute(kind, payload, state),
        Err(err) => log::error!("render: encode {kind:?} failed: {err}"),
    }
}

// ─── per-view pushes ────────────────────────────────────────────────

fn push_now_playing(
    sources: &Sources,
    peer: &PeerIdentity,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    push(
        ViewKind::NowPlaying,
        &views::now_playing_model(
            ServerNowPlayingInput::new(&sources.server),
            UiPreviewInput::new(&sources.preview),
            ActivityInput::new(&sources.activity),
            PeerIdInput::new(peer),
        ),
        driver,
        state,
    );
}

fn push_left_column(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let picker = match &sources.screen {
        Screen::PlaylistPicker { selected, .. } => Some(PickerOverride {
            selected: *selected,
        }),
        _ => None,
    };
    let model = views::left_column_model(
        PlaylistsInput::new(&sources.playlists),
        PendingPlaylistsInput::new(&sources.pending_playlists),
        PlaylistTracksFocusInput::new(&sources.playlist_tracks),
        ServerLabelInput::new(&sources.link, &sources.discovery, &sources.probes),
        LeftUiInput {
            backend_name: sources.session.backend_name.as_deref(),
            column_focused: sources.cursor.focus == ColumnFocus::Left,
            left_selected: sources.cursor.left,
            playlist_filter: &sources.filter.playlist,
            picker,
            viewing_active: matches!(sources.history.mode, MiddleMode::PlaylistSongs),
        },
    );
    push(ViewKind::LeftColumn, &model, driver, state);
}

fn push_middle_header(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let mode = middle_mode_view(&sources.history.mode);
    let album_total_secs = match &sources.history.mode {
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            views::album_detail_total_secs(*awaiting_seq, &sources.responses)
        }
        _ => 0.0,
    };
    let model = views::middle_header_model(
        mode,
        SearchCountsInput::new(&sources.search),
        PlaylistTracksDurationInput::new(&sources.playlist_tracks),
        album_total_secs,
        MiddleHeaderUiInput {
            focused: sources.cursor.focus == ColumnFocus::Middle,
            in_selection: sources.selection.context == Some(SelectionContext::Middle),
            middle_filter_empty: sources.filter.middle.is_empty(),
            history_back_count: sources.history.back.len(),
            history_fwd_count: sources.history.forward.len(),
        },
    );
    push(ViewKind::MiddleHeader, &model, driver, state);
}

fn push_queue(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = views::queue_column_model(
        QueueInput::new(&sources.queue),
        ServerPositionInput::new(&sources.server),
        sources.cursor.queue,
        &sources.filter.queue,
        sources.selection.context == Some(SelectionContext::Queue),
        &sources.selection.selected,
        IOS_FOCUSED,
    );
    push(ViewKind::Queue, &model, driver, state);
}

fn push_playlist_tracks(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = views::playlist_tracks_body_model(
        PlaylistTracksInput::new(&sources.playlist_tracks),
        PlaylistTracksPendingInput::new(&sources.pending_playlists),
        &sources.filter.middle,
        sources.selection.context == Some(SelectionContext::Middle),
        &sources.selection.selected,
        sources.cursor.middle,
        IOS_FOCUSED,
    );
    push(ViewKind::PlaylistTracks, &model, driver, state);
}

fn push_search_results(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = views::search_results_body_model(
        SearchResultsInput::new(&sources.search),
        &sources.filter.middle,
        sources.cursor.middle,
        IOS_FOCUSED,
        sources.selection.context == Some(SelectionContext::Middle),
        &sources.selection.selected,
    );
    push(ViewKind::SearchResults, &model, driver, state);
}

fn push_album_detail(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let awaiting_seq = match &sources.history.mode {
        MiddleMode::AlbumDetail { awaiting_seq, .. } => *awaiting_seq,
        _ => None,
    };
    let model = views::album_detail_body_model(
        AlbumDetailResponseInput::new(awaiting_seq, &sources.responses),
        &sources.filter.middle,
        sources.cursor.middle,
        IOS_FOCUSED,
        IOS_BODY_WIDTH,
        sources.selection.context == Some(SelectionContext::Middle),
        &sources.selection.selected,
    );
    push(ViewKind::AlbumDetail, &model, driver, state);
}

fn push_artist_detail(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let awaiting_seq = match &sources.history.mode {
        MiddleMode::ArtistDetail { awaiting_seq, .. } => *awaiting_seq,
        _ => None,
    };
    let model = views::artist_detail_body_model(
        ArtistDetailResponseInput::new(awaiting_seq, &sources.responses),
        ArtistDetailExtrasInput::new(&sources.artist_extras),
        sources.cursor.middle,
        IOS_FOCUSED,
        IOS_BODY_WIDTH,
        IOS_TIME_W,
    );
    push(ViewKind::ArtistDetail, &model, driver, state);
}

fn push_selection_bar(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let context = match sources.selection.context {
        Some(SelectionContext::Middle) => SelectionBarContext::Middle,
        Some(SelectionContext::Queue) => SelectionBarContext::Queue,
        None => SelectionBarContext::Middle, // model collapses to count=0 anyway
    };
    let model = views::selection_bar_model(
        context,
        &sources.selection.selected,
        SelectionBarSongsInput::new(&sources.queue, &sources.playlist_tracks),
        matches!(sources.history.mode, MiddleMode::PlaylistSongs),
    );
    push(ViewKind::SelectionBar, &model, driver, state);
}

fn push_pre_connect(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let server_picker_selected = match &sources.screen {
        Screen::ServerPicker { selected } => *selected,
        _ => 0,
    };
    let model = views::pre_connect_model(
        PreConnectInput::new(
            &sources.discovery,
            &sources.link,
            &sources.probes,
            &sources.credentials,
        ),
        sources.session.preferred_server.as_deref(),
        sources.session.lost_server.as_deref(),
        sources.session.auto_connect,
        server_picker_selected,
        sources.intent.target.as_deref(),
    );
    push(ViewKind::PreConnect, &model, driver, state);
}

fn middle_mode_view(mode: &MiddleMode) -> MiddleModeView {
    match mode {
        MiddleMode::PlaylistSongs => MiddleModeView::PlaylistSongs,
        MiddleMode::SearchResults {
            search_type, term, ..
        } => MiddleModeView::SearchResults {
            search_type: (*search_type).into(),
            term: std::sync::Arc::from(term.as_str()),
        },
        MiddleMode::AlbumDetail { awaiting_seq, .. } => MiddleModeView::AlbumDetail {
            awaiting_seq: *awaiting_seq,
        },
        MiddleMode::ArtistDetail { .. } => MiddleModeView::ArtistDetail,
    }
}

// ─── modal pushes ───────────────────────────────────────────────────
//
// Each modal pushes `Option<Model>`. `Some(model)` while its matching
// `Screen` variant is active; `None` otherwise. SwiftUI binds
// `.sheet(isPresented:)` (or `.alert`) to `payload != nil`.

fn push_action_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ActionModal(s) => Some(views::action_modal_model(ActionModalInput::new(
            s,
            &sources.keybindings,
        ))),
        _ => None,
    };
    push(ViewKind::ActionModal, &model, driver, state);
}

fn push_confirm_delete(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ConfirmDeletePlaylist { name, input, .. } => Some(
            views::confirm_delete_playlist_model(ConfirmDeletePlaylistInput { name, input }),
        ),
        _ => None,
    };
    push(ViewKind::ConfirmDeletePlaylist, &model, driver, state);
}

fn push_confirm_remove(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ConfirmRemoveFromPlaylist { song_title, .. } => {
            Some(views::confirm_remove_model(ConfirmRemoveInput {
                song_title,
            }))
        }
        _ => None,
    };
    push(ViewKind::ConfirmRemove, &model, driver, state);
}

fn push_error_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ErrorModal { message } => {
            Some(views::error_modal_model(ErrorModalInput { message }))
        }
        _ => None,
    };
    push(ViewKind::ErrorModal, &model, driver, state);
}

fn push_filter_input(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::FilterInput(s) => Some(views::filter_input_model(FilterStateInput::new(s))),
        _ => None,
    };
    push(ViewKind::FilterInput, &model, driver, state);
}

fn push_help_overlay(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::HelpOverlay { scroll } => Some(views::help_overlay_model(
            views::HelpOverlayInput::new(*scroll, &sources.keybindings),
        )),
        _ => None,
    };
    push(ViewKind::HelpOverlay, &model, driver, state);
}

fn push_input_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::CreatePlaylist { input, .. } => Some(views::input_modal_model(
            InputModalInput::new(InputModalKind::CreatePlaylist, input),
        )),
        Screen::RenamePlaylist { input, .. } => Some(views::input_modal_model(
            InputModalInput::new(InputModalKind::RenamePlaylist, input),
        )),
        _ => None,
    };
    push(ViewKind::InputModal, &model, driver, state);
}

fn push_pairing_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    use mkpclient_state_pairing::PairingPhase;
    let model = if sources.pairing.phase == PairingPhase::AwaitingConfirmation {
        Some(views::pairing_modal_model(PairingModalInput::new(
            &sources.pairing,
        )))
    } else {
        None
    };
    push(ViewKind::PairingModal, &model, driver, state);
}

fn push_playlist_action_modal(
    sources: &Sources,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    let model = match &sources.screen {
        Screen::PlaylistAction { selected, .. } => Some(views::playlist_action_modal_model(
            views::PlaylistActionModalInput::new(*selected, &sources.keybindings),
        )),
        _ => None,
    };
    push(ViewKind::PlaylistActionModal, &model, driver, state);
}

fn push_playlist_picker_hint(
    sources: &Sources,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    let model = match &sources.screen {
        Screen::PlaylistPicker { item, .. } => {
            Some(views::playlist_picker_hint_model(PlaylistPickerHintInput {
                item_label: &item.label,
            }))
        }
        _ => None,
    };
    push(ViewKind::PlaylistPickerHint, &model, driver, state);
}

fn push_search_input_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::SearchInput(s) => Some(views::search_input_model(SearchInputModalInput::new(s))),
        _ => None,
    };
    push(ViewKind::SearchInputModal, &model, driver, state);
}

fn push_selection_action_modal(
    sources: &Sources,
    driver: &UiBridgeDriver,
    state: &mut UiBridgeState,
) {
    let model = match &sources.screen {
        Screen::SelectionActionModal { selected } => Some(views::selection_action_modal_model(
            SelectionActionModalInput::new(
                sources.selection.context,
                &sources.history.mode,
                sources.selection.selected.len(),
                *selected,
                &sources.keybindings,
            ),
        )),
        _ => None,
    };
    push(ViewKind::SelectionActionModal, &model, driver, state);
}

fn push_server_lost_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ServerLostModal { server } => {
            Some(views::server_lost_modal_model(ServerLostModalInput {
                server,
            }))
        }
        _ => None,
    };
    push(ViewKind::ServerLostModal, &model, driver, state);
}

fn push_server_picker_modal(sources: &Sources, driver: &UiBridgeDriver, state: &mut UiBridgeState) {
    let model = match &sources.screen {
        Screen::ServerPicker { selected } => Some(views::server_picker_modal_model(
            ServerPickerModalInput::new(
                &sources.discovery,
                sources.session.backend_name.as_deref(),
                *selected,
            ),
        )),
        _ => None,
    };
    push(ViewKind::ServerPickerModal, &model, driver, state);
}
