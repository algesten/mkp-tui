use mkpclient_runtime::views::KeybindingsEditorModel;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph};
use ratatui::Frame;

use super::{cursor_style, dim_style, title_focused};

pub fn draw(frame: &mut Frame, area: Rect, model: &KeybindingsEditorModel) {
    let width = area.width.saturating_sub(4).min(65);
    let height = (area.height * 60 / 100).max(10);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, modal_area);

    let hints = if model.listening || model.adding {
        let mode = if model.adding { "add" } else { "rebind" };
        hint_line(&[("any key", mode), ("Esc", "cancel")])
    } else {
        hint_line(&[
            ("Enter", "rebind"),
            ("a", "add key"),
            ("d", "reset"),
            ("s", "save"),
            ("Esc", "close"),
        ])
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Keybindings ", title_focused()))
        .title_alignment(Alignment::Center)
        .title_bottom(hints.centered())
        .padding(Padding::horizontal(1));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    if inner.width < 30 || inner.height < 3 {
        return;
    }

    let left_w = 20u16.min(inner.width / 3);
    let [left_area, div_area, right_area] = Layout::horizontal([
        Constraint::Length(left_w),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let contexts: Vec<ListItem> = model
        .contexts
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let selected = i == model.selected_context;
            let style = if selected && !model.focus_right {
                cursor_style()
            } else if selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            ListItem::new(Span::styled(format!(" {name}"), style))
        })
        .collect();
    let mut context_state = ListState::default();
    context_state.select(Some(model.selected_context));
    frame.render_stateful_widget(List::new(contexts), left_area, &mut context_state);

    let divider: Vec<Line> = (0..div_area.height)
        .map(|_| Line::from(Span::styled("│", dim_style())))
        .collect();
    frame.render_widget(Paragraph::new(divider), div_area);

    let name_w = 20.min((right_area.width as usize).saturating_sub(10));
    let actions: Vec<ListItem> = model
        .actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let selected = i == model.selected_binding && model.focus_right;
            let name_style = if selected {
                cursor_style()
            } else {
                Style::default()
            };
            let input_style = if selected && (model.listening || model.adding) {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            };
            let mut spans = vec![Span::styled(
                format!(" {:<name_w$}", action.name),
                name_style,
            )];
            for (i, key) in action.keys.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(format!(" {key} "), input_style));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut action_state = ListState::default();
    if model.focus_right {
        action_state.select(Some(model.selected_binding));
    }
    frame.render_stateful_widget(List::new(actions), right_area, &mut action_state);
}

fn hint_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, description)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("[{key}]"),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {description}"), dim_style()));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}
