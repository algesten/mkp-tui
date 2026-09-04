//! Album-detail (middle-pane AlbumDetail body) consumer.
//!
//! Mirrors legacy `mkp2/mkp/src/nav/player/middle.rs::draw_album_detail`:
//!
//! ```text
//!     Abbey Road              ← BOLD
//!     The Beatles             ← cyan
//!     2019                    ← dim
//!
//!     The Beatles' grand…     ← dim, wrapped to body width
//!
//!     Title                                            Time   ← dim+bold
//!  1  Come Together (2019 Mix)                        04:20
//!  …
//!
//!     UMC (Universal …)        ← dim
//!     ℗ 2019 …                 ← dim
//! ```
//!
//! Album rows do NOT use the shared `Title / Artist / Album / Time`
//! layout — albums repeat the same artist + album on every row, so
//! the legacy renderer drops those columns and prefixes the title
//! with a numeric track number instead. The shared track header is
//! suppressed by `middle_header_model` for `MiddleMode::AlbumDetail`.

use std::cell::Cell;

use mkpclient_runtime::views::{AlbumDetailBodyModel, AlbumDetailState, AlbumHeader};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use super::{dim_style, list_state, pad_or_truncate, pane_cursor_style};

pub fn draw(
    frame: &mut Frame,
    body_area: Rect,
    model: &AlbumDetailBodyModel,
    spinner_glyph: char,
    middle_offset: &Cell<usize>,
) {
    match &model.state {
        AlbumDetailState::Loading => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{spinner_glyph} Loading\u{2026}"),
                    dim_style(),
                ))),
                body_area,
            );
        }
        AlbumDetailState::Tracks {
            header,
            rows,
            use_hours,
        } => {
            if body_area.height < 2 {
                return;
            }

            // Mirror legacy info_height arithmetic exactly. Notes
            // are pre-wrapped by the memo, so the painter just
            // counts and renders.
            let mut info_height: u16 = 2; // album name + artist
            if header.year.is_some() {
                info_height += 1;
            }
            let has_notes = !header.notes_lines.is_empty();
            if has_notes {
                info_height += 1; // blank line before notes
                info_height += header.notes_lines.len() as u16;
            }
            info_height += 1; // blank separator before track list

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(info_height),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(body_area);
            let info_area = chunks[0];
            let header_area = chunks[1];
            let list_area = chunks[2];

            frame.render_widget(Paragraph::new(build_info_lines(header)), info_area);

            frame.render_widget(
                Paragraph::new(album_track_header_line(header_area.width, *use_hours)),
                header_area,
            );

            let mut items: Vec<ListItem> = rows
                .iter()
                .enumerate()
                .map(|(row_i, row)| {
                    let is_cursor = row_i == model.selected_filtered && model.focused;
                    let style = if is_cursor {
                        pane_cursor_style(model.in_selection)
                    } else {
                        Style::default()
                    };
                    album_track_list_item(
                        row.track_number,
                        &row.title,
                        &row.duration_str,
                        list_area.width,
                        *use_hours,
                        style,
                        is_cursor,
                        row.is_multi_selected,
                    )
                })
                .collect();

            // Legacy parity: when label or copyright is present,
            // append a blank row + dim label + dim copyright after
            // the tracks. These are decorative and never selectable.
            if header.record_label.is_some() || header.copyright.is_some() {
                let pad = "   ";
                items.push(ListItem::new(""));
                if let Some(label) = header.record_label.as_deref() {
                    items.push(ListItem::new(Line::from(vec![
                        Span::raw(pad),
                        Span::styled(label.to_string(), dim_style()),
                    ])));
                }
                if let Some(cr) = header.copyright.as_deref() {
                    items.push(ListItem::new(Line::from(vec![
                        Span::raw(pad),
                        Span::styled(cr.to_string(), dim_style()),
                    ])));
                }
            }

            let mut state = list_state(Some(model.selected_filtered), middle_offset);
            frame.render_stateful_widget(List::new(items), list_area, &mut state);
            middle_offset.set(state.offset());
        }
    }
}

fn build_info_lines(header: &AlbumHeader) -> Vec<Line<'static>> {
    let pad = "   ";
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::raw(pad),
        Span::styled(
            header.name.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw(pad),
        Span::styled(header.artist.to_string(), Style::default().fg(Color::Cyan)),
    ]));
    if let Some(year) = header.year.as_deref() {
        lines.push(Line::from(vec![
            Span::raw(pad),
            Span::styled(year.to_string(), dim_style()),
        ]));
    }
    if !header.notes_lines.is_empty() {
        lines.push(Line::from(""));
        for line in header.notes_lines.iter() {
            lines.push(Line::from(vec![
                Span::raw(pad),
                Span::styled(line.to_string(), dim_style()),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines
}

fn album_track_header_line(width: u16, use_hours: bool) -> Line<'static> {
    let w = width as usize;
    let dur_w = if use_hours { 9 } else { 6 };
    let num_w = 3;
    let title_w = w.saturating_sub(dur_w + num_w);

    let s = dim_style().add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(" ".repeat(num_w), s),
        Span::styled(pad_or_truncate("Title", title_w), s),
        Span::styled(format!("{:>w$}", "Time", w = dur_w), s),
    ])
}

fn album_track_list_item(
    track_number: Option<u32>,
    title: &str,
    duration_str: &str,
    width: u16,
    use_hours: bool,
    row_style: Style,
    is_cursor: bool,
    is_multi_selected: bool,
) -> ListItem<'static> {
    let prefix_w: usize = if is_multi_selected && !is_cursor {
        2
    } else {
        0
    };
    let w = (width as usize).saturating_sub(prefix_w);
    let dur_w = if use_hours { 9 } else { 6 };
    let num_w = 3;
    let title_w = w.saturating_sub(dur_w + num_w);

    let dim_col = if is_cursor {
        row_style
    } else {
        row_style.fg(Color::DarkGray)
    };
    // Time column: cyan when non-cursor; on the cursor row, the
    // cursor band's black-on-yellow takes over so it stays legible.
    let cyan_col = if is_cursor {
        row_style
    } else {
        Style::default().fg(Color::Cyan)
    };

    let num = match track_number {
        Some(n) => format!("{:>2} ", n),
        None => "   ".to_string(),
    };

    let mut spans: Vec<Span> = Vec::new();
    if prefix_w > 0 {
        spans.push(Span::styled(
            "\u{276F} ",
            Style::default().fg(Color::LightMagenta),
        ));
    }
    spans.extend([
        Span::styled(num, dim_col),
        Span::styled(pad_or_truncate(title, title_w), row_style),
        Span::styled(format!("{:>w$}", duration_str, w = dur_w), cyan_col),
    ]);
    ListItem::new(Line::from(spans))
}
