//! Left-column ("Playlists") consumer.
//!
//! Pure painter over a [`LeftColumnModel`]. The picker overlay
//! quirk is encoded in the model: a row's `is_picker` flag wins
//! over `is_cursor`, and the visual cursor in `list_cursor` already
//! accounts for the picker.

use std::cell::Cell;

use mkpclient_runtime::views::{LeftColumnModel, LeftRow};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Padding};
use ratatui::Frame;

use super::{
    current_style, cursor_dim_style, cursor_style, dim_style, list_state, title_focused,
    title_unfocused,
};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    model: &LeftColumnModel,
    scroll_offset: &Cell<usize>,
    spinner_glyph: char,
) {
    let mut title_style = if model.focused {
        title_focused()
    } else {
        title_unfocused()
    };
    if model.on_server_row {
        title_style = title_style.patch(cursor_style());
    }
    // Legacy parity: the left column never picks up the magenta
    // selection-accent border — only Middle / Queue do.
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(model.server_title.to_string(), title_style))
        .title_alignment(Alignment::Center)
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items: Vec<ListItem> = model
        .rows
        .iter()
        .map(|row| match row {
            LeftRow::SearchHint => ListItem::new(Line::from(vec![
                Span::styled("S", dim_style().add_modifier(Modifier::UNDERLINED)),
                Span::styled("earch\u{2026}", dim_style()),
            ])),
            LeftRow::Blank => ListItem::new(""),
            LeftRow::Playlist {
                name,
                is_viewing,
                is_cursor,
                is_picker,
                is_pending,
            } => {
                // Replacing (not patching) drops bold from
                // `current_style` so a cursor on a viewing row reads
                // as plain Black-on-Yellow.
                let style = if *is_picker || *is_cursor {
                    cursor_style()
                } else if *is_viewing {
                    current_style()
                } else if *is_pending {
                    dim_style()
                } else {
                    Style::default()
                };
                let label = if *is_pending {
                    format!("{spinner_glyph} {name}")
                } else {
                    name.clone()
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            }
            LeftRow::NewPlaylist { is_cursor, focused } => {
                let mut style = dim_style();
                if *is_cursor {
                    style = style.patch(if *focused {
                        cursor_style()
                    } else {
                        cursor_dim_style()
                    });
                }
                ListItem::new(Line::from(Span::styled("New\u{2026}", style)))
            }
        })
        .collect();

    let mut state = list_state(model.list_cursor, scroll_offset);
    frame.render_stateful_widget(List::new(items), inner, &mut state);
    scroll_offset.set(state.offset());
}
