//! Search-input modal consumer.
//!
//! Centered modal with the type-tabs (Song | Artist | Album),
//! the input row, and a "Recent" history list when the user has
//! prior searches stored.

use mkproto::SearchType;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Padding, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::SearchInputModel;

use super::{cursor_style, dim_style, pad_or_truncate, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &SearchInputModel) {
    // Legacy parity (mkp2 nav/search_input.rs::draw): centered
    // " Search " title + bottom ` [Enter] search  [Tab] type  [Esc]
    // close ` hint, body is tabs row (Song | Artist | Album, current
    // = STYLE_SELECTED), input row, then a blank + "Recent" header +
    // history list (query left-aligned, type right-aligned and
    // dimmed).
    let history_count = model.history.len();
    let width = (area.width * 60 / 100).clamp(20, 60);
    let base_height: u16 = 4; // borders(2) + tabs(1) + input(1)
    let min_height: u16 = 8; // breathing room before history fills in
    let history_height: u16 = if history_count > 0 {
        3 + history_count as u16
    } else {
        0
    };
    let max_height = (area.height * 80 / 100).max(base_height);
    let height = (base_height + history_height)
        .max(min_height)
        .min(max_height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 4;
    let modal_area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let hint_key = Style::default().add_modifier(Modifier::BOLD);
    let bottom = Line::from(vec![
        Span::raw(" "),
        Span::styled("[Enter]", hint_key),
        Span::styled(" search", dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled("[Tab]", hint_key),
        Span::styled(" type", dim_style()),
        Span::styled("  ", dim_style()),
        Span::styled("[Esc]", hint_key),
        Span::styled(" close", dim_style()),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Search ", title_focused()))
        .title_alignment(Alignment::Center)
        .title_bottom(bottom.centered());
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if inner.height < 2 {
        return;
    }

    let constraints: Vec<Constraint> = if history_count > 0 {
        vec![
            Constraint::Length(1), // tabs
            Constraint::Length(1), // input
            Constraint::Length(1), // blank separator
            Constraint::Length(1), // "Recent" header
            Constraint::Min(0),    // history area
        ]
    } else {
        vec![
            Constraint::Length(1), // tabs
            Constraint::Length(1), // input
        ]
    };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let tabs_area = areas[0];
    let input_area = areas[1];

    // Tabs row, centered.
    let selected_bg = Style::default().fg(Color::Black).bg(Color::Yellow);
    let tabs: [(&str, SearchType); 3] = [
        ("Song", SearchType::Song),
        ("Artist", SearchType::Artist),
        ("Album", SearchType::Album),
    ];
    let mut tab_spans: Vec<Span> = Vec::new();
    for (i, (label, st)) in tabs.iter().enumerate() {
        if i > 0 {
            tab_spans.push(Span::styled(" | ", dim_style()));
        }
        let style = if *st == model.last_type {
            selected_bg
        } else {
            dim_style()
        };
        tab_spans.push(Span::styled(*label, style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(tab_spans)).alignment(Alignment::Center),
        tabs_area,
    );

    // Input row — pad to full width so the STYLE_INPUT background is
    // visible (legacy `mkp2 nav/mod.rs:41` — Indexed 236).
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

    // History list.
    if history_count > 0 && areas.len() >= 5 {
        frame.render_widget(
            Paragraph::new(Span::styled("Recent", title_focused())),
            areas[3],
        );
        let history_area = areas[4];
        let avail_w = history_area.width as usize;
        let items: Vec<ListItem> = model
            .history
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let label = row.type_label;
                let label_w = label.len();
                let query_w = avail_w.saturating_sub(label_w + 1);
                let query_display = pad_or_truncate(&row.query, query_w);
                let selected = model.history_selected == Some(i);
                if selected {
                    ListItem::new(Line::from(vec![
                        Span::styled(query_display, cursor_style()),
                        Span::styled(
                            format!("{:>width$}", label, width = label_w + 1),
                            cursor_style(),
                        ),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::styled(query_display, Style::default()),
                        Span::styled(
                            format!("{:>width$}", label, width = label_w + 1),
                            dim_style(),
                        ),
                    ]))
                }
            })
            .collect();
        frame.render_widget(List::new(items), history_area);
    }
}
