//! Queue-column consumer.
//!
//! Pure painter: takes a [`QueueColumnModel`] (computed by the
//! runtime free function of the same name) plus a `&Cell<usize>`
//! scroll-offset slot owned by `AppState`, and writes the column
//! into the frame's buffer.

use std::cell::Cell;

use mkpclient_runtime::views::QueueColumnModel;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};
use ratatui::Frame;

use super::{
    dim_style, list_state, row_style_combined, selection_accent, styled_block_spans, title_focused,
    title_unfocused,
};

pub fn draw(frame: &mut Frame, area: Rect, model: &QueueColumnModel, scroll_offset: &Cell<usize>) {
    // ── block + title ──────────────────────────────────────────────
    let title_style = if model.focused {
        title_focused()
    } else {
        title_unfocused()
    };
    let title_spans = vec![
        Span::styled(" Q", title_style),
        Span::styled("u", title_style.add_modifier(Modifier::UNDERLINED)),
        Span::styled("eue ", title_style),
    ];
    let mut block = styled_block_spans(title_spans);
    if model.in_selection {
        block = block.border_style(selection_accent());
    }
    if model.focused && model.has_filter {
        block = block.title_bottom(
            Line::from(Span::styled(" Shift-F unfilter ", dim_style())).left_aligned(),
        );
    }
    if let Some(remaining) = model.remaining.as_deref() {
        block = block.title_bottom(
            Line::from(Span::styled(format!(" {remaining} "), dim_style())).right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // ── rows ───────────────────────────────────────────────────────
    let items: Vec<ListItem> = model
        .rows
        .iter()
        .enumerate()
        .map(|(row_i, row)| {
            let is_cursor = row_i == model.selected_filtered && model.focused;
            let style = row_style_combined(
                row_i,
                model.selected_filtered,
                model.focused,
                row.is_current,
                model.in_selection,
            );
            // In selection mode every row carries a 2-column prefix so
            // columns stay aligned: a magenta "❯ " on multi-selected
            // non-cursor rows, two spaces everywhere else.
            if row.is_multi_selected && !is_cursor {
                ListItem::new(Line::from(vec![
                    Span::styled("\u{276F} ", Style::default().fg(Color::LightMagenta)),
                    Span::styled(row.title.clone(), style),
                ]))
            } else if model.in_selection {
                ListItem::new(Line::from(vec![
                    Span::styled("  ", style),
                    Span::styled(row.title.clone(), style),
                ]))
            } else {
                ListItem::new(Line::from(Span::styled(row.title.clone(), style)))
            }
        })
        .collect();
    let mut state = list_state(Some(model.selected_filtered), scroll_offset);
    frame.render_stateful_widget(List::new(items), inner, &mut state);
    scroll_offset.set(state.offset());
}
