//! Generic input-modal consumer (used by Create/Rename Playlist).

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::InputModalModel;
use unicode_width::UnicodeWidthStr;

use super::{dim_style, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &InputModalModel) {
    // Legacy parity (mkp2 nav/{create,rename}_playlist.rs::draw):
    // ~40 wide x 6 tall, Padding::new(1,1,1,1), centered title +
    // bottom hint `[Enter] verb  [Esc] cancel`, body is "Name:"
    // dim header on first row + the input on the second.
    let width = (area.width * 50 / 100).clamp(20, 40);
    let height: u16 = 6;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 3;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let title_style = title_focused();
    let hint_key = Style::default().add_modifier(Modifier::BOLD);
    let bottom = Line::from(vec![
        Span::raw(" "),
        Span::styled("[Enter]", hint_key),
        Span::styled(format!(" {}", model.hint_verb), dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled("[Esc]", hint_key),
        Span::styled(" cancel", dim_style()),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::new(1, 1, 1, 1))
        .title(Span::styled(format!(" {} ", model.title), title_style))
        .title_alignment(Alignment::Center)
        .title_bottom(bottom.centered());
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if inner.height < 2 {
        return;
    }
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(Paragraph::new(Span::styled("Name:", dim_style())), areas[0]);
    let input_w = areas[1].width as usize;
    let typed_w = model.input.width();
    let padded: String = if typed_w < input_w {
        format!("{}{}", model.input, " ".repeat(input_w - typed_w))
    } else {
        model.input.to_string()
    };
    // Legacy parity (`mkp2 nav/mod.rs:41` STYLE_INPUT): the input
    // field has a dark grey (Indexed 236) background so the user
    // can see where the editable area is even when empty.
    let input_style = Style::default().bg(Color::Indexed(236));
    frame.render_widget(Paragraph::new(Span::styled(padded, input_style)), areas[1]);
}
