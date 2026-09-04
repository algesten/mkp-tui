//! Per-view models + `#[derive(drv::Input)]` projections + memos.
//!
//! Every screen-region the TUI / iOS bridge draws is described here
//! by:
//!   1. A small `Clone + PartialEq` model struct (what the renderer
//!      needs to paint).
//!   2. One or more `#[derive(drv::Input)]` projections that borrow
//!      only the source fields the memo reads.
//!   3. A `#[drv::memo]` function that turns the projections into
//!      the model.
//!
//! Inputs and projections live here in the consumer crate (per
//! guideline 11). State crates stay drv-free and depend only on
//! `mkproto`.

mod action_modal;
mod album_detail;
mod artist_detail;
mod confirm_delete;
mod confirm_remove;
mod error_modal;
mod filter_input;
mod help_overlay;
mod input_modal;
mod keybindings_editor;
mod left;
mod middle_header;
mod now_playing;
mod pairing_modal;
mod playlist_action_modal;
mod playlist_picker_hint;
mod playlist_tracks;
mod pre_connect;
mod queue;
mod search_input_modal;
mod search_results;
mod selection_action_modal;
mod selection_bar;
mod server_lost_modal;
mod server_picker_modal;
mod util;

pub use action_modal::{action_modal_model, ActionModalInput, ActionModalModel, ActionModalRow};
pub use album_detail::{
    album_detail_body_model, AlbumDetailBodyModel, AlbumDetailResponseInput, AlbumDetailRow,
    AlbumDetailState, AlbumHeader,
};
pub use artist_detail::{
    artist_detail_body_model, ArtistDetailBodyModel, ArtistDetailExtrasInput, ArtistDetailLoaded,
    ArtistDetailResponseInput, ArtistDetailRow, ArtistDetailState, ArtistInfo, SimilarArtistEntry,
};
pub use confirm_delete::{
    confirm_delete_playlist_model, ConfirmDeletePlaylistInput, ConfirmDeletePlaylistModel,
};
pub use confirm_remove::{confirm_remove_model, ConfirmRemoveInput, ConfirmRemoveModel};
pub use error_modal::{error_modal_model, ErrorModalInput, ErrorModalModel};
pub use filter_input::{filter_input_model, FilterInputModel, FilterStateInput};
pub use help_overlay::{
    help_overlay_model, HelpEntry, HelpOverlayInput, HelpOverlayModel, HelpSection,
};
pub use input_modal::{input_modal_model, InputModalInput, InputModalKind, InputModalModel};
pub use keybindings_editor::{
    keybindings_editor_model, KeybindingsEditorAction, KeybindingsEditorInput,
    KeybindingsEditorModel,
};
pub use left::{
    left_column_model, LeftColumnModel, LeftRow, LeftUiInput, PendingPlaylistsInput,
    PickerOverride, PlaylistTracksFocusInput, PlaylistsInput, ServerLabelInput,
};
pub use middle_header::{
    album_detail_total_secs, column_widths, middle_header_model, ColumnWidths, MiddleHeaderModel,
    MiddleHeaderUiInput, MiddleMode as MiddleModeView, PlaylistTracksDurationInput,
    SearchCountsInput, SearchKind,
};
pub use now_playing::{
    now_playing_model, now_playing_song_model, ActivityInput, NowPlayingMeta, NowPlayingModel,
    NowPlayingRepeat, NowPlayingStatus, NowPlayingTitle, PeerActivityFrame, PeerIdInput,
    ServerNowPlayingInput, SongMetaInput, UiPreviewInput,
};
pub use pairing_modal::{pairing_modal_model, PairingModalInput, PairingModalModel};
pub use playlist_action_modal::{
    playlist_action_modal_model, PlaylistActionModalInput, PlaylistActionModalModel,
    PlaylistActionRow,
};
pub use playlist_picker_hint::{
    playlist_picker_hint_model, PlaylistPickerHintInput, PlaylistPickerHintModel,
};
pub use playlist_tracks::{
    playlist_tracks_body_model, PlaylistTrackRow, PlaylistTracksBodyModel, PlaylistTracksInput,
    PlaylistTracksPendingInput, PlaylistTracksState,
};
pub use pre_connect::{
    pre_connect_model, ConnectingKind, PreConnectInput, PreConnectModel, PreConnectRow,
};
pub use queue::{queue_column_model, QueueColumnModel, QueueInput, QueueRow, ServerPositionInput};
pub use search_input_modal::{
    search_input_model, SearchHistoryRow, SearchInputModalInput, SearchInputModel,
};
pub use search_results::{
    search_results_body_model, SearchAlbumRow, SearchArtistRow, SearchResultsBodyModel,
    SearchResultsInput, SearchResultsState, SearchSongRow,
};
pub use selection_action_modal::{
    selection_action_modal_model, SelectionActionModalInput, SelectionActionModalModel,
    SelectionActionRow,
};
pub use selection_bar::{
    selection_bar_model, SelectionBarContext, SelectionBarModel, SelectionBarSongsInput,
};
pub use server_lost_modal::{server_lost_modal_model, ServerLostModalInput, ServerLostModalModel};
pub use server_picker_modal::{
    server_picker_modal_model, ServerPickerModalInput, ServerPickerModalModel, ServerPickerRow,
};
