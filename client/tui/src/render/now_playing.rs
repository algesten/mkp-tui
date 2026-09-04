//! Now-playing bar consumer.
//!
//! Pure painter: takes a [`NowPlayingModel`] (computed by the runtime
//! memo of the same name), the current spinner glyph, and an
//! optional toast overlay, and writes them into the frame's buffer.
//!
//! Toast and spinner are passed in rather than read from sources
//! because they live on `AppState` for now (state-ui-toast / the
//! `tick` field land in step 4); this slice only pulls the
//! cacheable, song-derived chunk of the bar through the memo.

use mkpclient_runtime::views::{
    NowPlayingMeta, NowPlayingModel, NowPlayingRepeat, NowPlayingStatus, NowPlayingTitle,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{current_style, dim_style, styled_block};

/// Paint the bar from the precomputed model.
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    model: &NowPlayingModel,
    spinner_glyph: char,
    toast: Option<&str>,
) {
    let block = styled_block("Now Playing", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    // ── line 1: title (left) + repeat mode (right) ─────────────────
    // Legacy parity (mkp2 nav/player/bar.rs::draw_bar_song): when
    // repeat is All/One, render "Repeat All" / "Repeat One" right-
    // aligned on the title line in dim grey, truncating the title
    // with `…` if needed to leave a 1-cell gap before the mode.
    let mode_str: &'static str = match model.repeat {
        NowPlayingRepeat::Off => "",
        NowPlayingRepeat::All => "Repeat All",
        NowPlayingRepeat::One => "Repeat One",
    };
    let mode_w = mode_str.width();

    let title_part: Option<(String, Style)> = match &model.title {
        NowPlayingTitle::Hidden => None,
        NowPlayingTitle::NowPlaying(t) => Some((t.to_string(), current_style())),
        NowPlayingTitle::Preview(t) => {
            Some((t.to_string(), dim_style().add_modifier(Modifier::BOLD)))
        }
    };

    if let Some((mut title_text, style)) = title_part {
        let avail = chunks[0].width as usize;
        if mode_w > 0 && title_text.width() + 1 + mode_w > avail {
            let max_title = avail.saturating_sub(1 + mode_w);
            title_text = truncate_with_ellipsis(&title_text, max_title);
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(title_text, style))),
            chunks[0],
        );

        if mode_w > 0 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(mode_str, dim_style())))
                    .alignment(Alignment::Right),
                chunks[0],
            );
        }
    }

    // ── line 2: meta (left) + status (right) ────────────────────────
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(24)])
        .split(chunks[1]);

    let meta_line = if let Some(msg) = toast {
        Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        match &model.meta {
            NowPlayingMeta::Empty => Line::from(Span::raw("")),
            NowPlayingMeta::Song { artist, album, dim } => {
                let style = if *dim {
                    dim_style()
                } else {
                    Style::default().fg(Color::Cyan)
                };
                Line::from(Span::styled(format!("{artist} \u{2022} {album}"), style))
            }
            NowPlayingMeta::Peer(p) => Line::from(Span::styled(
                format!(
                    "{spinner_glyph} {who}: {label}",
                    who = p.who,
                    label = p.label
                ),
                dim_style(),
            )),
        }
    };
    frame.render_widget(Paragraph::new(meta_line), bottom[0]);

    if let Some(status_span) = match &model.status {
        NowPlayingStatus::Hidden => None,
        NowPlayingStatus::Preview { duration } => {
            Some(Span::styled(format!(" {duration}"), dim_style()))
        }
        NowPlayingStatus::Playing {
            icon,
            position,
            duration,
        } => Some(Span::styled(
            format!("{icon} {position} / {duration}"),
            Style::default().fg(Color::Cyan),
        )),
    } {
        frame.render_widget(
            Paragraph::new(Line::from(status_span).alignment(Alignment::Right)),
            bottom[1],
        );
    }
}

/// Trunc-only ellipsis (no padding). Mirrors legacy
/// `mkp2/mkp/src/ui/format.rs::truncate_with_ellipsis`.
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return ".".repeat(max);
    }
    let mut w = 0usize;
    let truncated: String = s
        .chars()
        .take_while(|c| {
            w += UnicodeWidthChar::width(*c).unwrap_or(0);
            w <= max - 1
        })
        .collect();
    format!("{truncated}…")
}
