//! Playlist-tracks (middle-pane PlaylistSongs body) consumer.

use std::cell::Cell;

use mkpclient_runtime::views::{
    ColumnWidths, PlaylistTrackRow, PlaylistTracksBodyModel, PlaylistTracksState,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use super::{dim_style, list_state, pad_or_truncate, row_style_combined};

pub fn draw(
    frame: &mut Frame,
    body_area: Rect,
    model: &PlaylistTracksBodyModel,
    widths: &ColumnWidths,
    spinner_glyph: char,
    middle_offset: &Cell<usize>,
) {
    match &model.state {
        PlaylistTracksState::Empty => {
            // Legacy parity: no playlist viewed → leave the body
            // blank.
        }
        PlaylistTracksState::Loading => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{spinner_glyph} Loading\u{2026}"),
                    dim_style(),
                ))),
                body_area,
            );
        }
        PlaylistTracksState::Tracks { rows } => {
            let items: Vec<ListItem> = rows
                .iter()
                .enumerate()
                .map(|(row_i, row)| match row {
                    PlaylistTrackRow::Pending => {
                        ListItem::new(Line::from(Span::styled("…", dim_style())))
                    }
                    PlaylistTrackRow::PendingAdd => ListItem::new(Line::from(Span::styled(
                        format!("{spinner_glyph} Adding\u{2026}"),
                        dim_style(),
                    ))),
                    PlaylistTrackRow::Song {
                        title,
                        artist,
                        album,
                        duration_str,
                        is_multi_selected,
                        ..
                    } => {
                        let is_cursor = row_i == model.selected_filtered && model.focused;
                        let row_style = row_style_combined(
                            row_i,
                            model.selected_filtered,
                            model.focused,
                            // Playlist tracks don't carry the green
                            // now-playing marker — that lives only
                            // in the Queue column.
                            false,
                            model.in_selection,
                        );
                        // Magenta `❯ ` prefix for non-cursor multi-
                        // selected rows; widths shrink by 1 each on
                        // the variable-width columns.
                        let with_prefix = *is_multi_selected && !is_cursor;
                        let (tw, aw) = if with_prefix {
                            (
                                widths.title.saturating_sub(1),
                                widths.artist.saturating_sub(1),
                            )
                        } else {
                            (widths.title, widths.artist)
                        };
                        let dim_col = if is_cursor {
                            row_style
                        } else {
                            row_style.fg(Color::DarkGray)
                        };
                        // Time column: cyan on non-cursor rows; on the
                        // cursor row, fall back to the cursor style
                        // (black-on-yellow) so the time stays legible
                        // against the yellow band.
                        let cyan_col = if is_cursor {
                            row_style
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
                            Span::styled(pad_or_truncate(title, tw), row_style),
                            Span::styled(" ", row_style),
                            Span::styled(pad_or_truncate(artist, aw), dim_col),
                            Span::styled(" ", row_style),
                            Span::styled(pad_or_truncate(album, widths.album), dim_col),
                            Span::styled(
                                format!("{:>w$}", duration_str, w = widths.time),
                                cyan_col,
                            ),
                        ]);
                        ListItem::new(Line::from(spans))
                    }
                })
                .collect();
            let mut state = list_state(Some(model.selected_filtered), middle_offset);
            frame.render_stateful_widget(List::new(items), body_area, &mut state);
            middle_offset.set(state.offset());
        }
    }
}
