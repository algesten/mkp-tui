//! Selection-action modal consumer.
//!
//! Rendered when the user presses `x` while in selection mode —
//! the list of bulk-actions ("Play next", "Play last", "Add to
//! playlist", …) materialised by `selection_action_modal_model`.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::SelectionActionModalModel;

use super::cursor_style;

pub fn draw(frame: &mut Frame, area: Rect, model: &SelectionActionModalModel) {
    // Legacy parity (mkp2 nav/selection_action.rs): centered title
    // " <count> selected " + borders, no padding, items rendered as
    // ` [k] Label ` with yellow key tag.
    let count = model.count;
    let width: u16 = 28;
    let height: u16 = 2 + model.rows.len() as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    let title = format!(" {count} selected ");
    // Legacy parity: the bulk-action modal title is yellow-bold,
    // not cyan-bold like other section titles.
    let title_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, title_style))
        .title_alignment(Alignment::Center);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let yellow = Style::default().fg(Color::Yellow);
    let lines: Vec<Line> = model
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let cur = i == model.selected;
            let row_style = if cur {
                cursor_style()
            } else {
                Style::default()
            };
            let key_style = if cur { cursor_style() } else { yellow };
            Line::from(vec![
                Span::styled(format!(" [{}] ", row.key), key_style),
                Span::styled(row.label, row_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
