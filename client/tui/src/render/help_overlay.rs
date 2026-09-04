//! Help-overlay painter — 2-column legacy layout (mkp2
//! `nav/help_overlay.rs::draw`): " Help " title, two paragraphs
//! laid out side-by-side via a horizontal split, bottom hint
//! `[c] configure  [Esc] close`. The model carries pre-formatted
//! key + description strings so the painter is allocation-free on
//! cache hits.

use mkpclient_runtime::views::{HelpOverlayModel, HelpSection};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

use super::{dim_style, title_focused};

/// Width of the key column inside each pane. Matches legacy: left
/// column uses key-width 18, right uses 22 — the right column has
/// chord-style entries like `Shift-\u{2190} / Shift-\u{2192}` that
/// need more room than the left.
const LEFT_KEY_W: usize = 18;
const RIGHT_KEY_W: usize = 22;

pub fn draw(frame: &mut Frame, area: Rect, model: &HelpOverlayModel) {
    let left_lines = section_lines(&model.left, LEFT_KEY_W);
    let right_lines = section_lines(&model.right, RIGHT_KEY_W);
    let content_h = left_lines.len().max(right_lines.len()) as u16;

    let width: u16 = 82.min(area.width.saturating_sub(4));
    let height = (content_h + 2).min(area.height * 80 / 100).max(5);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, modal_area);

    let hint_key = Style::default().add_modifier(Modifier::BOLD);
    let bottom = Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("[{}]", model.configure_key), hint_key),
        Span::styled(" configure", dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled(format!("[{}]", model.close_key), hint_key),
        Span::styled(" close", dim_style()),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Help ", title_focused()))
        .title_alignment(Alignment::Center)
        .title_bottom(bottom.centered());
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(left_lines).scroll((model.scroll, 0)),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(right_lines).scroll((model.scroll, 0)),
        cols[1],
    );
}

fn section_lines(sections: &[HelpSection], key_w: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            section.heading.clone(),
            title_focused(),
        )));
        for entry in &section.entries {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:width$}", entry.key, width = key_w),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(entry.description.clone()),
            ]));
        }
    }
    lines
}
