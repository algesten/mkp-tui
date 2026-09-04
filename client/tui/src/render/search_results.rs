//! Search-results (middle-pane SearchResults body) consumer.
//!
//! Each search type owns its own header layout — legacy parity with
//! `mkp2/mkp/src/nav/player/middle.rs::draw_search_results /
//! draw_album_results / draw_artist_results`:
//!
//! - **Song search**: `Title / Artist / Album / Time` header above
//!   the list, rendered only when there are rows. Searching /
//!   NoResults suppresses the header so the body matches legacy's
//!   "blank pane on No results, plain spinner row on Searching".
//! - **Album search**: `Album / Artist / Tracks` header above.
//! - **Artist search**: no header, just a list of names.
//!
//! Searching state renders `{spinner} Searching…` (legacy parity).
//! NoResults state renders nothing — legacy leaves the pane blank;
//! the title bar above already says " No results ".

use std::cell::Cell;

use mkpclient_runtime::views::{ColumnWidths, SearchResultsBodyModel, SearchResultsState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use super::{dim_style, list_state, pad_or_truncate, pane_cursor_style, row_style_for};

pub fn draw(
    frame: &mut Frame,
    full_area: Rect,
    model: &SearchResultsBodyModel,
    widths: &ColumnWidths,
    spinner_glyph: char,
    middle_offset: &Cell<usize>,
) {
    match &model.state {
        SearchResultsState::Searching => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{spinner_glyph} Searching\u{2026}"),
                    dim_style(),
                ))),
                full_area,
            );
        }
        SearchResultsState::NoResults => {
            // Legacy parity: blank middle pane. Title bar already
            // says " No results "; no body text needed.
        }
        SearchResultsState::Songs { rows } => {
            // Song search owns its `Title / Artist / Album / Time`
            // header so it only renders when there are rows.
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(full_area);
            let header_style = dim_style().add_modifier(Modifier::BOLD);
            let header = Line::from(vec![
                Span::styled(pad_or_truncate("Title", widths.title), header_style),
                Span::raw(" "),
                Span::styled(pad_or_truncate("Artist", widths.artist), header_style),
                Span::raw(" "),
                Span::styled(pad_or_truncate("Album", widths.album), header_style),
                Span::styled(format!("{:>w$}", "Time", w = widths.time), header_style),
            ]);
            frame.render_widget(Paragraph::new(header), chunks[0]);

            let items: Vec<ListItem> = rows
                .iter()
                .enumerate()
                .map(|(row_i, row)| {
                    let is_cursor = row_i == model.selected_filtered && model.focused;
                    let style = if is_cursor {
                        pane_cursor_style(model.in_selection)
                    } else {
                        Style::default()
                    };
                    let with_prefix = row.is_multi_selected && !is_cursor;
                    let (tw, aw) = if with_prefix {
                        (
                            widths.title.saturating_sub(1),
                            widths.artist.saturating_sub(1),
                        )
                    } else {
                        (widths.title, widths.artist)
                    };
                    let dim_col = if is_cursor {
                        style
                    } else {
                        style.fg(Color::DarkGray)
                    };
                    let cyan_col = if is_cursor {
                        style
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let mut spans: Vec<Span> = Vec::new();
                    if with_prefix {
                        spans.push(Span::styled(
                            "\u{276F} ",
                            Style::default().fg(Color::LightMagenta),
                        ));
                    }
                    spans.extend([
                        Span::styled(pad_or_truncate(&row.title, tw), style),
                        Span::styled(" ", style),
                        Span::styled(pad_or_truncate(&row.artist, aw), dim_col),
                        Span::styled(" ", style),
                        Span::styled(pad_or_truncate(&row.album, widths.album), dim_col),
                        Span::styled(
                            format!("{:>w$}", row.duration_str, w = widths.time),
                            cyan_col,
                        ),
                    ]);
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut state = list_state(Some(model.selected_filtered), middle_offset);
            frame.render_stateful_widget(List::new(items), chunks[1], &mut state);
            middle_offset.set(state.offset());
        }
        SearchResultsState::Albums { rows } => {
            // Album search: own header — Album (55%) | Artist (45%) | Tracks.
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(full_area);
            let w = full_area.width as usize;
            let tracks_w = 6;
            let remaining = w.saturating_sub(tracks_w + 2);
            let alb_w = remaining * 55 / 100;
            let art_w = remaining.saturating_sub(alb_w);
            let header_style = dim_style().add_modifier(Modifier::BOLD);
            let header = Line::from(vec![
                Span::styled(pad_or_truncate("Album", alb_w), header_style),
                Span::raw(" "),
                Span::styled(pad_or_truncate("Artist", art_w), header_style),
                Span::styled(format!("{:>w$}", "Tracks", w = tracks_w), header_style),
            ]);
            frame.render_widget(Paragraph::new(header), chunks[0]);

            let items: Vec<ListItem> = rows
                .iter()
                .enumerate()
                .map(|(row_i, row)| {
                    let style = row_style_for(row_i, model.selected_filtered, model.focused);
                    let is_cursor = row_i == model.selected_filtered && model.focused;
                    let dim_col = if is_cursor {
                        style
                    } else {
                        style.fg(Color::DarkGray)
                    };
                    // Time column: cyan on non-cursor rows; on the
                    // cursor row, the cursor band's black-on-yellow
                    // takes over so it stays legible.
                    let cyan_col = if is_cursor {
                        style
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let tracks_label = format!("{}", row.track_count);
                    ListItem::new(Line::from(vec![
                        Span::styled(pad_or_truncate(&row.name, alb_w), style),
                        Span::styled(" ", style),
                        Span::styled(pad_or_truncate(&row.artist, art_w), dim_col),
                        Span::styled(format!("{:>w$}", tracks_label, w = tracks_w), cyan_col),
                    ]))
                })
                .collect();
            let mut state = list_state(Some(model.selected_filtered), middle_offset);
            frame.render_stateful_widget(List::new(items), chunks[1], &mut state);
            middle_offset.set(state.offset());
        }
        SearchResultsState::Artists { rows } => {
            // Artist search: no header.
            let items: Vec<ListItem> = rows
                .iter()
                .enumerate()
                .map(|(row_i, row)| {
                    let style = row_style_for(row_i, model.selected_filtered, model.focused);
                    ListItem::new(Line::from(Span::styled(row.name.clone(), style)))
                })
                .collect();
            let mut state = list_state(Some(model.selected_filtered), middle_offset);
            frame.render_stateful_widget(List::new(items), full_area, &mut state);
            middle_offset.set(state.offset());
        }
    }
}
