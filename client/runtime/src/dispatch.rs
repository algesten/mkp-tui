//! Dispatch: the one place UI-originated events mutate user-decision
//! sources. Each event is a
//! 5-15-line free function, one per source mutation, called via a
//! per-action `DispatchEvent` variant.
//!
//! Crossterm-free by construction. The TUI translator in
//! `client/tui/src/input.rs` is the only place key codes turn into
//! events; everything below is plain Rust state mutation.

use std::sync::Arc;
use std::time::Duration;

use log::debug;

use mkpclient_driver_credentials_core::CredCmd;
use mkpclient_driver_link_core::LinkCmd;
use mkpclient_driver_persist_core::{LoadKey, PersistCmd, SavedView};
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_pairing::PairingPhase;
use mkpclient_state_ui_cursor::ColumnFocus;
use mkpclient_state_ui_filter::FilterTarget;
use mkpclient_state_ui_history::{HistoryFrame, HistoryTransition, MiddleMode};
use mkpclient_state_ui_keybindings::{Action, KeyChord, KeyContext};
use mkpclient_state_ui_picker::PendingCreateAdd;
use mkpclient_state_ui_screen::{
    ActionItem, ActionKind, ActionModalState, ActionOrigin, FilterState, Screen, SearchState,
};
use mkpclient_state_ui_selection::SelectionContext;
use mkproto::{
    ClientMsg, MediaKind, NavigateTarget, QueuePosition, RepeatMode, SearchType, TaskId,
};

use crate::drivers::Drivers;
use crate::queries;
use crate::sources::Sources;

// ─── cross-source helpers (called by handlers + the TUI translator) ─

/// Push the current middle mode onto the history stack and switch
/// to `new_mode`. Resets the middle cursor to 0 and clears the
/// middle filter (the new mode starts with a fresh search/filter
/// scope). Forward stack is dropped — fresh navigation invalidates
/// redo. Mutates sources.history + sources.filter + sources.cursor.
pub fn history_drill(sources: &mut Sources, new_mode: MiddleMode) {
    let old = HistoryFrame {
        mode: std::mem::replace(&mut sources.history.mode, new_mode),
        filter: std::mem::replace(&mut sources.filter.middle, Arc::from("")),
        selected: std::mem::replace(&mut sources.cursor.middle, 0),
    };
    sources.history.back.push(old);
    sources.history.forward.clear();
    sources.history.last_transition = Some(HistoryTransition::Drill);
    sources.history.transition_seq = sources.history.transition_seq.wrapping_add(1);
    // Drilling into a detail view is one navigation action — the
    // focus follows the new content. Bundling the write here (per
    // §6 "dispatch handlers are 5–15-line free functions, one per
    // source mutation") keeps the rule in one place instead of
    // repeated at every call site.
    sources.cursor.focus = ColumnFocus::Middle;
}

/// Pop one entry off the back stack and push the current frame onto
/// the forward stack. Returns `true` if we moved.
pub fn history_back(sources: &mut Sources) -> bool {
    let Some(prev) = sources.history.back.pop() else {
        return false;
    };
    let cur = HistoryFrame {
        mode: std::mem::replace(&mut sources.history.mode, prev.mode),
        filter: std::mem::replace(&mut sources.filter.middle, Arc::from("")),
        selected: sources.cursor.middle,
    };
    sources.history.forward.push(cur);
    sources.filter.middle = prev.filter;
    sources.cursor.middle = prev.selected;
    sources.history.last_transition = Some(HistoryTransition::Back);
    sources.history.transition_seq = sources.history.transition_seq.wrapping_add(1);
    true
}

/// Re-apply a previously-popped history entry — moves an entry off
/// the forward stack back onto the live mode.
pub fn history_forward(sources: &mut Sources) -> bool {
    let Some(next) = sources.history.forward.pop() else {
        return false;
    };
    let cur = HistoryFrame {
        mode: std::mem::replace(&mut sources.history.mode, next.mode),
        filter: std::mem::replace(&mut sources.filter.middle, Arc::from("")),
        selected: sources.cursor.middle,
    };
    sources.history.back.push(cur);
    sources.filter.middle = next.filter;
    sources.cursor.middle = next.selected;
    sources.history.last_transition = Some(HistoryTransition::Forward);
    sources.history.transition_seq = sources.history.transition_seq.wrapping_add(1);
    true
}

/// Clear the error left by a previous failure so a fresh attempt is not
/// reported under the old one.
///
/// This used to also rewrite `Closed` back to `Idle`, because
/// `apply_link` refused to dial from `Closed` — which is precisely why
/// only user-driven paths could recover from a drop. `Closed` is now
/// just "nothing open" and dials like any other resting phase, so
/// nothing has to be acknowledged.
fn clear_last_error(link: &mut Link) {
    link.last_err = None;
}

// ─── DispatchEvent ──────────────────────────────────────────────────

/// Where a `JumpTo` should land.
#[derive(Debug, Clone, Copy)]
pub enum JumpTarget {
    Top,
    Bottom,
}

/// Direction of a one-page cursor move.
#[derive(Debug, Clone, Copy)]
pub enum PageStep {
    Up,
    Down,
}

/// Rows to advance the cursor by on a single PageUp/PageDown.
const PAGE_SIZE: usize = 10;

/// Backend-agnostic events emitted both by UI inputs and by
/// shared lifecycle code. Anything that names a server, a song, a
/// persistence operation, or a transport command lives here. The
/// iOS FFI accepts a subset of these; the desktop TUI emits all of
/// them via its key translator.
#[derive(Debug, Clone)]
pub enum SemanticEvent {
    // ── Connection ────────────────────────────────────────────────
    /// User picked a server from the discovery list. The runtime
    /// probes its current cert fingerprint, matches against stored
    /// credentials, and connects if a match is found.
    ConnectTo {
        server_name: String,
    },
    /// User asked to pair a newly-discovered server.
    BeginPair {
        server_name: String,
    },
    /// User confirmed the displayed pairing code.
    ConfirmPair,
    /// User rejected the displayed pairing code.
    RejectPair,
    /// User asked to disconnect.
    Disconnect,
    /// User asked to un-pair (forget) a server.
    Forget {
        fingerprint: String,
    },
    /// Ship a client-mode request. Caller can pre-allocate `task_id`
    /// out-of-band when they need to correlate with the response.
    SendRequest {
        msg: ClientMsg,
        task_id: Option<TaskId>,
    },
    /// Start streaming a playlist's tracks. Sends `GetPlaylist`; the
    /// server replies with `ListBegin` + `ListChunk` broadcasts that
    /// ingest folds into `state-playlist-tracks`.
    ViewPlaylist {
        id: String,
    },

    // ── Transport ─────────────────────────────────────────────────
    /// `]` / `>` — enqueue the next-track command.
    SkipNext,
    /// `[` / `<` — enqueue the previous-track command.
    SkipPrevious,
    /// `}` — seek forward (10 s, or 1 s with Alt).
    SeekForward {
        fine: bool,
    },
    /// `{` — seek back.
    SeekBackward {
        fine: bool,
    },
    /// `.` — cycle Off → All → One → Off.
    CycleRepeatMode,
    /// SwiftUI play/pause button or `MPRemoteCommand` Play/Pause/
    /// Toggle from the lock screen / AirPods / headphones. The
    /// dispatch handler reads the current state and sends the
    /// matching `ClientMsg::SetPaused`.
    TogglePlayPause,
    /// Absolute seek (the lock-screen scrubber). `position` is in
    /// seconds from the start of the track.
    SeekTo {
        position: f64,
    },

    // ── Lifecycle modal opens / cursor snaps ──────────────────────
    /// Open ServerLostModal with the given server name (auto-restore
    /// triggers this on disconnect).
    OpenServerLostModal {
        server: String,
    },
    /// Open ErrorModal with the given message (auto-restore triggers
    /// this when the responses queue surfaces a `ServerMsg::Error`).
    OpenErrorModal {
        message: String,
    },
    /// `auto_restore` consumed (took) a response by seq — internally
    /// just removes the response so the modal isn't re-opened.
    TakeResponse {
        seq: u64,
    },
    /// Move the cursor to the row whose song id is `id`, in the
    /// currently-active middle mode. No-op if the row isn't there.
    SnapMiddleCursorToSongId {
        id: String,
    },

    // ── Session breadcrumbs (called by lifecycle / TUI auto_restore)
    SetBackendName(Option<String>),
    SetLostServer(Option<String>),
    SetAutoConnect(bool),
    SetAutoRestoredView(bool),
    SetPreferredServer(Option<String>),
    SetPendingCursorSongId(Option<String>),

    // ── Saved-view restore hooks (called from auto_restore) ───────
    /// Restore a "Playlist" saved view: ViewPlaylist + drill into
    /// PlaylistSongs at the given selected index. The translator
    /// passes `selected_id` so the row can be snapped once tracks
    /// stream in.
    RestoreSavedPlaylist {
        playlist_id: String,
        selected: usize,
        selected_id: Option<String>,
    },
    /// Restore an "AlbumDetail" saved view.
    RestoreSavedAlbum {
        album_id: String,
        album_name: String,
        selected: usize,
        selected_id: Option<String>,
    },
    /// Restore an "ArtistDetail" saved view.
    RestoreSavedArtist {
        artist_id: String,
        artist_name: String,
        selected: usize,
    },
    /// Restore a "Search" saved view — re-runs the search query.
    RestoreSavedSearch {
        query: String,
        search_type: SearchType,
        selected: usize,
        selected_id: Option<String>,
    },
    /// First-run on a backend: load the first playlist and drill
    /// into PlaylistSongs.
    OpenFirstPlaylist {
        id: String,
    },
    /// auto_restore detected a successful CreatePlaylist landing —
    /// the TUI's pending_create_add picker breadcrumb fires the
    /// deferred AddToPlaylist using `playlist_id`. The handler
    /// also clears the breadcrumb and bumps `last_add_playlist`.
    FireDeferredAddToPlaylist {
        playlist_id: String,
    },

    // ── Toasts (used by the TUI translator after a side effect) ──
    Toast {
        text: String,
        ttl: Duration,
    },

    // ── Clipboard ─────────────────────────────────────────────────
    /// Enqueue a clipboard write. The driver consumes
    /// `clipboard.pending` and ships a Cmd; on success the toast
    /// lifecycle fires `success_toast` for 3 s.
    ClipboardCopy {
        text: String,
        success_toast: String,
    },

    // ── Persist driver dispatches ────────────────────────────────
    /// Persist the just-connected server's name as the next-startup
    /// preferred. Caller passes the name (already de-aliased).
    PersistSaveLastServer {
        name: String,
    },
    /// Persist the playlist id the user just added a song to as the
    /// next-open default for this backend.
    PersistSaveLastAddPlaylist {
        backend: String,
        playlist_id: String,
    },
    /// Append a `(query, search_type, ts)` tuple to the per-backend
    /// search-history file (driver-side dedups + truncates).
    PersistPushSearchHistory {
        backend: String,
        query: String,
        search_type: String,
    },
    /// Snapshot the current middle-pane state to the per-backend
    /// `last_view` file. Reads sources.history + cursor + the matching
    /// detail source to build the `SavedView`.
    PersistSaveCurrentView {
        backend: String,
    },
    /// Erase the per-backend `last_view` file.
    PersistClearView {
        backend: String,
    },
    /// Issue a `LoadView` for the connected backend if one isn't
    /// already in flight. Result lands via `ingest::ingest_persist`.
    PersistRequestLoadView,
    /// Issue a `LoadLastAddPlaylist` for the just-connected backend
    /// if one isn't already in flight.
    PersistRequestLoadLastAddPlaylist {
        backend: String,
    },
}

/// TUI-only events: keyboard cursor moves, modal text input, focus
/// rotation. These never cross the iOS FFI — iOS expresses the same
/// user intents through `SemanticEvent` (e.g. a tap-to-play SwiftUI
/// gesture sends `SemanticEvent::SendRequest { ClientMsg::Play, .. }`,
/// not a `MiddleActivate`).
#[derive(Debug, Clone)]
pub enum TuiCursorEvent {
    // ── Modal close (used by every modal's Esc / dismiss path) ────
    /// Reset screen back to NowPlaying. Used by Esc in most modals.
    CloseModal,

    // ── ActionModal (right-click style menu on a row) ─────────────
    ActionModalCursorUp,
    ActionModalCursorDown,
    /// Apply an `ActionModal` menu choice (`n` / `e` / `a` / `q` /
    /// `w` / `c` / `d`). Routes to the right side-effect: dispatch
    /// a `Play`, drill into a detail view, open the `PlaylistPicker`,
    /// open `ConfirmRemoveFromPlaylist`, copy a URL.
    ApplyActionChoice(char),

    // ── HelpOverlay ───────────────────────────────────────────────
    HelpScroll(i32),
    HelpScrollHome,
    OpenKeybindingsEditor,
    CloseKeybindingsEditor,
    KeybindingsEditorType(char),
    KeybindingsEditorSelect(Action),
    KeybindingsEditorBind(KeyChord),
    KeybindingsEditorSave,

    // ── FilterInput (live-applied) ────────────────────────────────
    FilterInputType(char),
    FilterInputBackspace,
    /// Esc: clear the filter and close the modal.
    FilterInputCancel,
    /// Enter: keep the filter and close the modal.
    FilterInputSubmit,

    // ── SearchInput ───────────────────────────────────────────────
    SearchHistoryUp,
    SearchHistoryDown,
    SearchInputType(char),
    SearchInputBackspace,
    SearchCycleType {
        forward: bool,
    },
    /// `e` on a history row pulls the row into the input field for
    /// editing (legacy: edit-then-search).
    SearchEditFromHistory,
    /// Enter — runs the search query (from input or from selected
    /// history row). The handler reads sources.screen to know which.
    SearchSubmit,

    // ── CreatePlaylist ────────────────────────────────────────────
    CreatePlaylistType(char),
    CreatePlaylistBackspace,
    CreatePlaylistSubmit,

    // ── RenamePlaylist ────────────────────────────────────────────
    RenamePlaylistType(char),
    RenamePlaylistBackspace,
    RenamePlaylistSubmit,

    // ── ConfirmDeletePlaylist (type-to-confirm) ──────────────────
    ConfirmDeleteType(char),
    ConfirmDeleteBackspace,
    ConfirmDeleteSubmit,

    // ── PlaylistAction (rename / delete picker) ──────────────────
    PlaylistActionCursorUp,
    PlaylistActionCursorDown,
    PlaylistActionSubmit,
    /// 'r' shortcut → jump straight to RenamePlaylist.
    PlaylistActionRename,
    /// 'd' shortcut → jump straight to ConfirmDeletePlaylist.
    PlaylistActionDelete,

    // ── PlaylistPicker (add-to-playlist) ──────────────────────────
    PlaylistPickerCursorUp,
    PlaylistPickerCursorDown,
    PlaylistPickerSubmit,

    // ── ConfirmRemoveFromPlaylist ────────────────────────────────
    ConfirmRemoveSubmit,

    // ── SelectionActionModal (bulk action menu) ───────────────────
    SelectionActionCursorUp,
    SelectionActionCursorDown,
    /// Apply a bulk action ('n' / 'e' / 'a' / 'd').
    SelectionActionApply(char),

    // ── ServerLostModal ───────────────────────────────────────────
    /// Enter on the lost-server modal — give up: clear lost+preferred,
    /// disconnect, return to NowPlaying.
    ServerLostGiveUp,

    // ── Selection mode (Cell B) ───────────────────────────────────
    SelectionAdd,
    SelectionRemove,
    SelectionToggleAnchor,
    SelectionMoveUp,
    SelectionMoveDown,
    /// Enter — PlaySongs Reset of every selected row.
    SelectionPlayReset,
    /// Tab — open the selection action modal.
    SelectionOpenModal,
    /// Esc — clear selection.
    SelectionClear,

    // ── Server picker (pre-connect) ───────────────────────────────
    PickerCursorUp,
    PickerCursorDown,
    /// Enter — connect to the highlighted server (or pair first if
    /// the server has no stored credentials; the auto-fallback in
    /// `apply_link` swaps `intent.target` → `intent.pair_target`
    /// once the probe lands).
    PickerConnect,

    // ── Server picker modal (opened from the connected server row) ─
    ServerPickerModalCursorUp,
    ServerPickerModalCursorDown,
    /// Enter on a row — same server closes; different server swaps
    /// intent.target so the link driver reconnects.
    ServerPickerModalSelect,

    // ── Left pane ─────────────────────────────────────────────────
    LeftCursorUp,
    LeftCursorDown,
    /// Enter — depending on the row: server-row → request disconnect,
    /// playlist-row → ViewPlaylist + history_drill, "+ New" → open
    /// CreatePlaylist.
    LeftActivate,
    /// Tab on a playlist row → open PlaylistAction modal.
    LeftOpenAction,
    /// Space toggles play/pause (legacy parity — Left pane has no
    /// inline filter; the FilterInput modal owns playlist filtering).
    LeftTogglePlayPause,

    // ── Middle pane ───────────────────────────────────────────────
    MiddleCursorUp,
    MiddleCursorDown,
    /// Enter — activate the focused row (play / drill).
    MiddleActivate,
    /// Tab — open the action modal for the focused row.
    MiddleOpenAction,
    /// 'x' shortcut — same as MiddleOpenAction.
    MiddleOpenActionX,
    /// Space — toggle play/pause.
    MiddleTogglePlayPause,

    // ── Queue pane ────────────────────────────────────────────────
    QueueCursorUp,
    QueueCursorDown,
    /// Enter — skip to the focused server-assigned queue entry.
    QueueActivate,
    /// Tab — open the action modal for the focused row.
    QueueOpenAction,
    /// 'x' shortcut — same as QueueOpenAction.
    QueueOpenActionX,
    /// Space — toggle play/pause.
    QueueTogglePlayPause,

    // ── NowPlaying globals ────────────────────────────────────────
    /// Left/Right: rotate pane focus.
    CycleFocusForward,
    CycleFocusBackward,
    /// Shift-Left/Right: walk middle-pane history.
    HistoryBack,
    HistoryForward,

    /// 'S' — open the search input modal. The modal opens with empty
    /// history; the persist driver's `LoadSearchHistory` runs in
    /// parallel and the ingest phase folds the result into the open
    /// `Screen::SearchInput.history` once it lands.
    OpenSearchInput,
    /// '?' — open the help overlay.
    OpenHelpOverlay,
    /// 'F' — open filter input on the focused pane (if applicable).
    OpenFilterInputForFocused,
    /// 'M' — toggle multi-selection on the focused pane.
    ToggleSelectionForFocused,
    /// 'g' / Home — jump cursor to top of focused pane.
    JumpTopFocused,
    /// 'G' / End — jump cursor to bottom of focused pane.
    JumpBottomFocused,
    /// PageUp — move cursor up one page in the focused pane.
    PageUpFocused,
    /// PageDown — move cursor down one page in the focused pane.
    PageDownFocused,
    /// Alt+Enter — shuffle-activate the focused row.
    ShuffleActivateFocused,
    /// Tab on a row → open the action menu for the focused pane.
    /// If no menu is appropriate (cursor on a non-action row),
    /// behave as `CycleFocusForward`.
    OpenActionMenuOrCycle,
    /// Esc on NowPlaying — clears the focused pane's filter (if any),
    /// otherwise no-op.
    ClearFocusedFilter,
}

/// Outer event the dispatcher accepts. Use `From<SemanticEvent>` /
/// `From<TuiCursorEvent>` (also wired through `Runtime::dispatch`'s
/// `impl Into<DispatchEvent>` signature) so call sites can write
/// `rt.dispatch(SemanticEvent::ConnectTo { ... })` directly.
#[derive(Debug, Clone)]
pub enum DispatchEvent {
    Semantic(SemanticEvent),
    Cursor(TuiCursorEvent),
}

impl From<SemanticEvent> for DispatchEvent {
    fn from(ev: SemanticEvent) -> Self {
        DispatchEvent::Semantic(ev)
    }
}

impl From<TuiCursorEvent> for DispatchEvent {
    fn from(ev: TuiCursorEvent) -> Self {
        DispatchEvent::Cursor(ev)
    }
}

// ─── handlers (one per variant, 5–15 lines each) ────────────────────

pub fn dispatch<E: Into<DispatchEvent>>(ev: E, sources: &mut Sources, drivers: &Drivers) {
    match ev.into() {
        DispatchEvent::Semantic(s) => dispatch_semantic(s, sources, drivers),
        DispatchEvent::Cursor(c) => dispatch_cursor(c, sources, drivers),
    }
}

fn dispatch_semantic(ev: SemanticEvent, sources: &mut Sources, drivers: &Drivers) {
    use SemanticEvent::*;
    match ev {
        // ── Connection ──────────────────────────────────────────────
        ConnectTo { server_name } => connect_to(sources, server_name),
        BeginPair { server_name } => begin_pair(sources, server_name),
        ConfirmPair => confirm_pair(sources, drivers),
        RejectPair => reject_pair(sources, drivers),
        Disconnect => disconnect(sources, drivers),
        Forget { fingerprint } => forget(sources, drivers, fingerprint),
        SendRequest { msg, task_id } => {
            sources.requests.push(msg, task_id);
        }
        ViewPlaylist { id } => view_playlist(sources, id),

        // ── Transport ───────────────────────────────────────────────
        SkipNext => skip_next(sources),
        SkipPrevious => skip_previous(sources),
        SeekForward { fine } => seek_relative(sources, if fine { 1.0 } else { 10.0 }),
        SeekBackward { fine } => seek_relative(sources, if fine { -1.0 } else { -10.0 }),
        CycleRepeatMode => cycle_repeat_mode(sources),
        TogglePlayPause => toggle_play_pause(sources),
        SeekTo { position } => {
            sources.requests.push(ClientMsg::Seek { position }, None);
        }

        // ── Lifecycle modals + cursor snap ──────────────────────────
        OpenServerLostModal { server } => {
            sources.screen = Screen::ServerLostModal {
                server: Arc::from(server),
            };
        }
        OpenErrorModal { message } => {
            sources.screen = Screen::ErrorModal {
                message: Arc::from(message),
            };
        }
        TakeResponse { seq } => {
            sources.responses.take(seq);
        }
        SnapMiddleCursorToSongId { id } => snap_middle_cursor_to_song_id(sources, id),

        // ── Session breadcrumbs ─────────────────────────────────────
        SetBackendName(v) => sources.session.backend_name = v.map(Arc::from),
        SetLostServer(v) => sources.session.lost_server = v.map(Arc::from),
        SetAutoConnect(v) => sources.session.auto_connect = v,
        SetAutoRestoredView(v) => sources.session.auto_restored_view = v,
        SetPreferredServer(v) => sources.session.preferred_server = v.map(Arc::from),
        SetPendingCursorSongId(v) => sources.session.pending_cursor_song_id = v.map(Arc::from),

        // ── Saved-view restore ──────────────────────────────────────
        RestoreSavedPlaylist {
            playlist_id,
            selected,
            selected_id,
        } => {
            restore_saved_playlist(sources, playlist_id, selected, selected_id);
        }
        RestoreSavedAlbum {
            album_id,
            album_name,
            selected,
            selected_id,
        } => {
            restore_saved_album(sources, album_id, album_name, selected, selected_id);
        }
        RestoreSavedArtist {
            artist_id,
            artist_name,
            selected,
        } => {
            restore_saved_artist(sources, artist_id, artist_name, selected);
        }
        RestoreSavedSearch {
            query,
            search_type,
            selected,
            selected_id,
        } => {
            restore_saved_search(sources, query, search_type, selected, selected_id);
        }
        OpenFirstPlaylist { id } => open_first_playlist(sources, id),
        FireDeferredAddToPlaylist { playlist_id } => {
            fire_deferred_add_to_playlist(sources, playlist_id)
        }

        Toast { text, ttl } => sources.toast.show(text, sources.clock.now + ttl),

        ClipboardCopy {
            text,
            success_toast,
        } => {
            sources.clipboard.enqueue(text, success_toast);
        }

        // ── Persist ─────────────────────────────────────────────────
        PersistSaveLastServer { name } => {
            drivers
                .persist
                .execute([&PersistCmd::SaveLastServer { name }]);
        }
        PersistSaveLastAddPlaylist {
            backend,
            playlist_id,
        } => {
            drivers.persist.execute([&PersistCmd::SaveLastAddPlaylist {
                backend,
                playlist_id,
            }]);
        }
        PersistPushSearchHistory {
            backend,
            query,
            search_type,
        } => {
            drivers.persist.execute([&PersistCmd::PushSearchHistory {
                backend,
                query,
                search_type,
            }]);
        }
        PersistSaveCurrentView { backend } => save_current_view(sources, drivers, backend),
        PersistClearView { backend } => {
            drivers
                .persist
                .execute([&PersistCmd::ClearView { backend }]);
        }
        PersistRequestLoadView => {
            if let Some(backend) = sources.session.backend_name.clone() {
                request_load_view(sources, drivers, backend.to_string());
            }
        }
        PersistRequestLoadLastAddPlaylist { backend } => {
            request_load_last_add_playlist(sources, drivers, backend);
        }
    }
}

fn dispatch_cursor(ev: TuiCursorEvent, sources: &mut Sources, drivers: &Drivers) {
    use TuiCursorEvent::*;
    match ev {
        CloseModal => sources.screen = Screen::NowPlaying,

        ActionModalCursorUp => action_modal_cursor_up(sources),
        ActionModalCursorDown => action_modal_cursor_down(sources),
        ApplyActionChoice(c) => apply_action_choice(sources, c),

        HelpScroll(delta) => help_scroll(sources, delta),
        HelpScrollHome => help_scroll_home(sources),
        OpenKeybindingsEditor => open_keybindings_editor(sources),
        CloseKeybindingsEditor => close_keybindings_editor(sources),
        KeybindingsEditorType(c) => keybindings_editor_type(sources, c),
        KeybindingsEditorSelect(action) => keybindings_editor_select(sources, action),
        KeybindingsEditorBind(key) => keybindings_editor_bind(sources, key),
        KeybindingsEditorSave => keybindings_editor_save(sources, drivers),

        FilterInputType(c) => filter_input_type(sources, c),
        FilterInputBackspace => filter_input_backspace(sources),
        FilterInputCancel => filter_input_cancel(sources),
        FilterInputSubmit => filter_input_submit(sources),

        SearchHistoryUp => search_history_up(sources),
        SearchHistoryDown => search_history_down(sources),
        SearchInputType(c) => search_input_type(sources, c),
        SearchInputBackspace => search_input_backspace(sources),
        SearchCycleType { forward } => search_cycle_type(sources, forward),
        SearchEditFromHistory => search_edit_from_history(sources),
        SearchSubmit => search_submit(sources),

        CreatePlaylistType(c) => create_playlist_type(sources, c),
        CreatePlaylistBackspace => create_playlist_backspace(sources),
        CreatePlaylistSubmit => create_playlist_submit(sources),

        RenamePlaylistType(c) => rename_playlist_type(sources, c),
        RenamePlaylistBackspace => rename_playlist_backspace(sources),
        RenamePlaylistSubmit => rename_playlist_submit(sources),

        ConfirmDeleteType(c) => confirm_delete_type(sources, c),
        ConfirmDeleteBackspace => confirm_delete_backspace(sources),
        ConfirmDeleteSubmit => confirm_delete_submit(sources),

        PlaylistActionCursorUp => playlist_action_cursor_up(sources),
        PlaylistActionCursorDown => playlist_action_cursor_down(sources),
        PlaylistActionSubmit => playlist_action_submit(sources),
        PlaylistActionRename => playlist_action_rename(sources),
        PlaylistActionDelete => playlist_action_delete(sources),

        PlaylistPickerCursorUp => playlist_picker_cursor_up(sources),
        PlaylistPickerCursorDown => playlist_picker_cursor_down(sources),
        PlaylistPickerSubmit => playlist_picker_submit(sources),

        ConfirmRemoveSubmit => confirm_remove_submit(sources),

        SelectionActionCursorUp => selection_action_cursor_up(sources),
        SelectionActionCursorDown => selection_action_cursor_down(sources),
        SelectionActionApply(c) => selection_action_apply(sources, c),

        ServerLostGiveUp => server_lost_give_up(sources, drivers),

        SelectionAdd => selection_add(sources),
        SelectionRemove => selection_remove(sources),
        SelectionToggleAnchor => selection_toggle_anchor(sources),
        SelectionMoveUp => selection_move_up(sources),
        SelectionMoveDown => selection_move_down(sources),
        SelectionPlayReset => selection_play_reset(sources),
        SelectionOpenModal => sources.screen = Screen::SelectionActionModal { selected: 0 },
        SelectionClear => sources.selection.clear(),

        PickerCursorUp => picker_cursor_up(sources),
        PickerCursorDown => picker_cursor_down(sources),
        PickerConnect => picker_connect(sources),

        ServerPickerModalCursorUp => server_picker_modal_cursor_up(sources),
        ServerPickerModalCursorDown => server_picker_modal_cursor_down(sources),
        ServerPickerModalSelect => server_picker_modal_select(sources, drivers),

        LeftCursorUp => left_cursor_up(sources),
        LeftCursorDown => left_cursor_down(sources),
        LeftActivate => left_activate(sources),
        LeftOpenAction => left_open_action(sources),
        LeftTogglePlayPause => toggle_play_pause(sources),

        MiddleCursorUp => middle_cursor_up(sources),
        MiddleCursorDown => middle_cursor_down(sources),
        MiddleActivate => middle_activate(sources),
        MiddleOpenAction | MiddleOpenActionX => middle_open_action(sources),
        MiddleTogglePlayPause => toggle_play_pause(sources),

        QueueCursorUp => queue_cursor_up(sources),
        QueueCursorDown => queue_cursor_down(sources),
        QueueActivate => queue_activate(sources),
        QueueOpenAction | QueueOpenActionX => queue_open_action(sources),
        QueueTogglePlayPause => toggle_play_pause(sources),

        CycleFocusForward => {
            sources.cursor.cycle_focus_forward();
            snap_queue_cursor_to_current(sources);
        }
        CycleFocusBackward => {
            sources.cursor.cycle_focus_backward();
            snap_queue_cursor_to_current(sources);
        }
        HistoryBack => {
            history_back(sources);
        }
        HistoryForward => {
            history_forward(sources);
        }

        OpenSearchInput => open_search_input(sources, drivers),
        OpenHelpOverlay => sources.screen = Screen::HelpOverlay { scroll: 0 },
        OpenFilterInputForFocused => open_filter_input_for_focused(sources),
        ToggleSelectionForFocused => toggle_selection_for_focused(sources),
        JumpTopFocused => jump_focused(sources, JumpTarget::Top),
        JumpBottomFocused => jump_focused(sources, JumpTarget::Bottom),
        PageUpFocused => page_focused(sources, PageStep::Up),
        PageDownFocused => page_focused(sources, PageStep::Down),
        ShuffleActivateFocused => shuffle_activate_focused(sources),
        OpenActionMenuOrCycle => open_action_menu_or_cycle(sources),
        ClearFocusedFilter => clear_focused_filter(sources),
    }
}

// ─── Connection ─────────────────────────────────────────────────────

fn connect_to(sources: &mut Sources, server_name: String) {
    // A pairing session belongs to the connection that carried it. If
    // one is still recorded from a handshake that dropped, it describes
    // nothing — and leaving it would make the runtime treat this fresh,
    // user-asked-for attempt as the dead one still being in flight.
    sources.pairing = Default::default();
    sources.intent.target = Some(Arc::from(server_name));
    sources.intent.pair_target = None;
    clear_last_error(&mut sources.link);
    // The user asked for this one now; a backoff accumulated by
    // earlier automatic retries must not delay it.
    sources.link.clear_retry();
    sources.probes.retry_unresolved();
}

fn begin_pair(sources: &mut Sources, server_name: String) {
    sources.pairing = Default::default();
    sources.intent.pair_target = Some(Arc::from(server_name));
    clear_last_error(&mut sources.link);
    sources.link.clear_retry();
    sources.probes.retry_unresolved();
}

fn confirm_pair(sources: &mut Sources, drivers: &Drivers) {
    if sources.pairing.phase == PairingPhase::AwaitingConfirmation {
        sources.pairing.phase = PairingPhase::Confirming;
        drivers.link.execute([&LinkCmd::ConfirmPair]);
    } else {
        debug!(
            "dispatch: ConfirmPair ignored (phase = {:?})",
            sources.pairing.phase
        );
    }
}

fn reject_pair(sources: &mut Sources, drivers: &Drivers) {
    drivers.link.execute([&LinkCmd::RejectPair]);
    sources.pairing = Default::default();
    sources.intent.pair_target = None;
}

fn disconnect(sources: &mut Sources, drivers: &Drivers) {
    sources.intent.target = None;
    if matches!(
        sources.link.phase,
        LinkPhase::Connected | LinkPhase::Connecting
    ) {
        drivers.link.execute([&LinkCmd::Disconnect]);
    }
}

fn forget(sources: &mut Sources, drivers: &Drivers, fingerprint: String) {
    if sources.intent.target.as_deref() == Some(fingerprint.as_str()) {
        sources.intent.target = None;
        drivers.link.execute([&LinkCmd::Disconnect]);
    }
    drivers
        .credentials
        .execute([&CredCmd::Delete { fingerprint }]);
}

fn view_playlist(sources: &mut Sources, id: String) {
    sources.playlist_tracks.clear();
    sources.playlist_tracks.playlist_id = Some(Arc::from(id.as_str()));
    let task_id = sources.requests.alloc_task_id();
    sources.playlist_tracks.pending_task = Some(task_id);
    sources
        .requests
        .push(ClientMsg::GetPlaylist { id, focus: 0 }, Some(task_id));
}

// ─── ActionModal ────────────────────────────────────────────────────

fn action_modal_cursor_up(sources: &mut Sources) {
    let Screen::ActionModal(state) = &mut sources.screen else {
        return;
    };
    let n = state.len();
    if n == 0 {
        return;
    }
    state.selected = if state.selected == 0 {
        n - 1
    } else {
        state.selected - 1
    };
}

fn action_modal_cursor_down(sources: &mut Sources) {
    let Screen::ActionModal(state) = &mut sources.screen else {
        return;
    };
    let n = state.len();
    if n == 0 {
        return;
    }
    state.selected = (state.selected + 1) % n;
}

fn apply_action_choice(sources: &mut Sources, choice: char) {
    // Resolve "letter pressed" into the canonical menu key. Enter is
    // translated by the TUI to the highlighted row's key.
    let item = match &sources.screen {
        Screen::ActionModal(s) if s.menu().iter().any(|(k, _)| *k == choice) => s.item.clone(),
        _ => return,
    };
    let next_screen = dispatch_action_choice_inner(sources, choice, &item);
    sources.screen = if let Some(mut s) = next_screen {
        preselect_picker(&mut s, sources);
        s
    } else {
        Screen::NowPlaying
    };
}

/// Apply an ActionModal menu choice. Returns the next screen to
/// switch to (or None to stay on NowPlaying).
fn dispatch_action_choice_inner(
    sources: &mut Sources,
    choice: char,
    item: &ActionItem,
) -> Option<Screen> {
    match choice {
        'n' => {
            if let Some(media_kind) = item.kind.to_media() {
                sources.requests.push(
                    ClientMsg::Play {
                        id: item.id.to_string(),
                        kind: media_kind,
                        position: QueuePosition::Next,
                        start_index: None,
                    },
                    None,
                );
            }
            None
        }
        'e' => {
            if let Some(media_kind) = item.kind.to_media() {
                sources.requests.push(
                    ClientMsg::Play {
                        id: item.id.to_string(),
                        kind: media_kind,
                        position: QueuePosition::Last,
                        start_index: None,
                    },
                    None,
                );
            }
            None
        }
        'a' => Some(Screen::PlaylistPicker {
            item: item.clone(),
            selected: 0,
        }),
        'q' => {
            // Go to Artist. Prefer a known artist_id (album/artist
            // search results carry it); fall back to Navigate which
            // makes the server resolve via the song id.
            let title = item.artist_label.as_deref().unwrap_or("Artist").to_string();
            let (artist_id, seq) = match item.artist_id.clone() {
                Some(id) => {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetArtistDetail { id: id.to_string() }, None);
                    (id.to_string(), seq)
                }
                None if item.kind == ActionKind::Song => {
                    let seq = sources.requests.push(
                        ClientMsg::Navigate {
                            target: NavigateTarget::Artist,
                            song_id: item.id.to_string(),
                        },
                        None,
                    );
                    (String::new(), seq)
                }
                None => return None,
            };
            history_drill(
                sources,
                MiddleMode::ArtistDetail {
                    artist_id,
                    artist_name: title,
                    awaiting_seq: Some(seq),
                },
            );
            None
        }
        'w' => {
            let title = item.album_title.as_deref().unwrap_or("Album").to_string();
            let (album_id, seq) = match item.album_id.clone() {
                Some(id) => {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetAlbumDetail { id: id.to_string() }, None);
                    (id.to_string(), seq)
                }
                None if item.kind == ActionKind::Song => {
                    let seq = sources.requests.push(
                        ClientMsg::Navigate {
                            target: NavigateTarget::Album,
                            song_id: item.id.to_string(),
                        },
                        None,
                    );
                    (String::new(), seq)
                }
                None => return None,
            };
            history_drill(
                sources,
                MiddleMode::AlbumDetail {
                    album_id,
                    album_title: title,
                    awaiting_seq: Some(seq),
                },
            );
            None
        }
        'c' => {
            // Clipboard write happens TUI-side (it reads `item.url`
            // from the active modal). The TUI emits a Toast event
            // after a successful copy.
            None
        }
        'd' => match item.origin {
            ActionOrigin::PlaylistSongs => {
                item.playlist_id
                    .clone()
                    .zip(item.view_index)
                    .map(|(pid, idx)| Screen::ConfirmRemoveFromPlaylist {
                        playlist_id: pid,
                        song_index: idx,
                        song_title: item.label.clone(),
                    })
            }
            ActionOrigin::Queue => {
                if let Some(&index) =
                    queries::queue_filtered_indices(sources).get(sources.cursor.queue)
                {
                    remove_queue_entry(sources, index);
                }
                None
            }
            ActionOrigin::OtherMiddle => None,
        },
        _ => None,
    }
}

/// If we're about to open a `PlaylistPicker`, pre-select the
/// last-add playlist remembered for this backend.
fn preselect_picker(screen: &mut Screen, sources: &Sources) {
    if let Screen::PlaylistPicker { selected, .. } = screen {
        if let Some(last) = sources.picker.last_add_playlist.as_deref() {
            if let Some(pos) = sources.playlists.items.iter().position(|p| p.id == last) {
                *selected = pos;
            }
        }
    }
}

// ─── HelpOverlay ────────────────────────────────────────────────────

fn help_scroll(sources: &mut Sources, delta: i32) {
    let Screen::HelpOverlay { scroll } = &mut sources.screen else {
        return;
    };
    if delta > 0 {
        *scroll = scroll.saturating_add(delta as u16);
    } else {
        *scroll = scroll.saturating_sub(delta.unsigned_abs() as u16);
    }
}

fn help_scroll_home(sources: &mut Sources) {
    let Screen::HelpOverlay { scroll } = &mut sources.screen else {
        return;
    };
    *scroll = 0;
}

fn open_keybindings_editor(sources: &mut Sources) {
    let Screen::HelpOverlay { scroll } = sources.screen else {
        return;
    };
    if sources.persist.is_loading(&LoadKey::Keybindings) {
        sources.toast.show(
            "Keybindings are still loading",
            sources.clock.now + Duration::from_secs(3),
        );
        return;
    }
    sources.screen = Screen::KeybindingsEditor(
        mkpclient_state_ui_screen::KeybindingsEditorState::new(sources.keybindings.clone(), scroll),
    );
}

fn close_keybindings_editor(sources: &mut Sources) {
    let Screen::KeybindingsEditor(state) = &sources.screen else {
        return;
    };
    sources.screen = Screen::HelpOverlay {
        scroll: state.help_scroll,
    };
}

fn keybindings_editor_type(sources: &mut Sources, c: char) {
    let Screen::KeybindingsEditor(state) = &mut sources.screen else {
        return;
    };
    let ctx = KeyContext::ALL[state.selected_context];
    match c {
        'k' if state.focus_right => {
            state.selected_binding = state.selected_binding.saturating_sub(1)
        }
        'k' => {
            state.selected_context = state.selected_context.saturating_sub(1);
            state.selected_binding = 0;
        }
        'j' if state.focus_right => {
            let max = state.draft.sorted_actions(ctx).len().saturating_sub(1);
            state.selected_binding = (state.selected_binding + 1).min(max);
        }
        'j' => {
            state.selected_context =
                (state.selected_context + 1).min(KeyContext::ALL.len().saturating_sub(1));
            state.selected_binding = 0;
        }
        'l' => state.focus_right = true,
        'h' => state.focus_right = false,
        'a' if state.focus_right => state.adding = true,
        'd' if state.focus_right => {
            if let Some(action) = state
                .draft
                .sorted_actions(ctx)
                .get(state.selected_binding)
                .copied()
            {
                state.draft.reset(ctx, action);
            }
        }
        'g' if state.focus_right => state.selected_binding = 0,
        'G' if state.focus_right => {
            state.selected_binding = state.draft.sorted_actions(ctx).len().saturating_sub(1)
        }
        _ => {}
    }
}

fn keybindings_editor_select(sources: &mut Sources, action: Action) {
    let Screen::KeybindingsEditor(state) = &mut sources.screen else {
        return;
    };
    let ctx = KeyContext::ALL[state.selected_context];
    if state.focus_right {
        if let Some(index) = state
            .draft
            .sorted_actions(ctx)
            .iter()
            .position(|a| *a == action)
        {
            state.selected_binding = index;
            state.listening = true;
        }
    }
}

fn keybindings_editor_bind(sources: &mut Sources, key: KeyChord) {
    let Screen::KeybindingsEditor(state) = &mut sources.screen else {
        return;
    };
    let ctx = KeyContext::ALL[state.selected_context];
    if let Some(action) = state
        .draft
        .sorted_actions(ctx)
        .get(state.selected_binding)
        .copied()
    {
        if state.adding {
            state.draft.add(ctx, action, key);
        } else {
            state.draft.replace(ctx, action, key);
        }
    }
    state.listening = false;
    state.adding = false;
}

fn keybindings_editor_save(sources: &mut Sources, drivers: &Drivers) {
    let Screen::KeybindingsEditor(state) = &sources.screen else {
        return;
    };
    drivers.persist.execute([&PersistCmd::SaveKeybindings {
        keybindings: state.draft.clone(),
    }]);
}

// ─── Arc<str> mutation helpers ─────────────────────────────────────
//
// `Arc<str>` isn't mutable in place — for char-by-char text editing
// the only option is to reify the buffer to `String`, mutate, and
// re-Arc. The cost is one alloc per keystroke, which is fine; the
// payoff is per-frame projection snaps that are pure refcount bumps
// instead of `String::clone`.

fn arc_str_push_char(s: &mut Arc<str>, c: char) {
    let mut owned = s.to_string();
    owned.push(c);
    *s = Arc::from(owned);
}

fn arc_str_pop_char(s: &mut Arc<str>) {
    let mut owned = s.to_string();
    owned.pop();
    *s = Arc::from(owned);
}

// ─── FilterInput ────────────────────────────────────────────────────

fn filter_input_type(sources: &mut Sources, c: char) {
    let Screen::FilterInput(state) = &mut sources.screen else {
        return;
    };
    arc_str_push_char(&mut state.input, c);
    let target = state.target;
    let text = state.input.clone();
    set_pane_filter(sources, target, text);
}

fn filter_input_backspace(sources: &mut Sources) {
    let Screen::FilterInput(state) = &mut sources.screen else {
        return;
    };
    arc_str_pop_char(&mut state.input);
    let target = state.target;
    let text = state.input.clone();
    set_pane_filter(sources, target, text);
}

fn filter_input_cancel(sources: &mut Sources) {
    let Screen::FilterInput(state) = &sources.screen else {
        return;
    };
    let target = state.target;
    set_pane_filter(sources, target, Arc::from(""));
    sources.screen = Screen::NowPlaying;
}

fn filter_input_submit(sources: &mut Sources) {
    let Screen::FilterInput(state) = &sources.screen else {
        return;
    };
    let target = state.target;
    let text = state.input.clone();
    set_pane_filter(sources, target, text);
    sources.screen = Screen::NowPlaying;
}

fn set_pane_filter(sources: &mut Sources, target: FilterTarget, text: Arc<str>) {
    match target {
        FilterTarget::Middle => sources.filter.middle = text,
        FilterTarget::Queue => sources.filter.queue = text,
    }
}

// ─── SearchInput ────────────────────────────────────────────────────

fn search_history_up(sources: &mut Sources) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    if state.history.is_empty() {
        return;
    }
    state.history_selected = match state.history_selected {
        None => None,
        Some(0) => None,
        Some(i) => Some(i - 1),
    };
}

fn search_history_down(sources: &mut Sources) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    if state.history.is_empty() {
        return;
    }
    state.history_selected = match state.history_selected {
        None => Some(0),
        Some(i) if i + 1 < state.history.len() => Some(i + 1),
        Some(i) => Some(i),
    };
}

fn search_input_type(sources: &mut Sources, c: char) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    arc_str_push_char(&mut state.input, c);
    state.history_selected = None;
}

fn search_input_backspace(sources: &mut Sources) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    arc_str_pop_char(&mut state.input);
    state.history_selected = None;
}

fn search_cycle_type(sources: &mut Sources, forward: bool) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    state.last_type = if forward {
        state.last_type.next()
    } else {
        state.last_type.prev()
    };
}

fn search_edit_from_history(sources: &mut Sources) {
    let Screen::SearchInput(state) = &mut sources.screen else {
        return;
    };
    if let Some(i) = state.history_selected {
        if let Some(item) = state.history.get(i) {
            state.input = item.query.clone();
            state.last_type = queries::parse_search_type(&item.search_type);
        }
    }
    state.history_selected = None;
}

fn search_submit(sources: &mut Sources) {
    let (term, search_type): (Arc<str>, SearchType) = {
        let Screen::SearchInput(state) = &sources.screen else {
            return;
        };
        if let Some(i) = state.history_selected {
            if let Some(item) = state.history.get(i).cloned() {
                (item.query, queries::parse_search_type(&item.search_type))
            } else {
                (Arc::from(state.input.trim()), state.last_type)
            }
        } else {
            (Arc::from(state.input.trim()), state.last_type)
        }
    };
    if !term.is_empty() {
        let task_id = sources.requests.alloc_task_id();
        sources.search.begin(task_id, term.clone(), search_type);
        sources.requests.push(
            ClientMsg::Search {
                term: term.to_string(),
                search_type,
            },
            Some(task_id),
        );
        history_drill(
            sources,
            MiddleMode::SearchResults {
                term: term.to_string(),
                search_type,
                task_id: Some(task_id),
            },
        );
    }
    sources.screen = Screen::NowPlaying;
}

fn open_search_input(sources: &mut Sources, drivers: &Drivers) {
    sources.screen = Screen::SearchInput(SearchState {
        input: Arc::from(""),
        last_type: SearchType::default(),
        history: imbl::Vector::new(),
        history_selected: None,
    });
    if let Some(backend) = sources.session.backend_name.clone() {
        request_load_search_history(sources, drivers, backend.to_string());
    }
}

// ─── CreatePlaylist / RenamePlaylist / ConfirmDeletePlaylist ───────

fn create_playlist_type(sources: &mut Sources, c: char) {
    if let Screen::CreatePlaylist { input, .. } = &mut sources.screen {
        arc_str_push_char(input, c);
    }
}

fn create_playlist_backspace(sources: &mut Sources) {
    if let Screen::CreatePlaylist { input, .. } = &mut sources.screen {
        arc_str_pop_char(input);
    }
}

fn create_playlist_submit(sources: &mut Sources) {
    let prev = std::mem::replace(&mut sources.screen, Screen::NowPlaying);
    let Screen::CreatePlaylist { input, add_item } = prev else {
        return;
    };
    let name = input.trim().to_string();
    if name.is_empty() {
        return;
    }
    let seq = sources
        .requests
        .push(ClientMsg::CreatePlaylist { name: name.clone() }, None);
    // Optimistic shadow entry — the left-column memo appends this to
    // the visible list with a "Creating" marker. Cleared on the
    // matching `PlaylistCreated` response (or `Error` rollback) by
    // ingest, keyed on `seq`.
    sources.pending_playlists.add_creating(seq, name.clone());
    if let Some(item) = add_item {
        sources.picker.pending_create_add = Some(PendingCreateAdd {
            name: Arc::from(name),
            item,
        });
    }
}

fn rename_playlist_type(sources: &mut Sources, c: char) {
    if let Screen::RenamePlaylist { input, .. } = &mut sources.screen {
        arc_str_push_char(input, c);
    }
}

fn rename_playlist_backspace(sources: &mut Sources) {
    if let Screen::RenamePlaylist { input, .. } = &mut sources.screen {
        arc_str_pop_char(input);
    }
}

fn rename_playlist_submit(sources: &mut Sources) {
    let prev = std::mem::replace(&mut sources.screen, Screen::NowPlaying);
    let Screen::RenamePlaylist {
        id,
        original,
        input,
    } = prev
    else {
        return;
    };
    let name = input.trim().to_string();
    if !name.is_empty() && name.as_str() != &*original {
        let seq = sources.requests.push(
            ClientMsg::RenamePlaylist {
                id: id.to_string(),
                name: name.clone(),
            },
            None,
        );
        // Optimistic: the left column shows `name` immediately
        // with a spinner. Cleared on `Renamed` broadcast (success)
        // or `Error` reply (rollback to original).
        sources
            .pending_playlists
            .add_renaming(seq, id.to_string(), name);
    }
}

fn confirm_delete_type(sources: &mut Sources, c: char) {
    if let Screen::ConfirmDeletePlaylist { input, .. } = &mut sources.screen {
        arc_str_push_char(input, c);
    }
}

fn confirm_delete_backspace(sources: &mut Sources) {
    if let Screen::ConfirmDeletePlaylist { input, .. } = &mut sources.screen {
        arc_str_pop_char(input);
    }
}

fn confirm_delete_submit(sources: &mut Sources) {
    let confirmed = match &sources.screen {
        Screen::ConfirmDeletePlaylist { name, input, .. } => {
            crate::views::confirm_delete_playlist_model(crate::views::ConfirmDeletePlaylistInput {
                name,
                input,
            })
            .matches
        }
        _ => false,
    };
    if !confirmed {
        return;
    }
    let prev = std::mem::replace(&mut sources.screen, Screen::NowPlaying);
    if let Screen::ConfirmDeletePlaylist { id, .. } = prev {
        let seq = sources
            .requests
            .push(ClientMsg::DeletePlaylist { id: id.to_string() }, None);
        // Optimistic shadow — the left-column memo filters this id
        // out of the visible list immediately. Cleared on the
        // matching `Deleted` broadcast (success) or `Error` reply
        // (rollback) by ingest.
        sources.pending_playlists.add_deleting(seq, id.to_string());
    }
}

// ─── PlaylistAction (rename / delete picker) ───────────────────────

fn playlist_action_cursor_up(sources: &mut Sources) {
    let Screen::PlaylistAction { selected, .. } = &mut sources.screen else {
        return;
    };
    *selected = if *selected == 0 { 1 } else { 0 };
}

fn playlist_action_cursor_down(sources: &mut Sources) {
    let Screen::PlaylistAction { selected, .. } = &mut sources.screen else {
        return;
    };
    *selected = (*selected + 1) % 2;
}

fn playlist_action_submit(sources: &mut Sources) {
    let Screen::PlaylistAction {
        playlist_id,
        playlist_name,
        selected,
    } = &sources.screen
    else {
        return;
    };
    let id = playlist_id.clone();
    let name = playlist_name.clone();
    let s = *selected;
    sources.screen = if s == 0 {
        Screen::RenamePlaylist {
            id,
            original: name.clone(),
            input: name,
        }
    } else {
        Screen::ConfirmDeletePlaylist {
            id,
            name,
            input: Arc::from(""),
        }
    };
}

fn playlist_action_rename(sources: &mut Sources) {
    let Screen::PlaylistAction {
        playlist_id,
        playlist_name,
        ..
    } = &sources.screen
    else {
        return;
    };
    let id = playlist_id.clone();
    let name = playlist_name.clone();
    sources.screen = Screen::RenamePlaylist {
        id,
        original: name.clone(),
        input: name,
    };
}

fn playlist_action_delete(sources: &mut Sources) {
    let Screen::PlaylistAction {
        playlist_id,
        playlist_name,
        ..
    } = &sources.screen
    else {
        return;
    };
    let id = playlist_id.clone();
    let name = playlist_name.clone();
    sources.screen = Screen::ConfirmDeletePlaylist {
        id,
        name,
        input: Arc::from(""),
    };
}

// ─── PlaylistPicker (add-to-playlist) ───────────────────────────────

fn playlist_picker_cursor_up(sources: &mut Sources) {
    let Screen::PlaylistPicker { selected, .. } = &mut sources.screen else {
        return;
    };
    *selected = selected.saturating_sub(1);
}

fn playlist_picker_cursor_down(sources: &mut Sources) {
    // n_rows = playlists + 1 ("+ New playlist" sentinel).
    let n_rows = sources.playlists.items.len() + 1;
    let Screen::PlaylistPicker { selected, .. } = &mut sources.screen else {
        return;
    };
    if *selected + 1 < n_rows {
        *selected += 1;
    }
}

fn playlist_picker_submit(sources: &mut Sources) {
    let n = sources.playlists.items.len();
    let new_row = n;
    let (picked, item) = match &sources.screen {
        Screen::PlaylistPicker { item, selected } => (*selected, item.clone()),
        _ => return,
    };
    if picked == new_row {
        // "+ New playlist" — jump into CreatePlaylist carrying the
        // original add_item. Once the server broadcasts
        // PlaylistCreated, the deferred AddToPlaylist fires.
        sources.screen = Screen::CreatePlaylist {
            input: Arc::from(""),
            add_item: Some(item),
        };
        return;
    }
    let pid = match sources.playlists.items.iter().nth(picked) {
        Some(p) => p.id.clone(),
        None => {
            sources.screen = Screen::NowPlaying;
            return;
        }
    };
    let (song_ids, album_ids): (Vec<String>, Vec<String>) = match item.kind {
        ActionKind::Song => (vec![item.id.to_string()], vec![]),
        ActionKind::Album => (vec![], vec![item.id.to_string()]),
        ActionKind::Artist => (vec![], vec![]),
    };
    let seq = sources.requests.push(
        ClientMsg::AddToPlaylist {
            playlist_id: pid.to_string(),
            song_ids,
            album_ids,
        },
        None,
    );
    // Optimistic: a "+ Adding…" placeholder row appears in the
    // playlist's track view (when loaded) until `SongAdded` lands or
    // an `Error` rolls back.
    sources.pending_playlists.add_adding(seq, pid.clone());
    sources.picker.last_add_playlist = Some(Arc::from(pid));
    sources.screen = Screen::NowPlaying;
}

// ─── ConfirmRemoveFromPlaylist ─────────────────────────────────────

fn confirm_remove_submit(sources: &mut Sources) {
    let (pid, idx) = match &sources.screen {
        Screen::ConfirmRemoveFromPlaylist {
            playlist_id,
            song_index,
            ..
        } => (playlist_id.clone(), *song_index),
        _ => return,
    };
    let song_id = sources
        .playlist_tracks
        .songs
        .get(idx)
        .and_then(|slot| slot.as_ref())
        .map(|s| s.id.clone());
    if let Some(song_id) = song_id {
        let seq = sources.requests.push(
            ClientMsg::RemoveFromPlaylist {
                playlist_id: pid.to_string(),
                items: vec![(song_id.clone(), idx)],
            },
            None,
        );
        // Optimistic: row vanishes from the playlist's track view
        // immediately. Cleared on `SongRemoved` broadcast or rolled
        // back on `Error`.
        sources
            .pending_playlists
            .add_removing(seq, pid.to_string(), vec![song_id]);
    }
    sources.screen = Screen::NowPlaying;
}

// ─── SelectionActionModal ───────────────────────────────────────────

fn selection_action_cursor_up(sources: &mut Sources) {
    let menu = queries::selection_action_menu(sources);
    let n = menu.len();
    let Screen::SelectionActionModal { selected } = &mut sources.screen else {
        return;
    };
    if n == 0 {
        return;
    }
    *selected = if *selected == 0 { n - 1 } else { *selected - 1 };
}

fn selection_action_cursor_down(sources: &mut Sources) {
    let menu = queries::selection_action_menu(sources);
    let n = menu.len();
    let Screen::SelectionActionModal { selected } = &mut sources.screen else {
        return;
    };
    if n == 0 {
        return;
    }
    *selected = (*selected + 1) % n;
}

fn selection_action_apply(sources: &mut Sources, choice: char) {
    let Some(ctx) = sources.selection.context else {
        sources.screen = Screen::NowPlaying;
        return;
    };
    let song_ids = queries::gather_selection_song_ids(sources, ctx);
    match choice {
        'n' if !song_ids.is_empty() => {
            sources.requests.push(
                ClientMsg::PlaySongs {
                    song_ids,
                    position: QueuePosition::Next,
                },
                None,
            );
            sources.selection.clear();
            sources.screen = Screen::NowPlaying;
        }
        'e' if !song_ids.is_empty() => {
            sources.requests.push(
                ClientMsg::PlaySongs {
                    song_ids,
                    position: QueuePosition::Last,
                },
                None,
            );
            sources.selection.clear();
            sources.screen = Screen::NowPlaying;
        }
        'a' if !song_ids.is_empty() => {
            // Bulk add via PlaylistPicker — wrap the list as a synthetic
            // ActionItem; the picker resolves the ids on confirm.
            let label = format!("{} songs", song_ids.len());
            let item = ActionItem::new(String::new(), ActionKind::Song, label);
            sources.screen = Screen::PlaylistPicker { item, selected: 0 };
        }
        'd' => selection_action_delete(sources, ctx),
        _ => sources.screen = Screen::NowPlaying,
    }
}

fn selection_action_delete(sources: &mut Sources, ctx: SelectionContext) {
    match ctx {
        SelectionContext::Queue => {
            let indices = queries::sorted_queue_indices(sources);
            for index in indices {
                remove_queue_entry(sources, index);
            }
            sources.selection.clear();
            sources.screen = Screen::NowPlaying;
        }
        SelectionContext::Middle => {
            if let MiddleMode::PlaylistSongs = sources.history.mode {
                let pid = match sources.playlist_tracks.playlist_id.clone() {
                    Some(p) => p,
                    None => {
                        sources.screen = Screen::NowPlaying;
                        return;
                    }
                };
                let items: Vec<(String, usize)> = sources
                    .selection
                    .selected
                    .iter()
                    .filter_map(|&i| {
                        sources
                            .playlist_tracks
                            .songs
                            .get(i)
                            .and_then(|s| s.as_ref())
                            .map(|s| (s.id.clone(), i))
                    })
                    .collect();
                if !items.is_empty() {
                    let song_ids: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();
                    let seq = sources.requests.push(
                        ClientMsg::RemoveFromPlaylist {
                            playlist_id: pid.to_string(),
                            items,
                        },
                        None,
                    );
                    // Optimistic bulk-remove: rows vanish from the
                    // visible track list immediately.
                    sources
                        .pending_playlists
                        .add_removing(seq, pid.to_string(), song_ids);
                }
            }
            sources.selection.clear();
            sources.screen = Screen::NowPlaying;
        }
    }
}

// ─── ServerLostModal ────────────────────────────────────────────────

fn server_lost_give_up(sources: &mut Sources, drivers: &Drivers) {
    sources.session.lost_server = None;
    sources.session.preferred_server = None;
    sources.session.auto_connect = false;
    // Giving up is the point at which the retained view stops being
    // worth keeping: the user is going to pick a different server, so
    // the old one's playlists / queue / tracks must not bleed into it.
    // (A plain drop keeps them — see `ingest`'s `LinkEvent::Closed`.)
    sources.queue = Default::default();
    sources.playlists = Default::default();
    sources.playlist_tracks.clear();
    sources.search.clear();
    sources.artist_extras.clear();
    sources.server = Default::default();
    sources.link.clear_retry();
    sources.screen = Screen::NowPlaying;
    // Unconditionally: `intent` is what `apply_link` dials from, and it
    // survives a close. Leaving it set would redial the very server the
    // user just walked away from, onto the view cleared above. The
    // phase-guarded `disconnect` used to cover this only because a drop
    // parked on `Closed`; the link is released to `Idle` now.
    sources.intent.target = None;
    sources.intent.pair_target = None;
    if matches!(
        sources.link.phase,
        LinkPhase::Connected | LinkPhase::Connecting
    ) {
        disconnect(sources, drivers);
    }
}

// ─── Selection mode ─────────────────────────────────────────────────

fn selection_add(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let n = queries::selection_row_count(sources, ctx);
    if n == 0 {
        return;
    }
    let cursor_val = *pane_cursor_mut(sources, ctx);
    sources.selection.add(cursor_val);
    *pane_cursor_mut(sources, ctx) = (cursor_val + 1).min(n - 1);
}

fn selection_remove(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let cursor = *pane_cursor_mut(sources, ctx);
    sources.selection.remove(cursor);
}

fn selection_toggle_anchor(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let cursor = *pane_cursor_mut(sources, ctx);
    if sources.selection.range_anchor.is_some() {
        // Finalize the live range — drop the anchor + range tracking
        // but leave `selected` as it stands (matches legacy
        // `mkp2 nav/player/selection.rs::ToggleRangeAnchor`).
        sources.selection.range_anchor = None;
        sources.selection.range_selected.clear();
        return;
    }
    sources.selection.range_anchor = Some(cursor);
    sources.selection.add(cursor);
    sources.selection.range_selected.clear();
    sources.selection.range_selected.insert(cursor);
}

fn selection_move_up(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let n = queries::selection_row_count(sources, ctx);
    if n == 0 {
        return;
    }
    let cursor = pane_cursor_mut(sources, ctx);
    *cursor = cursor.saturating_sub(1);
    update_range_selection(sources);
}

fn selection_move_down(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let n = queries::selection_row_count(sources, ctx);
    if n == 0 {
        return;
    }
    let cursor = pane_cursor_mut(sources, ctx);
    if *cursor + 1 < n {
        *cursor += 1;
    }
    update_range_selection(sources);
}

/// Legacy parity (`mkp2 nav/player/selection.rs::update_range_selection`):
/// after the cursor moves while a range anchor is active, recompute
/// the contiguous anchor→cursor span. Items that were in the
/// previous range but fall outside the new range are removed from
/// `selected`; new in-range items are added. Explicit toggles
/// outside the range are untouched.
fn update_range_selection(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let Some(anchor) = sources.selection.range_anchor else {
        return;
    };
    let cursor = *pane_cursor_mut(sources, ctx);
    let lo = anchor.min(cursor);
    let hi = anchor.max(cursor);
    let new_range: imbl::OrdSet<usize> = (lo..=hi).collect();
    let prev_range = std::mem::take(&mut sources.selection.range_selected);
    for i in prev_range.iter() {
        if !new_range.contains(i) {
            sources.selection.selected.remove(i);
        }
    }
    for i in new_range.iter() {
        sources.selection.selected.insert(*i);
    }
    sources.selection.range_selected = new_range;
}

fn selection_play_reset(sources: &mut Sources) {
    let Some(ctx) = sources.selection.context else {
        return;
    };
    let song_ids = queries::gather_selection_song_ids(sources, ctx);
    if !song_ids.is_empty() {
        sources.requests.push(
            ClientMsg::PlaySongs {
                song_ids,
                position: QueuePosition::Reset,
            },
            None,
        );
    }
    sources.selection.clear();
}

fn pane_cursor_mut(sources: &mut Sources, ctx: SelectionContext) -> &mut usize {
    match ctx {
        SelectionContext::Middle => &mut sources.cursor.middle,
        SelectionContext::Queue => &mut sources.cursor.queue,
    }
}

// ─── Server picker ──────────────────────────────────────────────────

fn picker_cursor_up(sources: &mut Sources) {
    let n = sources.discovery.servers.len();
    if n == 0 {
        return;
    }
    let sel = &mut sources.cursor.server_picker;
    *sel = if *sel == 0 { n - 1 } else { *sel - 1 };
}

fn picker_cursor_down(sources: &mut Sources) {
    let n = sources.discovery.servers.len();
    if n == 0 {
        return;
    }
    let sel = &mut sources.cursor.server_picker;
    *sel = (*sel + 1) % n;
}

fn picker_connect(sources: &mut Sources) {
    let Some(name) = sources
        .discovery
        .servers
        .iter()
        .nth(sources.cursor.server_picker)
        .map(|s| s.name.clone())
    else {
        return;
    };
    sources.session.lost_server = None;
    sources.session.auto_connect = false;
    connect_to(sources, name.clone());
    sources.toast.show(
        format!("connecting to {name}…"),
        sources.clock.now + Duration::from_secs(3),
    );
}

fn server_picker_modal_cursor_up(sources: &mut Sources) {
    let n = sources.discovery.servers.len();
    if n == 0 {
        return;
    }
    let Screen::ServerPicker { selected } = &mut sources.screen else {
        return;
    };
    *selected = selected.saturating_sub(1);
}

fn server_picker_modal_cursor_down(sources: &mut Sources) {
    let n = sources.discovery.servers.len();
    if n == 0 {
        return;
    }
    let Screen::ServerPicker { selected } = &mut sources.screen else {
        return;
    };
    if *selected + 1 < n {
        *selected += 1;
    }
}

fn server_picker_modal_select(sources: &mut Sources, drivers: &Drivers) {
    let Screen::ServerPicker { selected } = sources.screen else {
        return;
    };
    let Some(name) = sources
        .discovery
        .servers
        .iter()
        .nth(selected)
        .map(|s| s.name.clone())
    else {
        sources.screen = Screen::NowPlaying;
        return;
    };
    // Same server → just close the modal.
    if sources.session.backend_name.as_deref() == Some(name.as_str()) {
        sources.screen = Screen::NowPlaying;
        return;
    }
    // Different server: swap intent + tear down the current link so
    // the auto-connect lifecycle brings the new target up.
    sources.session.preferred_server = Some(Arc::from(name.as_str()));
    sources.session.auto_connect = true;
    sources.session.lost_server = None;
    connect_to(sources, name);
    if matches!(
        sources.link.phase,
        LinkPhase::Connected | LinkPhase::Connecting
    ) {
        drivers.link.execute([&LinkCmd::Disconnect]);
    }
    sources.screen = Screen::NowPlaying;
}

// ─── Left pane ──────────────────────────────────────────────────────

fn left_n_rows(sources: &Sources) -> usize {
    // [server][playlists…][+ New]
    queries::filtered_playlist_count(sources, &sources.filter.playlist) + 2
}

fn left_cursor_up(sources: &mut Sources) {
    if left_n_rows(sources) == 0 {
        return;
    }
    sources.cursor.left = sources.cursor.left.saturating_sub(1);
}

fn left_cursor_down(sources: &mut Sources) {
    let n = left_n_rows(sources);
    if n == 0 {
        return;
    }
    if sources.cursor.left + 1 < n {
        sources.cursor.left += 1;
    }
}

fn left_activate(sources: &mut Sources) {
    let n_playlists = queries::filtered_playlist_count(sources, &sources.filter.playlist);
    let server_row = 0usize;
    let playlist_start = 1usize;
    let new_row = playlist_start + n_playlists;
    if sources.cursor.left == server_row {
        // Legacy parity (`mkp2 nav/server_picker.rs`): open a
        // server-picker modal *on top of* the main view rather
        // than disconnecting. The user can then pick a different
        // server (which fires a swap) or Esc out without losing
        // the current connection. Cursor starts on the currently-
        // connected server so `Enter` is a no-op by default.
        let backend = sources.session.backend_name.as_deref();
        let selected = backend
            .and_then(|b| {
                sources
                    .discovery
                    .servers
                    .iter()
                    .position(|s| s.name.as_str() == b)
            })
            .unwrap_or(0);
        sources.screen = Screen::ServerPicker { selected };
    } else if sources.cursor.left == new_row {
        sources.screen = Screen::CreatePlaylist {
            input: Arc::from(""),
            add_item: None,
        };
    } else if let Some(id) = queries::selected_playlist_id(
        sources,
        &sources.filter.playlist,
        sources.cursor.left.saturating_sub(playlist_start),
    ) {
        view_playlist(sources, id);
        history_drill(sources, MiddleMode::PlaylistSongs);
    }
}

fn left_open_action(sources: &mut Sources) {
    let n = queries::filtered_playlist_count(sources, &sources.filter.playlist);
    if sources.cursor.left < 1 || sources.cursor.left > n {
        return;
    }
    let idx = sources.cursor.left - 1;
    let Some(p) = queries::filtered_playlist(sources, &sources.filter.playlist, idx) else {
        return;
    };
    let playlist_id = Arc::from(p.id.as_str());
    let playlist_name = Arc::from(p.name.as_str());
    sources.screen = Screen::PlaylistAction {
        playlist_id,
        playlist_name,
        selected: 0,
    };
}

// ─── Middle pane ────────────────────────────────────────────────────

fn middle_cursor_up(sources: &mut Sources) {
    let n = queries::middle_row_count(sources);
    if n == 0 {
        return;
    }
    sources.cursor.middle = sources.cursor.middle.saturating_sub(1);
    if let Some(song) = queries::hovered_middle_song(sources) {
        sources
            .preview
            .set(song, sources.clock.now + Duration::from_secs(3));
    }
}

fn middle_cursor_down(sources: &mut Sources) {
    let n = queries::middle_row_count(sources);
    if n == 0 {
        return;
    }
    if sources.cursor.middle + 1 < n {
        sources.cursor.middle += 1;
    }
    if let Some(song) = queries::hovered_middle_song(sources) {
        sources
            .preview
            .set(song, sources.clock.now + Duration::from_secs(3));
    }
}

fn middle_activate(sources: &mut Sources) {
    let n = queries::middle_row_count(sources);
    if n == 0 {
        return;
    }
    let Some(&orig_idx) = queries::middle_filtered_indices(sources).get(sources.cursor.middle)
    else {
        return;
    };
    match sources.history.mode.clone() {
        MiddleMode::PlaylistSongs => {
            if let Some(id) = sources.playlist_tracks.playlist_id.clone() {
                sources.requests.push(
                    ClientMsg::Play {
                        id: id.to_string(),
                        kind: MediaKind::Playlist,
                        position: QueuePosition::Reset,
                        start_index: Some(orig_idx),
                    },
                    None,
                );
            }
        }
        MiddleMode::SearchResults { search_type, .. } => match search_type {
            SearchType::Song => {
                if let Some(s) = sources.search.songs.get(orig_idx) {
                    sources.requests.push(
                        ClientMsg::Play {
                            id: s.id.clone(),
                            kind: MediaKind::Song,
                            position: QueuePosition::Reset,
                            start_index: None,
                        },
                        None,
                    );
                }
            }
            SearchType::Album => {
                let pair = sources
                    .search
                    .albums
                    .get(orig_idx)
                    .map(|a| (a.id.clone(), a.name.clone()));
                if let Some((id, name)) = pair {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetAlbumDetail { id: id.clone() }, None);
                    history_drill(
                        sources,
                        MiddleMode::AlbumDetail {
                            album_id: id,
                            album_title: name,
                            awaiting_seq: Some(seq),
                        },
                    );
                }
            }
            SearchType::Artist => {
                let pair = sources
                    .search
                    .artists
                    .get(orig_idx)
                    .map(|a| (a.id.clone(), a.name.clone()));
                if let Some((id, name)) = pair {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetArtistDetail { id: id.clone() }, None);
                    history_drill(
                        sources,
                        MiddleMode::ArtistDetail {
                            artist_id: id,
                            artist_name: name,
                            awaiting_seq: Some(seq),
                        },
                    );
                }
            }
        },
        MiddleMode::AlbumDetail { album_id, .. } => {
            sources.requests.push(
                ClientMsg::Play {
                    id: album_id.clone(),
                    kind: MediaKind::Album,
                    position: QueuePosition::Reset,
                    start_index: Some(orig_idx),
                },
                None,
            );
        }
        MiddleMode::ArtistDetail { awaiting_seq, .. } => {
            match queries::artist_detail_item(awaiting_seq, sources, orig_idx) {
                Some(queries::ArtistDetailItem::Song(s)) => {
                    sources.requests.push(
                        ClientMsg::Play {
                            id: s.id,
                            kind: MediaKind::Song,
                            position: QueuePosition::Reset,
                            start_index: None,
                        },
                        None,
                    );
                }
                Some(queries::ArtistDetailItem::Album(a)) => {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetAlbumDetail { id: a.id.clone() }, None);
                    history_drill(
                        sources,
                        MiddleMode::AlbumDetail {
                            album_id: a.id,
                            album_title: a.name,
                            awaiting_seq: Some(seq),
                        },
                    );
                }
                Some(queries::ArtistDetailItem::Artist(a)) => {
                    let seq = sources
                        .requests
                        .push(ClientMsg::GetArtistDetail { id: a.id.clone() }, None);
                    history_drill(
                        sources,
                        MiddleMode::ArtistDetail {
                            artist_id: a.id,
                            artist_name: a.name,
                            awaiting_seq: Some(seq),
                        },
                    );
                }
                None => {}
            }
        }
    }
}

fn middle_open_action(sources: &mut Sources) {
    let n = queries::middle_row_count(sources);
    if n == 0 {
        return;
    }
    let Some(item) = queries::current_middle_action_item(sources) else {
        return;
    };
    sources.screen = Screen::ActionModal(ActionModalState { item, selected: 0 });
}

// ─── Queue pane ─────────────────────────────────────────────────────

fn queue_cursor_up(sources: &mut Sources) {
    let n = queries::queue_filtered_indices(sources).len();
    if n == 0 {
        return;
    }
    sources.cursor.queue = sources.cursor.queue.saturating_sub(1);
    if let Some(song) = queries::hovered_queue_song(sources) {
        sources
            .preview
            .set(song, sources.clock.now + Duration::from_secs(3));
    }
}

fn queue_cursor_down(sources: &mut Sources) {
    let n = queries::queue_filtered_indices(sources).len();
    if n == 0 {
        return;
    }
    if sources.cursor.queue + 1 < n {
        sources.cursor.queue += 1;
    }
    if let Some(song) = queries::hovered_queue_song(sources) {
        sources
            .preview
            .set(song, sources.clock.now + Duration::from_secs(3));
    }
}

fn queue_activate(sources: &mut Sources) {
    let n = queries::queue_filtered_indices(sources).len();
    if n == 0 {
        return;
    }
    if let Some(&orig) = queries::queue_filtered_indices(sources).get(sources.cursor.queue) {
        if let (Some(queue_id), Some(&entry_id)) =
            (sources.queue.queue_id, sources.queue.entry_ids.get(orig))
        {
            sources
                .requests
                .push(ClientMsg::SkipToQueueEntry { queue_id, entry_id }, None);
        }
    }
}

fn remove_queue_entry(sources: &mut Sources, index: usize) {
    if let (Some(queue_id), Some(&entry_id)) =
        (sources.queue.queue_id, sources.queue.entry_ids.get(index))
    {
        sources
            .requests
            .push(ClientMsg::RemoveFromQueue { queue_id, entry_id }, None);
    }
}

fn queue_open_action(sources: &mut Sources) {
    let n = queries::queue_filtered_indices(sources).len();
    if n == 0 {
        return;
    }
    let Some(item) = queries::current_queue_action_item(sources) else {
        return;
    };
    sources.screen = Screen::ActionModal(ActionModalState { item, selected: 0 });
}

// ─── Transport / globals ────────────────────────────────────────────

fn toggle_play_pause(sources: &mut Sources) {
    let is_playing = sources
        .server
        .play
        .as_ref()
        .map(|p| p.playback == mkproto::PlaybackState::Playing)
        .unwrap_or(false);
    sources
        .requests
        .push(ClientMsg::SetPaused { paused: is_playing }, None);
}

fn skip_next(sources: &mut Sources) {
    sources.requests.push(ClientMsg::Skip, None);
}

fn skip_previous(sources: &mut Sources) {
    sources.requests.push(ClientMsg::Previous, None);
}

fn seek_relative(sources: &mut Sources, offset: f32) {
    sources.requests.push(
        ClientMsg::SeekRelative {
            offset: offset as f64,
        },
        None,
    );
}

fn cycle_repeat_mode(sources: &mut Sources) {
    let next = match sources
        .server
        .play
        .as_ref()
        .map(|p| p.repeat)
        .unwrap_or(RepeatMode::Off)
    {
        RepeatMode::Off => RepeatMode::All,
        RepeatMode::All => RepeatMode::One,
        RepeatMode::One => RepeatMode::Off,
    };
    sources
        .requests
        .push(ClientMsg::SetRepeat { mode: next }, None);
}

fn open_filter_input_for_focused(sources: &mut Sources) {
    // Legacy parity: Shift-F is a no-op when the Left pane is
    // focused — only Middle/Queue have a filter.
    let target = match sources.cursor.focus {
        ColumnFocus::Middle => FilterTarget::Middle,
        ColumnFocus::Queue => FilterTarget::Queue,
        ColumnFocus::Left => return,
    };
    let initial = match target {
        FilterTarget::Middle => sources.filter.middle.clone(),
        FilterTarget::Queue => sources.filter.queue.clone(),
    };
    sources.screen = Screen::FilterInput(FilterState {
        target,
        input: initial,
    });
}

fn toggle_selection_for_focused(sources: &mut Sources) {
    let ctx = match sources.cursor.focus {
        ColumnFocus::Middle => Some(SelectionContext::Middle),
        ColumnFocus::Queue => Some(SelectionContext::Queue),
        ColumnFocus::Left => None,
    };
    let Some(ctx) = ctx else { return };
    if sources.selection.is_active() {
        sources.selection.clear();
    } else {
        sources.selection.begin(ctx);
    }
}

fn jump_focused(sources: &mut Sources, target: JumpTarget) {
    match sources.cursor.focus {
        ColumnFocus::Left => {
            // Indices: 0=server, 1..=N=playlists, N+1=New row.
            // Bottom should land on the New row (legacy parity).
            let new_row = queries::filtered_playlist_count(sources, &sources.filter.playlist) + 1;
            sources.cursor.left = match target {
                JumpTarget::Top => 0,
                JumpTarget::Bottom => new_row,
            };
        }
        ColumnFocus::Middle => {
            let n = queries::middle_filtered_indices(sources).len();
            sources.cursor.middle = match target {
                JumpTarget::Top => 0,
                JumpTarget::Bottom => n.saturating_sub(1),
            };
        }
        ColumnFocus::Queue => {
            let n = queries::queue_filtered_indices(sources).len();
            sources.cursor.queue = match target {
                JumpTarget::Top => 0,
                JumpTarget::Bottom => n.saturating_sub(1),
            };
        }
    }
}

fn page_focused(sources: &mut Sources, step: PageStep) {
    let apply = |cur: usize, max: usize| -> usize {
        match step {
            PageStep::Up => cur.saturating_sub(PAGE_SIZE),
            PageStep::Down => (cur + PAGE_SIZE).min(max),
        }
    };
    match sources.cursor.focus {
        ColumnFocus::Left => {
            let n = left_n_rows(sources);
            if n == 0 {
                return;
            }
            sources.cursor.left = apply(sources.cursor.left, n - 1);
        }
        ColumnFocus::Middle => {
            let n = queries::middle_row_count(sources);
            if n == 0 {
                return;
            }
            sources.cursor.middle = apply(sources.cursor.middle, n - 1);
            if let Some(song) = queries::hovered_middle_song(sources) {
                sources
                    .preview
                    .set(song, sources.clock.now + Duration::from_secs(3));
            }
        }
        ColumnFocus::Queue => {
            let n = queries::queue_filtered_indices(sources).len();
            if n == 0 {
                return;
            }
            sources.cursor.queue = apply(sources.cursor.queue, n - 1);
            if let Some(song) = queries::hovered_queue_song(sources) {
                sources
                    .preview
                    .set(song, sources.clock.now + Duration::from_secs(3));
            }
        }
    }
}

fn shuffle_activate_focused(sources: &mut Sources) {
    match sources.cursor.focus {
        ColumnFocus::Left => {
            if let Some(id) = queries::selected_playlist_id(
                sources,
                &sources.filter.playlist,
                sources.cursor.left.saturating_sub(1),
            ) {
                sources.requests.push(
                    ClientMsg::Play {
                        id,
                        kind: MediaKind::Playlist,
                        position: QueuePosition::Shuffle,
                        start_index: None,
                    },
                    None,
                );
            }
        }
        ColumnFocus::Middle => {
            let msg = match &sources.history.mode {
                MiddleMode::PlaylistSongs => {
                    sources
                        .playlist_tracks
                        .playlist_id
                        .clone()
                        .map(|id| ClientMsg::Play {
                            id: id.to_string(),
                            kind: MediaKind::Playlist,
                            position: QueuePosition::Shuffle,
                            start_index: None,
                        })
                }
                MiddleMode::AlbumDetail { album_id, .. } => Some(ClientMsg::Play {
                    id: album_id.clone(),
                    kind: MediaKind::Album,
                    position: QueuePosition::Shuffle,
                    start_index: None,
                }),
                _ => None,
            };
            if let Some(msg) = msg {
                sources.requests.push(msg, None);
            }
        }
        ColumnFocus::Queue => {
            // No queue-shuffle in legacy; fall through.
        }
    }
}

fn open_action_menu_or_cycle(sources: &mut Sources) {
    let opened = match sources.cursor.focus {
        ColumnFocus::Left => {
            let n = queries::filtered_playlist_count(sources, &sources.filter.playlist);
            if sources.cursor.left >= 1 && sources.cursor.left <= n {
                let idx = sources.cursor.left - 1;
                if let Some(p) = queries::filtered_playlist(sources, &sources.filter.playlist, idx)
                {
                    let playlist_id = Arc::from(p.id.as_str());
                    let playlist_name = Arc::from(p.name.as_str());
                    sources.screen = Screen::PlaylistAction {
                        playlist_id,
                        playlist_name,
                        selected: 0,
                    };
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        ColumnFocus::Middle => {
            if let Some(item) = queries::current_middle_action_item(sources) {
                sources.screen = Screen::ActionModal(ActionModalState { item, selected: 0 });
                true
            } else {
                false
            }
        }
        ColumnFocus::Queue => {
            if let Some(item) = queries::current_queue_action_item(sources) {
                sources.screen = Screen::ActionModal(ActionModalState { item, selected: 0 });
                true
            } else {
                false
            }
        }
    };
    if !opened {
        sources.cursor.cycle_focus_forward();
        snap_queue_cursor_to_current(sources);
    }
}

/// Legacy parity (`mkp2 app/server.rs:222-228`): when focus moves
/// onto the Queue pane, drop the cursor on the now-playing track
/// so the user lands on the track they're listening to. Translates
/// `queue.current_index` into queue-filter space; falls back to 0
/// when the current track is filtered out.
fn snap_queue_cursor_to_current(sources: &mut Sources) {
    if sources.cursor.focus != ColumnFocus::Queue {
        return;
    }
    let Some(ci) = sources.queue.current_index else {
        return;
    };
    let filtered = queries::queue_filtered_indices(sources);
    sources.cursor.queue = filtered.iter().position(|&i| i == ci).unwrap_or(0);
}

fn clear_focused_filter(sources: &mut Sources) {
    match sources.cursor.focus {
        ColumnFocus::Left => {
            if !sources.filter.playlist.is_empty() {
                sources.filter.playlist = Arc::from("");
                sources.cursor.left = 0;
            }
        }
        ColumnFocus::Middle => {
            if !sources.filter.middle.is_empty() {
                sources.filter.middle = Arc::from("");
                sources.cursor.middle = 0;
            }
        }
        ColumnFocus::Queue => {
            if !sources.filter.queue.is_empty() {
                sources.filter.queue = Arc::from("");
                sources.cursor.queue = 0;
            }
        }
    }
}

// ─── Saved-view restore (called from the auto_restore hook) ────────

fn restore_saved_playlist(
    sources: &mut Sources,
    playlist_id: String,
    selected: usize,
    selected_id: Option<String>,
) {
    let exists = sources.playlists.items.iter().any(|p| p.id == playlist_id);
    if !exists {
        return;
    }
    view_playlist(sources, playlist_id);
    sources.history.mode = MiddleMode::PlaylistSongs;
    sources.cursor.middle = selected;
    sources.session.pending_cursor_song_id = selected_id.map(Arc::from);
    sources.cursor.focus = ColumnFocus::Middle;
}

fn restore_saved_album(
    sources: &mut Sources,
    album_id: String,
    album_name: String,
    selected: usize,
    selected_id: Option<String>,
) {
    let seq = sources.requests.push(
        ClientMsg::GetAlbumDetail {
            id: album_id.clone(),
        },
        None,
    );
    sources.history.mode = MiddleMode::AlbumDetail {
        album_id,
        album_title: album_name,
        awaiting_seq: Some(seq),
    };
    sources.cursor.middle = selected;
    sources.session.pending_cursor_song_id = selected_id.map(Arc::from);
    sources.cursor.focus = ColumnFocus::Middle;
}

fn restore_saved_artist(
    sources: &mut Sources,
    artist_id: String,
    artist_name: String,
    selected: usize,
) {
    let seq = sources.requests.push(
        ClientMsg::GetArtistDetail {
            id: artist_id.clone(),
        },
        None,
    );
    sources.history.mode = MiddleMode::ArtistDetail {
        artist_id,
        artist_name,
        awaiting_seq: Some(seq),
    };
    sources.cursor.middle = selected;
    sources.cursor.focus = ColumnFocus::Middle;
}

fn restore_saved_search(
    sources: &mut Sources,
    query: String,
    search_type: SearchType,
    selected: usize,
    selected_id: Option<String>,
) {
    let task_id = sources.requests.alloc_task_id();
    sources
        .search
        .begin(task_id, Arc::from(query.as_str()), search_type);
    sources.requests.push(
        ClientMsg::Search {
            term: query.clone(),
            search_type,
        },
        Some(task_id),
    );
    sources.history.mode = MiddleMode::SearchResults {
        term: query,
        search_type,
        task_id: Some(task_id),
    };
    sources.cursor.middle = selected;
    if selected_id.is_some() {
        sources.session.pending_cursor_song_id = selected_id.map(Arc::from);
    }
    sources.cursor.focus = ColumnFocus::Middle;
}

fn open_first_playlist(sources: &mut Sources, id: String) {
    view_playlist(sources, id);
    sources.history.mode = MiddleMode::PlaylistSongs;
    sources.cursor.middle = 0;
}

pub fn fire_deferred_add_to_playlist_pub(sources: &mut Sources, playlist_id: String) {
    fire_deferred_add_to_playlist(sources, playlist_id);
}

fn fire_deferred_add_to_playlist(sources: &mut Sources, playlist_id: String) {
    let Some(pending) = sources.picker.pending_create_add.take() else {
        return;
    };
    let (song_ids, album_ids): (Vec<String>, Vec<String>) = match pending.item.kind {
        ActionKind::Song => (vec![pending.item.id.to_string()], vec![]),
        ActionKind::Album => (vec![], vec![pending.item.id.to_string()]),
        ActionKind::Artist => (vec![], vec![]),
    };
    let seq = sources.requests.push(
        ClientMsg::AddToPlaylist {
            playlist_id: playlist_id.clone(),
            song_ids,
            album_ids,
        },
        None,
    );
    sources
        .pending_playlists
        .add_adding(seq, playlist_id.clone());
    sources.picker.last_add_playlist = Some(Arc::from(playlist_id));
}

fn snap_middle_cursor_to_song_id(sources: &mut Sources, target: String) {
    match sources.history.mode.clone() {
        MiddleMode::PlaylistSongs => {
            if sources.playlist_tracks.songs.is_empty() {
                return;
            }
            if let Some((i, _)) = sources
                .playlist_tracks
                .songs
                .iter()
                .enumerate()
                .find(|(_, s)| s.as_ref().map(|s| s.id == target).unwrap_or(false))
            {
                sources.cursor.middle = i;
            }
            sources.session.pending_cursor_song_id = None;
        }
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            if let Some(songs) = queries::album_detail_songs(awaiting_seq, sources) {
                if let Some((i, _)) = songs.iter().enumerate().find(|(_, s)| s.id == target) {
                    sources.cursor.middle = i;
                }
                sources.session.pending_cursor_song_id = None;
            }
        }
        MiddleMode::SearchResults { search_type, .. } => match search_type {
            SearchType::Song => {
                if !sources.search.songs.is_empty() {
                    if let Some((i, _)) = sources
                        .search
                        .songs
                        .iter()
                        .enumerate()
                        .find(|(_, s)| s.id == target)
                    {
                        sources.cursor.middle = i;
                    }
                    sources.session.pending_cursor_song_id = None;
                }
            }
            SearchType::Album => {
                if !sources.search.albums.is_empty() {
                    if let Some((i, _)) = sources
                        .search
                        .albums
                        .iter()
                        .enumerate()
                        .find(|(_, a)| a.id == target)
                    {
                        sources.cursor.middle = i;
                    }
                    sources.session.pending_cursor_song_id = None;
                }
            }
            SearchType::Artist => {
                if !sources.search.artists.is_empty() {
                    if let Some((i, _)) = sources
                        .search
                        .artists
                        .iter()
                        .enumerate()
                        .find(|(_, a)| a.id == target)
                    {
                        sources.cursor.middle = i;
                    }
                    sources.session.pending_cursor_song_id = None;
                }
            }
        },
        _ => {
            sources.session.pending_cursor_song_id = None;
        }
    }
}

// ─── Persist driver helpers ─────────────────────────────────────────

/// Snapshot the current middle-pane state to a `SavedView` and ship
/// it to the persist driver. No-op if the current mode doesn't have
/// enough state to round-trip (e.g. PlaylistSongs without a loaded
/// playlist id).
pub fn save_current_view(sources: &Sources, drivers: &Drivers, backend: String) {
    let Some(view) = build_saved_view(sources) else {
        return;
    };
    drivers
        .persist
        .execute([&PersistCmd::SaveView { backend, view }]);
}

pub(crate) fn build_saved_view(sources: &Sources) -> Option<SavedView> {
    match &sources.history.mode {
        MiddleMode::PlaylistSongs => {
            let pid = sources.playlist_tracks.playlist_id.clone()?;
            let song_id = sources
                .playlist_tracks
                .songs
                .get(sources.cursor.middle)
                .and_then(|slot| slot.as_ref())
                .map(|s| s.id.clone())
                .unwrap_or_default();
            Some(SavedView::Playlist {
                playlist_id: pid.to_string(),
                selected: sources.cursor.middle,
                offset: 0,
                selected_id: song_id,
            })
        }
        MiddleMode::AlbumDetail {
            album_id,
            album_title,
            awaiting_seq,
        } => {
            let songs = queries::album_detail_songs(*awaiting_seq, sources);
            let song_id = songs
                .and_then(|s| s.get(sources.cursor.middle).cloned())
                .map(|s| s.id)
                .unwrap_or_default();
            Some(SavedView::AlbumDetail {
                album_id: album_id.clone(),
                album_name: album_title.clone(),
                selected: sources.cursor.middle,
                offset: 0,
                selected_id: song_id,
            })
        }
        MiddleMode::ArtistDetail {
            artist_id,
            artist_name,
            ..
        } => Some(SavedView::ArtistDetail {
            artist_id: artist_id.clone(),
            artist_name: artist_name.clone(),
            selected: sources.cursor.middle,
            offset: 0,
        }),
        MiddleMode::SearchResults {
            term, search_type, ..
        } => {
            let st = queries::search_type_str(*search_type);
            let song_id = sources
                .search
                .songs
                .get(sources.cursor.middle)
                .map(|s| s.id.clone())
                .unwrap_or_default();
            Some(SavedView::Search {
                query: term.clone(),
                search_type: st.into(),
                selected: sources.cursor.middle,
                offset: 0,
                selected_id: song_id,
            })
        }
    }
}

/// Apply a `SavedView` to sources — same shape as the saved-view
/// dispatch handlers, exposed so ingest_persist can run it inline
/// when a `ViewLoaded` event lands.
///
/// If the saved view points at a playlist that no longer exists
/// (e.g. the user deleted it on Apple Music), fall back to first-run
/// behaviour and open the first available playlist instead. Same
/// idea as `RestoreAction::OpenFirst` — keeps the UI usable when the
/// saved state has gone stale.
pub fn apply_saved_view(sources: &mut Sources, view: SavedView) {
    if let SavedView::Playlist { playlist_id, .. } = &view {
        let exists = sources.playlists.items.iter().any(|p| p.id == *playlist_id);
        if !exists {
            if let Some(first_id) = sources.playlists.items.iter().next().map(|p| p.id.clone()) {
                open_first_playlist(sources, first_id);
            }
            return;
        }
    }
    match view {
        SavedView::Playlist {
            playlist_id,
            selected,
            selected_id,
            ..
        } => restore_saved_playlist(sources, playlist_id, selected, nonempty(selected_id)),
        SavedView::AlbumDetail {
            album_id,
            album_name,
            selected,
            selected_id,
            ..
        } => restore_saved_album(
            sources,
            album_id,
            album_name,
            selected,
            nonempty(selected_id),
        ),
        SavedView::ArtistDetail {
            artist_id,
            artist_name,
            selected,
            ..
        } => restore_saved_artist(sources, artist_id, artist_name, selected),
        SavedView::Search {
            query,
            search_type,
            selected,
            selected_id,
            ..
        } => restore_saved_search(
            sources,
            query,
            queries::parse_search_type(&search_type),
            selected,
            nonempty(selected_id),
        ),
    }
}

/// `OpenFirstPlaylist`-equivalent reachable from ingest_persist.
pub fn open_first_playlist_pub(sources: &mut Sources, id: String) {
    open_first_playlist(sources, id);
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn request_load_view(sources: &mut Sources, drivers: &Drivers, backend: String) {
    let key = LoadKey::View(backend.clone());
    if sources.persist.is_loading(&key) {
        return;
    }
    sources.persist.loads_in_flight.insert(key);
    drivers.persist.execute([&PersistCmd::LoadView { backend }]);
}

fn request_load_last_add_playlist(sources: &mut Sources, drivers: &Drivers, backend: String) {
    let key = LoadKey::LastAddPlaylist(backend.clone());
    if sources.persist.is_loading(&key) {
        return;
    }
    sources.persist.loads_in_flight.insert(key);
    drivers
        .persist
        .execute([&PersistCmd::LoadLastAddPlaylist { backend }]);
}

pub(crate) fn request_load_search_history(
    sources: &mut Sources,
    drivers: &Drivers,
    backend: String,
) {
    let key = LoadKey::SearchHistory(backend.clone());
    if sources.persist.is_loading(&key) {
        return;
    }
    sources.persist.loads_in_flight.insert(key);
    drivers
        .persist
        .execute([&PersistCmd::LoadSearchHistory { backend }]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_next_presses_are_all_queued() {
        let mut sources = Sources::default();

        skip_next(&mut sources);
        skip_next(&mut sources);

        let messages: Vec<_> = sources
            .requests
            .pending
            .iter()
            .map(|pending| &pending.msg)
            .collect();
        assert!(matches!(
            messages.as_slice(),
            [ClientMsg::Skip, ClientMsg::Skip]
        ));
    }

    #[test]
    fn rapid_previous_presses_are_all_queued() {
        let mut sources = Sources::default();

        skip_previous(&mut sources);
        skip_previous(&mut sources);

        let messages: Vec<_> = sources
            .requests
            .pending
            .iter()
            .map(|pending| &pending.msg)
            .collect();
        assert!(matches!(
            messages.as_slice(),
            [ClientMsg::Previous, ClientMsg::Previous]
        ));
    }

    #[test]
    fn keybindings_editor_edits_draft_without_changing_active_bindings() {
        let mut sources = Sources::default();
        sources.screen = Screen::HelpOverlay { scroll: 0 };
        open_keybindings_editor(&mut sources);
        let Screen::KeybindingsEditor(state) = &mut sources.screen else {
            panic!()
        };
        state.focus_right = true;
        let ctx = KeyContext::ALL[state.selected_context];
        let action = state.draft.sorted_actions(ctx)[0];
        keybindings_editor_select(&mut sources, action);
        keybindings_editor_bind(&mut sources, KeyChord::char('v'));
        let Screen::KeybindingsEditor(state) = &sources.screen else {
            panic!()
        };
        assert_eq!(state.draft.keys_for(ctx, action), vec![KeyChord::char('v')]);
        assert_ne!(
            sources.keybindings.keys_for(ctx, action),
            vec![KeyChord::char('v')]
        );
    }

    #[test]
    fn closing_editor_discards_unsaved_draft() {
        let mut sources = Sources::default();
        let original = sources.keybindings.clone();
        sources.screen = Screen::HelpOverlay { scroll: 7 };
        open_keybindings_editor(&mut sources);
        if let Screen::KeybindingsEditor(state) = &mut sources.screen {
            state
                .draft
                .replace(KeyContext::Global, Action::PlayPause, KeyChord::char('p'));
        }
        close_keybindings_editor(&mut sources);
        assert_eq!(sources.keybindings, original);
        assert!(matches!(sources.screen, Screen::HelpOverlay { scroll: 7 }));
    }

    #[test]
    fn editor_does_not_open_before_keybindings_finish_loading() {
        let mut sources = Sources::default();
        sources.screen = Screen::HelpOverlay { scroll: 4 };
        sources.persist.loads_in_flight.insert(LoadKey::Keybindings);
        open_keybindings_editor(&mut sources);
        assert!(matches!(sources.screen, Screen::HelpOverlay { scroll: 4 }));
        assert_eq!(
            sources.toast.message.as_deref(),
            Some("Keybindings are still loading")
        );
    }
}

// ─── Dispatcher (kept for FFI / tests that constructed it directly) ─

pub struct Dispatcher<'a> {
    pub sources: &'a mut Sources,
    pub drivers: &'a Drivers,
}
