//! Ratatui rendering — three-column layout matching the legacy TUI:
//!
//! ```text
//!  ┌──────────────┬────────────────────────────────────┬──────────┐
//!  │ {server}     │ Playlist                           │ Queue    │
//!  │ Search…      │ Title    Artist   Album    Time    │ …        │
//!  │ <playlists…> │ <tracks…>                          │          │
//!  │                                           {total} │  {total} │
//!  ├──────────────┴────────────────────────────────────┴──────────┤
//!  │ Now Playing                                                  │
//!  │ <title>                                                      │
//!  │ <artist · album>                      ▶ mm:ss / mm:ss        │
//!  └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! Pre-connect / pairing states reuse most of the body area with
//! different content.

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
mod now_playing;
mod pairing_modal;
mod playlist_action;
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

use mkpclient_runtime::Runtime;
use mkpclient_state_link::LinkPhase;
use mkpclient_state_pairing::PairingPhase;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListState, Padding, Paragraph};
use ratatui::Frame;
use std::cell::Cell;

use mkpclient_state_ui_cursor::ColumnFocus;
use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_screen::Screen;
use mkpclient_state_ui_selection::SelectionContext;

use crate::app::AppState;

// ─── colours (verbatim from legacy `nav/mod.rs:30-42`) ──────────────

/// Cursor row when the pane is focused — solid yellow background
/// with black foreground, matching legacy STYLE_SELECTED. Overrides
/// any underlying colour (including the green "now playing" marker)
/// so the row reads as one unbroken highlighted block.
pub(super) fn cursor_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}

/// Cursor when selection mode is active — pink (legacy
/// STYLE_SELECTED_SEL) so the user can tell at a glance they're in
/// multi-select mode.
pub(super) fn cursor_select_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::LightMagenta)
}

pub(super) fn pane_cursor_style(in_selection: bool) -> Style {
    if in_selection {
        cursor_select_style()
    } else {
        cursor_style()
    }
}

/// Selection-mode accent for borders + the bottom Selection bar.
/// Legacy uses `Color::Magenta` (Idx 5) for borders specifically —
/// the LightMagenta hue is reserved for the cursor row and the
/// `❯ ` multi-select prefix.
pub(super) fn selection_accent() -> Style {
    Style::default().fg(Color::Magenta)
}

/// Cursor row when the pane is *unfocused* — legacy shows no
/// visible highlight in this case. Use plain default so the row
/// keeps its natural appearance.
pub(super) fn cursor_dim_style() -> Style {
    Style::default()
}

/// Currently-playing row / track.
pub(super) fn current_style() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

/// Section title, focused pane.
pub(super) fn title_focused() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// Section title, unfocused pane.
pub(super) fn title_unfocused() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Column headers, hint text, placeholders.
pub(super) fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Braille spinner glyphs lifted from legacy `nav/mod.rs:44`.
const SPINNER: &[char] = &['⠤', '⠶', '⠿', '⠿', '⠶', '⠤'];

pub fn spinner(tick: u32) -> char {
    SPINNER[(tick as usize) % SPINNER.len()]
}

/// Legacy parity: pad with spaces to `width`, or truncate with a
/// trailing `…` if too long. Mirrors `mkp2/mkp/src/ui/format.rs::pad_or_truncate`.
pub(super) fn pad_or_truncate(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let sw = s.width();
    if sw > width {
        if width <= 1 {
            return ".".repeat(width);
        }
        let mut w = 0usize;
        let truncated: String = s
            .chars()
            .take_while(|c| {
                w += UnicodeWidthChar::width(*c).unwrap_or(0);
                w < width
            })
            .collect();
        let pad = width.saturating_sub(truncated.width() + 1);
        format!("{}…{}", truncated, " ".repeat(pad))
    } else {
        let pad = width.saturating_sub(sw);
        format!("{}{}", s, " ".repeat(pad))
    }
}

/// Build a `ListState` that carries the previous frame's scroll
/// offset so ratatui doesn't re-anchor the viewport on every cursor
/// move. The caller writes the (possibly adjusted) offset back via
/// `cell.set(state.offset())` after rendering. Without this,
/// moving the cursor up from a scrolled-down position would scroll
/// the viewport on every step instead of waiting for the cursor to
/// reach the top edge.
pub(super) fn list_state(selected: Option<usize>, offset_cell: &Cell<usize>) -> ListState {
    ListState::default()
        .with_offset(offset_cell.get())
        .with_selected(selected)
}

pub(super) fn styled_block(title: &str, focused: bool) -> Block<'_> {
    let style = if focused {
        title_focused()
    } else {
        title_unfocused()
    };
    Block::default()
        .title(Span::styled(format!(" {title} "), style))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
}

/// Like `styled_block` but takes a span list, so callers can mark
/// up a single character (e.g. underline the 'u' in "Queue").
pub(super) fn styled_block_spans<'a>(spans: Vec<Span<'a>>) -> Block<'a> {
    Block::default()
        .title(Line::from(spans))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
}

// ─── entry point ────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &AppState, rt: &Runtime) {
    let area = frame.area();

    if rt.sources.pairing.phase == PairingPhase::AwaitingConfirmation {
        let model = mkpclient_runtime::views::pairing_modal_model(
            mkpclient_runtime::views::PairingModalInput::new(&rt.sources.pairing),
        );
        pairing_modal::draw(frame, area, &model);
        return;
    }

    // A dropped link no longer means the full-screen server list. When
    // the lost-server modal is up the runtime is redialling behind it,
    // so the user stays in the layout they were working in rather than
    // being thrown back to server selection. The columns behind the
    // modal are empty — the server-derived rows are dropped on close
    // and rebuilt on reconnect. The pre-connect screen is for genuinely
    // having nowhere to be: first run, or after giving up on a server.
    let reconnecting = matches!(rt.sources.screen, Screen::ServerLostModal { .. });
    if rt.sources.link.phase != LinkPhase::Connected && !reconnecting {
        draw_pre_connect(frame, area, app, rt);
        return;
    }

    draw_main(frame, area, app, rt);

    match &rt.sources.screen {
        Screen::NowPlaying => {}
        Screen::SearchInput(state) => {
            let model = mkpclient_runtime::views::search_input_model(
                mkpclient_runtime::views::SearchInputModalInput::new(state),
            );
            search_input_modal::draw(frame, area, &model);
        }
        Screen::ActionModal(state) => {
            let model = mkpclient_runtime::views::action_modal_model(
                mkpclient_runtime::views::ActionModalInput::new(state, &rt.sources.keybindings),
            );
            action_modal::draw(frame, area, &model);
        }
        Screen::FilterInput(state) => {
            let model = mkpclient_runtime::views::filter_input_model(
                mkpclient_runtime::views::FilterStateInput::new(state),
            );
            filter_input::draw(frame, area, &model);
        }
        Screen::HelpOverlay { scroll } => {
            let model = mkpclient_runtime::views::help_overlay_model(
                mkpclient_runtime::views::HelpOverlayInput::new(*scroll, &rt.sources.keybindings),
            );
            help_overlay::draw(frame, area, &model);
        }
        Screen::KeybindingsEditor(state) => {
            let model = mkpclient_runtime::views::keybindings_editor_model(
                mkpclient_runtime::views::KeybindingsEditorInput::new(state),
            );
            keybindings_editor::draw(frame, area, &model);
        }
        Screen::CreatePlaylist { input, .. } => {
            let model = mkpclient_runtime::views::input_modal_model(
                mkpclient_runtime::views::InputModalInput::new(
                    mkpclient_runtime::views::InputModalKind::CreatePlaylist,
                    input,
                ),
            );
            input_modal::draw(frame, area, &model);
        }
        Screen::RenamePlaylist { input, .. } => {
            let model = mkpclient_runtime::views::input_modal_model(
                mkpclient_runtime::views::InputModalInput::new(
                    mkpclient_runtime::views::InputModalKind::RenamePlaylist,
                    input,
                ),
            );
            input_modal::draw(frame, area, &model);
        }
        Screen::PlaylistAction { selected, .. } => {
            let model = mkpclient_runtime::views::playlist_action_modal_model(
                mkpclient_runtime::views::PlaylistActionModalInput::new(
                    *selected,
                    &rt.sources.keybindings,
                ),
            );
            playlist_action::draw(frame, area, &model);
        }
        Screen::ConfirmDeletePlaylist { name, input, .. } => {
            let model = mkpclient_runtime::views::confirm_delete_playlist_model(
                mkpclient_runtime::views::ConfirmDeletePlaylistInput { name, input },
            );
            confirm_delete::draw(frame, area, &model);
        }
        Screen::PlaylistPicker { item, selected: _ } => {
            // Legacy behaviour: the left column itself becomes the
            // picker — the cursor highlight tracks the picker's
            // own `selected`, drawn in `draw_playlists_col` below.
            // We just show a small hint so the user knows they're
            // in pick mode.
            let model = mkpclient_runtime::views::playlist_picker_hint_model(
                mkpclient_runtime::views::PlaylistPickerHintInput {
                    item_label: &item.label,
                },
            );
            playlist_picker_hint::draw(frame, area, &model);
        }
        Screen::ConfirmRemoveFromPlaylist { song_title, .. } => {
            let model = mkpclient_runtime::views::confirm_remove_model(
                mkpclient_runtime::views::ConfirmRemoveInput { song_title },
            );
            confirm_remove::draw(frame, area, &model);
        }
        Screen::SelectionActionModal { selected } => {
            let model = mkpclient_runtime::views::selection_action_modal_model(
                mkpclient_runtime::views::SelectionActionModalInput::new(
                    rt.sources.selection.context,
                    &rt.sources.history.mode,
                    rt.sources.selection.selected.len(),
                    *selected,
                    &rt.sources.keybindings,
                ),
            );
            selection_action_modal::draw(frame, area, &model);
        }
        Screen::ErrorModal { message } => {
            let model = mkpclient_runtime::views::error_modal_model(
                mkpclient_runtime::views::ErrorModalInput { message },
            );
            error_modal::draw(frame, area, &model);
        }
        Screen::ServerLostModal { server } => {
            let model = mkpclient_runtime::views::server_lost_modal_model(
                mkpclient_runtime::views::ServerLostModalInput { server },
            );
            server_lost_modal::draw(frame, area, &model, spinner(app.tick));
        }
        Screen::ServerPicker { selected } => {
            let model = mkpclient_runtime::views::server_picker_modal_model(
                mkpclient_runtime::views::ServerPickerModalInput::new(
                    &rt.sources.discovery,
                    rt.sources.session.backend_name.as_deref(),
                    *selected,
                ),
            );
            server_picker_modal::draw(frame, area, &model);
        }
    }
}

// ─── connected main view ────────────────────────────────────────────

fn draw_main(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(4)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(55),
            Constraint::Percentage(25),
        ])
        .split(root[0]);

    draw_playlists_col(frame, cols[0], app, rt);
    draw_tracks_col(frame, cols[1], app, rt);
    draw_queue_col(frame, cols[2], app, rt);
    draw_now_playing_bar(frame, root[1], app, rt);
}

fn draw_playlists_col(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    let picker = match &rt.sources.screen {
        Screen::PlaylistPicker { selected, .. } => Some(mkpclient_runtime::views::PickerOverride {
            selected: *selected,
        }),
        _ => None,
    };
    let model = mkpclient_runtime::views::left_column_model(
        mkpclient_runtime::views::PlaylistsInput::new(&rt.sources.playlists),
        mkpclient_runtime::views::PendingPlaylistsInput::new(&rt.sources.pending_playlists),
        mkpclient_runtime::views::PlaylistTracksFocusInput::new(&rt.sources.playlist_tracks),
        mkpclient_runtime::views::ServerLabelInput::new(
            &rt.sources.link,
            &rt.sources.discovery,
            &rt.sources.probes,
        ),
        mkpclient_runtime::views::LeftUiInput {
            backend_name: rt.sources.session.backend_name.as_deref(),
            column_focused: rt.sources.cursor.focus == ColumnFocus::Left,
            left_selected: rt.sources.cursor.left,
            playlist_filter: &rt.sources.filter.playlist,
            picker,
            viewing_active: matches!(rt.sources.history.mode, MiddleMode::PlaylistSongs),
        },
    );
    let spinner_glyph = spinner(app.tick);
    left::draw(frame, area, &model, &app.left_offset, spinner_glyph);
}

fn middle_mode_view(mode: &MiddleMode) -> mkpclient_runtime::views::MiddleModeView {
    use mkpclient_runtime::views::MiddleModeView as V;
    match mode {
        MiddleMode::PlaylistSongs => V::PlaylistSongs,
        MiddleMode::SearchResults {
            search_type, term, ..
        } => V::SearchResults {
            search_type: (*search_type).into(),
            term: std::sync::Arc::from(term.as_str()),
        },
        MiddleMode::AlbumDetail { awaiting_seq, .. } => V::AlbumDetail {
            awaiting_seq: *awaiting_seq,
        },
        MiddleMode::ArtistDetail { .. } => V::ArtistDetail,
    }
}

fn draw_tracks_col(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    let focused = rt.sources.cursor.focus == ColumnFocus::Middle;
    let album_total_secs = match &rt.sources.history.mode {
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            mkpclient_runtime::views::album_detail_total_secs(*awaiting_seq, &rt.sources.responses)
        }
        _ => 0.0,
    };
    let header_model = mkpclient_runtime::views::middle_header_model(
        middle_mode_view(&rt.sources.history.mode),
        mkpclient_runtime::views::SearchCountsInput::new(&rt.sources.search),
        mkpclient_runtime::views::PlaylistTracksDurationInput::new(&rt.sources.playlist_tracks),
        album_total_secs,
        mkpclient_runtime::views::MiddleHeaderUiInput {
            focused,
            in_selection: rt.sources.selection.context == Some(SelectionContext::Middle),
            middle_filter_empty: rt.sources.filter.middle.is_empty(),
            history_back_count: rt.sources.history.back.len(),
            history_fwd_count: rt.sources.history.forward.len(),
        },
    );

    let mut block = styled_block(&header_model.title, focused);
    if header_model.in_selection {
        block = block.border_style(selection_accent());
    }
    if header_model.can_back || header_model.can_fwd {
        let back = if header_model.can_back {
            "\u{25C0}"
        } else {
            " "
        };
        let fwd = if header_model.can_fwd {
            "\u{25B6}"
        } else {
            " "
        };
        block = block.title_top(
            Line::from(Span::styled(format!(" {back} {fwd} "), dim_style())).left_aligned(),
        );
    }
    if header_model.show_unfilter_hint {
        block = block.title_bottom(
            Line::from(Span::styled(" Shift-F unfilter ", dim_style())).left_aligned(),
        );
    }
    if let Some(total) = header_model.total_duration.as_deref() {
        block = block.title_bottom(
            Line::from(Span::styled(format!(" {total} "), dim_style())).right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let widths = mkpclient_runtime::views::column_widths(inner.width);
    let title_w = widths.title;
    let artist_w = widths.artist;
    let album_w = widths.album;
    let time_w = widths.time;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
        ])
        .split(inner);

    if header_model.render_track_header {
        let header_style = dim_style().add_modifier(Modifier::BOLD);
        let header = Line::from(vec![
            Span::styled(pad_or_truncate("Title", title_w), header_style),
            Span::raw(" "),
            Span::styled(pad_or_truncate("Artist", artist_w), header_style),
            Span::raw(" "),
            Span::styled(pad_or_truncate("Album", album_w), header_style),
            Span::styled(format!("{:>w$}", "Time", w = time_w), header_style),
        ]);
        frame.render_widget(Paragraph::new(header), chunks[0]);
    }
    let body_area = if header_model.render_track_header {
        chunks[1]
    } else {
        inner
    };

    match &rt.sources.history.mode {
        MiddleMode::PlaylistSongs => {
            draw_playlist_tracks_body(frame, body_area, app, rt, focused, &widths);
        }
        MiddleMode::SearchResults { .. } => {
            draw_search_results_body(frame, body_area, app, rt, focused, &widths);
        }
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            draw_album_detail_body(frame, body_area, app, rt, focused, *awaiting_seq);
        }
        MiddleMode::ArtistDetail { awaiting_seq, .. } => {
            draw_artist_detail_body(frame, body_area, app, rt, focused, *awaiting_seq, &widths);
        }
    }
}

fn draw_artist_detail_body(
    frame: &mut Frame,
    body_area: Rect,
    app: &AppState,
    rt: &Runtime,
    focused: bool,
    awaiting_seq: Option<u64>,
    widths: &mkpclient_runtime::views::ColumnWidths,
) {
    let model = mkpclient_runtime::views::artist_detail_body_model(
        mkpclient_runtime::views::ArtistDetailResponseInput::new(
            awaiting_seq,
            &rt.sources.responses,
        ),
        mkpclient_runtime::views::ArtistDetailExtrasInput::new(&rt.sources.artist_extras),
        rt.sources.cursor.middle,
        focused,
        body_area.width,
        widths.time,
    );
    artist_detail::draw(
        frame,
        body_area,
        &model,
        spinner(app.tick),
        &app.middle_offset,
    );
}

fn draw_album_detail_body(
    frame: &mut Frame,
    body_area: Rect,
    app: &AppState,
    rt: &Runtime,
    focused: bool,
    awaiting_seq: Option<u64>,
) {
    let model = mkpclient_runtime::views::album_detail_body_model(
        mkpclient_runtime::views::AlbumDetailResponseInput::new(
            awaiting_seq,
            &rt.sources.responses,
        ),
        &rt.sources.filter.middle,
        rt.sources.cursor.middle,
        focused,
        body_area.width,
        rt.sources.selection.context == Some(SelectionContext::Middle),
        &rt.sources.selection.selected,
    );
    album_detail::draw(
        frame,
        body_area,
        &model,
        spinner(app.tick),
        &app.middle_offset,
    );
}

fn draw_playlist_tracks_body(
    frame: &mut Frame,
    body_area: Rect,
    app: &AppState,
    rt: &Runtime,
    focused: bool,
    widths: &mkpclient_runtime::views::ColumnWidths,
) {
    let model = mkpclient_runtime::views::playlist_tracks_body_model(
        mkpclient_runtime::views::PlaylistTracksInput::new(&rt.sources.playlist_tracks),
        mkpclient_runtime::views::PlaylistTracksPendingInput::new(&rt.sources.pending_playlists),
        &rt.sources.filter.middle,
        rt.sources.selection.context == Some(SelectionContext::Middle),
        &rt.sources.selection.selected,
        rt.sources.cursor.middle,
        focused,
    );
    playlist_tracks::draw(
        frame,
        body_area,
        &model,
        widths,
        spinner(app.tick),
        &app.middle_offset,
    );
}

fn draw_search_results_body(
    frame: &mut Frame,
    full_area: Rect,
    app: &AppState,
    rt: &Runtime,
    focused: bool,
    widths: &mkpclient_runtime::views::ColumnWidths,
) {
    let model = mkpclient_runtime::views::search_results_body_model(
        mkpclient_runtime::views::SearchResultsInput::new(&rt.sources.search),
        &rt.sources.filter.middle,
        rt.sources.cursor.middle,
        focused,
        rt.sources.selection.context == Some(SelectionContext::Middle),
        &rt.sources.selection.selected,
    );
    search_results::draw(
        frame,
        full_area,
        &model,
        widths,
        spinner(app.tick),
        &app.middle_offset,
    );
}

pub(super) fn row_style_for(row: usize, selected: usize, focused: bool) -> Style {
    row_style_for_pane(row, selected, focused, false)
}

fn row_style_for_pane(row: usize, selected: usize, focused: bool, in_selection: bool) -> Style {
    if row == selected {
        if focused {
            pane_cursor_style(in_selection)
        } else {
            cursor_dim_style()
        }
    } else {
        Style::default()
    }
}

/// Combined row style: accounts for cursor, currently-playing
/// marker, and selection mode. Multi-select membership is rendered
/// by the painters as a leading "❯ " prefix (legacy parity), not
/// via row-level bg, so it isn't a parameter here.
pub(super) fn row_style_combined(
    row: usize,
    cursor: usize,
    focused: bool,
    is_current: bool,
    in_selection: bool,
) -> Style {
    let base = if is_current {
        current_style()
    } else {
        Style::default()
    };
    if row == cursor && focused {
        pane_cursor_style(in_selection)
    } else {
        base
    }
}

fn draw_queue_col(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    let model = mkpclient_runtime::views::queue_column_model(
        mkpclient_runtime::views::QueueInput::new(&rt.sources.queue),
        mkpclient_runtime::views::ServerPositionInput::new(&rt.sources.server),
        rt.sources.cursor.queue,
        &rt.sources.filter.queue,
        rt.sources.selection.context == Some(SelectionContext::Queue),
        &rt.sources.selection.selected,
        rt.sources.cursor.focus == ColumnFocus::Queue,
    );
    queue::draw(frame, area, &model, &app.queue_offset);
}

fn draw_selection_bar(frame: &mut Frame, area: Rect, _app: &AppState, rt: &Runtime) {
    let context = match rt.sources.selection.context {
        Some(SelectionContext::Middle) => mkpclient_runtime::views::SelectionBarContext::Middle,
        Some(SelectionContext::Queue) => mkpclient_runtime::views::SelectionBarContext::Queue,
        None => return, // caller already short-circuits, but be safe.
    };
    let model = mkpclient_runtime::views::selection_bar_model(
        context,
        &rt.sources.selection.selected,
        mkpclient_runtime::views::SelectionBarSongsInput::new(
            &rt.sources.queue,
            &rt.sources.playlist_tracks,
        ),
        matches!(rt.sources.history.mode, MiddleMode::PlaylistSongs),
    );
    selection_bar::draw(frame, area, &model);
}

fn draw_now_playing_bar(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    // Legacy parity: when in selection mode, the bottom bar
    // becomes a "Selection" hint instead of the Now Playing meta.
    if rt.sources.selection.context.is_some() {
        draw_selection_bar(frame, area, app, rt);
        return;
    }

    let model = mkpclient_runtime::views::now_playing_model(
        mkpclient_runtime::views::ServerNowPlayingInput::new(&rt.sources.server),
        mkpclient_runtime::views::UiPreviewInput::new(&rt.sources.preview),
        mkpclient_runtime::views::ActivityInput::new(&rt.sources.activity),
        mkpclient_runtime::views::PeerIdInput::new(&rt.peer),
    );
    now_playing::draw(
        frame,
        area,
        &model,
        spinner(app.tick),
        rt.sources.toast.message.as_deref(),
    );
}

// ─── pre-connect (server picker, pairing) ──────────────────────────

fn draw_pre_connect(frame: &mut Frame, area: Rect, app: &AppState, rt: &Runtime) {
    let model = mkpclient_runtime::views::pre_connect_model(
        mkpclient_runtime::views::PreConnectInput::new(
            &rt.sources.discovery,
            &rt.sources.link,
            &rt.sources.probes,
            &rt.sources.credentials,
        ),
        rt.sources.session.preferred_server.as_deref(),
        rt.sources.session.lost_server.as_deref(),
        rt.sources.session.auto_connect,
        rt.sources.cursor.server_picker,
    );
    pre_connect::draw(frame, area, &model, spinner(app.tick));
}

// ─── modal renderers ───────────────────────────────────────────────

// `draw_filter_input_modal` lives in `render/filter_input.rs`.
// `draw_action_modal` lives in `render/action_modal.rs`.
// `draw_search_input_modal` lives in `render/search_input_modal.rs`.
// `draw_selection_action_modal` lives in `render/selection_action_modal.rs`.
// `draw_playlist_picker_hint` lives in `render/playlist_picker_hint.rs`.
// `draw_playlist_picker` lives in `render/playlist_picker.rs`.

// `format_duration` lives in `mkpclient_runtime::views::util` —
// view models pre-format durations, the renderer no longer needs
// its own copy.
