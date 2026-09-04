//! Server-picker modal painter — opened from Enter on the
//! connected-server row in the left pane. Layered on top of the
//! main view so the queue / playlist tracks stay visible while the
//! user selects a different server.

use mkpclient_runtime::views::ServerPickerModalModel;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding};
use ratatui::Frame;

use super::{current_style, cursor_style, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &ServerPickerModalModel) {
    let count = model.rows.len();
    let width: u16 = 30.min(area.width.saturating_sub(4));
    let height: u16 = ((count as u16) + 2).min(area.height * 80 / 100).max(4);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Server ", title_focused()))
        .title_alignment(Alignment::Center)
        .padding(Padding::horizontal(1));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let items: Vec<ListItem> = model
        .rows
        .iter()
        .map(|row| {
            let style = if row.is_cursor {
                cursor_style()
            } else if row.is_current {
                current_style()
            } else {
                ratatui::style::Style::default()
            };
            ListItem::new(Line::from(Span::styled(row.label.to_string(), style)))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}
