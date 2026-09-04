//! Error modal consumer.
//!
//! Painter for the `Screen::ErrorModal` overlay — a red-bordered
//! modal that surfaces a server / pairing error to the user. The
//! body wraps long messages; controls are `Esc`/`c` (handled by the
//! input layer).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use mkpclient_runtime::views::ErrorModalModel;

pub fn draw(frame: &mut Frame, area: Rect, model: &ErrorModalModel) {
    let width: u16 = area.width.min(70);
    let height: u16 = 8.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(
            " Error — Esc=close · c=copy ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    frame.render_widget(
        Paragraph::new(model.message.to_string()).wrap(Wrap { trim: false }),
        inner,
    );
}
