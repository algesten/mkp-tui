//! Confirm-delete-playlist modal consumer.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::ConfirmDeletePlaylistModel;

use unicode_width::UnicodeWidthStr;

use super::{dim_style, pad_or_truncate};

pub fn draw(frame: &mut Frame, area: Rect, model: &ConfirmDeletePlaylistModel) {
    // Legacy parity (mkp2 nav/confirm_delete.rs::draw): ~40 wide x
    // 8 tall, red border + title, body is "Type playlist name to
    // confirm:" / <name> / blank / <input>, bottom hint
    // ` [Enter] delete  [Esc] cancel `.
    let width: u16 = 40.min(area.width.saturating_sub(4));
    let height: u16 = 8;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let red_bold = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let hint_key = Style::default().add_modifier(Modifier::BOLD);
    let bottom = Line::from(vec![
        Span::raw(" "),
        Span::styled("[Enter]", hint_key),
        Span::styled(" delete", dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled("[Esc]", hint_key),
        Span::styled(" cancel", dim_style()),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(" Delete Playlist ", red_bold))
        .title_alignment(Alignment::Center)
        .title_bottom(bottom.centered())
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if inner.height < 4 {
        return;
    }
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Span::styled("Type playlist name to confirm:", dim_style())),
        areas[0],
    );
    let inner_w = areas[1].width as usize;
    frame.render_widget(
        Paragraph::new(Span::styled(
            pad_or_truncate(&model.name, inner_w),
            Style::default(),
        )),
        areas[1],
    );
    let input_w = model.input.width();
    let pad_input: String = if input_w < inner_w {
        format!("{}{}", model.input, " ".repeat(inner_w - input_w))
    } else {
        model.input.to_string()
    };
    // Input field has a dark grey (Indexed 236) background; once the
    // user types the exact playlist name the foreground turns green to
    // flag that Enter is now safe.
    let input_style = if model.matches {
        Style::default().bg(Color::Indexed(236)).fg(Color::Green)
    } else {
        Style::default().bg(Color::Indexed(236))
    };
    frame.render_widget(
        Paragraph::new(Span::styled(pad_input, input_style)),
        areas[3],
    );
}
