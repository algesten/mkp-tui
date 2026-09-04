//! Pairing-confirmation modal consumer.
//!
//! Painter shown while `PairingPhase::AwaitingConfirmation` — the
//! user must visually verify the short code and server fingerprint
//! match the values displayed by the server before accepting (`y` /
//! Enter) or rejecting (`n`) the new pairing.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use mkpclient_runtime::views::PairingModalModel;

use super::{current_style, styled_block};

pub fn draw(frame: &mut Frame, area: Rect, model: &PairingModalModel) {
    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Verify this code matches the one shown on the server:",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("    {}", model.code), current_style())),
        Line::from(""),
        Line::from(format!("Server fingerprint: {}", model.fingerprint)),
        Line::from(""),
        Line::from(Span::styled(
            "y/Enter = confirm     n = reject",
            Style::default().fg(Color::Cyan),
        )),
    ])
    .wrap(Wrap { trim: false })
    .block(styled_block("Pairing", true));
    frame.render_widget(body, area);
}
