//! Confirm-remove-from-playlist modal consumer.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::ConfirmRemoveModel;

pub fn draw(frame: &mut Frame, area: Rect, model: &ConfirmRemoveModel) {
    let w = area.width.clamp(34, 50);
    let h = 5u16.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(ratatui::widgets::Clear, modal);

    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(" Remove \"{}\"?", model.song_title),
            Style::default().fg(Color::Red),
        )),
    ])
    .block(
        Block::default()
            .title(Span::styled(
                " Enter=remove  Esc=cancel ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL),
    );
    frame.render_widget(body, modal);
}
