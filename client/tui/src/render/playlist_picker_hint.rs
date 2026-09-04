//! Playlist-picker hint modal consumer.
//!
//! Tiny "[\u{2190}] Select playlist" hint shown while the left
//! column is acting as a playlist picker. The picker itself is
//! drawn inside `draw_playlists_col` via the left-column model;
//! this overlay just tells the user where to look.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mkpclient_runtime::views::PlaylistPickerHintModel;

pub fn draw(frame: &mut Frame, area: Rect, _model: &PlaylistPickerHintModel) {
    // Legacy parity (mkp2 nav/action_modal.rs::draw_playlist_picker_hint):
    // small 23x3 modal with one line ` [←] Select playlist ` and a
    // yellow-bold left-arrow tag.
    let width: u16 = 23;
    let height: u16 = 3;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);
    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let line = Line::from(vec![
        Span::styled(
            " [\u{2190}] ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Select playlist", Style::default()),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
}
