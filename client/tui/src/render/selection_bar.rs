//! Bottom "Selection" bar consumer (replaces the now-playing bar
//! while multi-select is active).
//!
//! The keystroke hints (line 1, line 2) are static — they don't
//! belong in the model. Only the right-aligned counter on line 2
//! varies, and that comes from [`SelectionBarModel`].

use mkpclient_runtime::views::SelectionBarModel;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use super::{dim_style, selection_accent, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &SelectionBarModel) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Selection ", title_focused()))
        .title_alignment(Alignment::Center)
        .padding(Padding::horizontal(1))
        .border_style(selection_accent());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let key = Style::default().add_modifier(Modifier::BOLD);
    let line1 = Line::from(vec![
        Span::styled("[\u{2192}]", key),
        Span::styled(" select  ", dim_style()),
        Span::styled("[\u{2190}]", key),
        Span::styled(" deselect  ", dim_style()),
        Span::styled("[space]", key),
        Span::styled(" range", dim_style()),
    ]);
    let line2 = Line::from(vec![
        Span::styled("[enter]", key),
        Span::styled(" play  ", dim_style()),
        Span::styled("[tab]", key),
        Span::styled(" actions  ", dim_style()),
        Span::styled("[esc]", key),
        Span::styled(" cancel", dim_style()),
    ]);
    frame.render_widget(Paragraph::new(line1), chunks[0]);
    frame.render_widget(Paragraph::new(line2), chunks[1]);

    if let Some(info) = model.info.as_deref() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(info.to_string(), dim_style())).right_aligned()),
            chunks[1],
        );
    }
}
