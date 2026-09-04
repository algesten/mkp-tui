//! Server-lost modal consumer.
//!
//! Painter for the `Screen::ServerLostModal` overlay — a yellow-
//! bordered modal that informs the user the connection dropped and
//! shows a spinner while the runtime attempts to reconnect.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::ServerLostModalModel;

use super::dim_style;

pub fn draw(frame: &mut Frame, area: Rect, model: &ServerLostModalModel, spinner_glyph: char) {
    let width: u16 = area.width.min(60);
    let height: u16 = 5.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " Connection lost — Enter=pick another · Esc=keep waiting ",
            Style::default().fg(Color::Yellow),
        ));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    let lines = vec![
        Line::from(format!("Lost connection to {}", model.server)),
        Line::from(Span::styled(
            format!("{spinner_glyph} reconnecting…"),
            dim_style(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}
