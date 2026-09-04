//! Artist-detail (middle-pane ArtistDetail body) consumer.
//!
//! Mirrors legacy `mkp2/mkp/src/nav/player/middle.rs::draw_artist_detail`:
//!
//! ```text
//!   Bruce Hornsby           ← bold, info paragraph (above the list)
//!
//!   The American pianist…   ← dim, wrapped to body width
//!
//!   Top Songs               ← cyan-bold section header
//!     The Way It Is                                     04:55
//!     Mandolin Rain                                     05:14
//!
//!   Top Albums
//!     2010   Camp Meeting                                  9
//!
//!   Discography
//!     2019   Absolute Zero                                12
//!
//!   Related Artists
//!     Don Henley • Steve Winwood • Toto                ← " • " flow
//! ```
//!
//! - The artist name + editorial notes live in an info `Paragraph`
//!   above the list (not list rows).
//! - Section headers ("Top Songs" / "Top Albums" / "Discography" /
//!   "Related Artists") are decorative — cursor never lands on
//!   them. The memo's `item_visual_indices` skips them.
//! - Related Artists are bullet-flowed: each visual line packs
//!   multiple names. The memo emits one `SimilarFlow` row per
//!   visual line; the painter renders the entries with a `" • "`
//!   dim separator, highlighting the entry whose `cursor_stop`
//!   matches the current `selected_item`.

use std::cell::Cell;

use mkpclient_runtime::views::{
    ArtistDetailBodyModel, ArtistDetailLoaded, ArtistDetailRow, ArtistDetailState,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use super::{cursor_style, dim_style, list_state, pad_or_truncate};

pub fn draw(
    frame: &mut Frame,
    body_area: Rect,
    model: &ArtistDetailBodyModel,
    spinner_glyph: char,
    middle_offset: &Cell<usize>,
) {
    let loaded = match &model.state {
        ArtistDetailState::Loading => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{spinner_glyph} Loading\u{2026}"),
                    dim_style(),
                ))),
                body_area,
            );
            return;
        }
        ArtistDetailState::Loaded(l) => l,
    };

    if body_area.height < 2 {
        return;
    }

    // Legacy info_height arithmetic: 1 (artist name) + (1 blank +
    // N notes lines) when notes present + 1 trailing blank.
    let mut info_height: u16 = 1;
    let has_notes = !loaded.info.notes_lines.is_empty();
    if has_notes {
        info_height += 1; // blank line before notes
        info_height += loaded.info.notes_lines.len() as u16;
    }
    info_height += 1; // trailing blank separator

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(info_height), Constraint::Min(0)])
        .split(body_area);
    let info_area = chunks[0];
    let list_area = chunks[1];

    frame.render_widget(Paragraph::new(build_info_lines(loaded)), info_area);

    let items: Vec<ListItem> = loaded
        .rows
        .iter()
        .map(|row| build_row_item(row, model, loaded, spinner_glyph))
        .collect();

    let visual = loaded.item_visual_indices.get(model.selected_item).copied();
    let mut state = list_state(visual, middle_offset);
    frame.render_stateful_widget(List::new(items), list_area, &mut state);
    middle_offset.set(state.offset());
}

fn build_info_lines(loaded: &ArtistDetailLoaded) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        loaded.info.name.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if !loaded.info.notes_lines.is_empty() {
        lines.push(Line::from(""));
        for note in loaded.info.notes_lines.iter() {
            lines.push(Line::from(Span::styled(note.to_string(), dim_style())));
        }
    }
    lines.push(Line::from(""));
    lines
}

fn build_row_item(
    row: &ArtistDetailRow,
    model: &ArtistDetailBodyModel,
    loaded: &ArtistDetailLoaded,
    spinner_glyph: char,
) -> ListItem<'static> {
    let section_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    match row {
        ArtistDetailRow::Blank => ListItem::new(Line::from("")),
        ArtistDetailRow::SectionHeader(label) => {
            ListItem::new(Line::from(Span::styled(label.to_string(), section_style)))
        }
        ArtistDetailRow::SongItem {
            cursor_stop,
            title,
            album,
            duration_str,
        } => {
            let is_cursor = model.focused && *cursor_stop as usize == model.selected_item;
            let style = if is_cursor {
                cursor_style()
            } else {
                Style::default()
            };
            let dim_col = if is_cursor {
                style
            } else {
                style.fg(Color::DarkGray)
            };
            // Time column: cyan on non-cursor rows; on the cursor
            // row, the cursor band's black-on-yellow takes over so
            // it stays legible.
            let cyan_col = if is_cursor {
                style
            } else {
                Style::default().fg(Color::Cyan)
            };
            ListItem::new(Line::from(vec![
                Span::styled(pad_or_truncate(title, loaded.song_title_w), style),
                Span::styled(" ", style),
                Span::styled(pad_or_truncate(album, loaded.song_album_w), dim_col),
                Span::styled(
                    format!("{:>w$}", duration_str, w = loaded.song_time_w),
                    cyan_col,
                ),
            ]))
        }
        ArtistDetailRow::AlbumItem {
            cursor_stop,
            year,
            name,
            track_count,
        } => {
            let is_cursor = model.focused && *cursor_stop as usize == model.selected_item;
            let style = if is_cursor {
                cursor_style()
            } else {
                Style::default()
            };
            let dim_col = if is_cursor {
                style
            } else {
                style.fg(Color::DarkGray)
            };
            let year_padded = pad_or_truncate(year, loaded.album_year_w);
            let name_padded = pad_or_truncate(name, loaded.album_name_w);
            let tracks = format!("{track_count}");
            ListItem::new(Line::from(vec![
                Span::styled(year_padded, dim_col),
                Span::styled(name_padded, style),
                Span::styled(
                    format!("{:>w$}", tracks, w = loaded.album_tracks_w),
                    dim_col,
                ),
            ]))
        }
        ArtistDetailRow::SimilarFlow { artists } => {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (pos, entry) in artists.iter().enumerate() {
                if pos > 0 {
                    spans.push(Span::styled(" \u{2022} ", dim_style()));
                }
                let is_cursor = model.focused && entry.cursor_stop as usize == model.selected_item;
                let style = if is_cursor {
                    cursor_style()
                } else {
                    dim_style()
                };
                spans.push(Span::styled(entry.name.to_string(), style));
            }
            ListItem::new(Line::from(spans))
        }
        ArtistDetailRow::SimilarLoading => ListItem::new(Line::from(Span::styled(
            format!("{spinner_glyph} Loading\u{2026}"),
            dim_style(),
        ))),
    }
}
