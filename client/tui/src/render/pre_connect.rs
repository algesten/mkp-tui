//! Pre-connect view consumer.

use mkpclient_runtime::views::{ConnectingKind, PreConnectModel};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::{cursor_style, dim_style};

pub fn draw(frame: &mut Frame, area: Rect, model: &PreConnectModel, spinner_glyph: char) {
    // Legacy parity (`mkp2 nav/discovering.rs`): plain title without
    // focused-cyan styling — the discovering screen has no focus.
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Make Play ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match model {
        PreConnectModel::Status(kind) => {
            let msg = match kind {
                ConnectingKind::Discovering => {
                    format!("{spinner_glyph} Searching for Make Play server...")
                }
                ConnectingKind::ToServer { name } => {
                    format!("{spinner_glyph} Connecting to {name}...")
                }
            };
            // Legacy applies dim style to the *paragraph* so blank
            // cells around the message also pick up the dim fg.
            frame.render_widget(Paragraph::new(msg).style(dim_style()), inner);
        }
        PreConnectModel::ServerList { rows } => {
            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    let style = if r.is_cursor {
                        cursor_style()
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(vec![Span::styled(
                        format!("  {}", r.label),
                        style,
                    )]))
                })
                .collect();
            frame.render_widget(List::new(items), inner);
        }
    }
}
