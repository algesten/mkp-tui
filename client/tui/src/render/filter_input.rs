//! Filter-input modal consumer.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::FilterInputModel;

use super::{dim_style, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &FilterInputModel) {
    // Legacy parity (mkp2 nav/filter_input.rs): ~40 wide, " Filter "
    // title, bottom hint ` [Enter] apply  [Esc] cancel `, body is
    // the input field padded to width.
    let _ = model.target;
    let width = (area.width * 50 / 100).clamp(20, 40);
    let height: u16 = 4;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 3;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let hint_key = Style::default().add_modifier(Modifier::BOLD);
    let bottom = Line::from(vec![
        Span::raw(" "),
        Span::styled("[Enter]", hint_key),
        Span::styled(" apply", dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled("[Esc]", hint_key),
        Span::styled(" cancel", dim_style()),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Filter ", title_focused()))
        .title_alignment(Alignment::Center)
        .title_bottom(bottom.centered());
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if inner.height < 1 {
        return;
    }
    let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let input_w = input_area.width as usize;
    let padded: String = if model.input.len() < input_w {
        format!("{}{}", model.input, " ".repeat(input_w - model.input.len()))
    } else {
        model.input.to_string()
    };
    let input_style = Style::default().bg(Color::Indexed(236));
    frame.render_widget(
        Paragraph::new(Span::styled(padded, input_style)),
        input_area,
    );
}
