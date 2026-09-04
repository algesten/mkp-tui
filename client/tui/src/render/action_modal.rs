//! Action modal consumer.
//!
//! Painter for the per-row action context menu (`x` from middle or
//! queue panes). Consumes a memo-produced `ActionModalModel` per
//! the canonical view pattern.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::ActionModalModel;

use super::cursor_style;

pub fn draw(frame: &mut Frame, area: Rect, model: &ActionModalModel) {
    // Legacy parity (mkp2 nav/action_modal.rs::draw): borders only,
    // no title, 28 wide, height = 2 + item_count, items rendered as
    // ` [k] Label ` with yellow key tag.
    let w = 28u16.min(area.width);
    let h = (2 + model.rows.len() as u16).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(ratatui::widgets::Clear, modal);

    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

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
