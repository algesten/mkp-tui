//! Step 3 of the lifecycle: open the `ServerLostModal` when the link
//! drops while a backend was active, close it on reconnect.
//!
//! Spec §6: `desired_lost_modal()` answers "should the lost modal be
//! up?" given link.phase + session.lost_server; `lost_modal_action()`
//! diffs against the actual screen and returns Open/Close/Noop.
//! `apply_lost_modal()` writes the screen synchronously.

use std::sync::Arc;

use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_ui_screen::Screen;
use mkpclient_state_ui_session::UiSession;

use crate::sources::Sources;

// ─── inputs ─────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct LinkPhaseInput {
    pub connected: bool,
}

impl LinkPhaseInput {
    pub fn new(l: &Link) -> Self {
        Self {
            connected: matches!(l.phase, LinkPhase::Connected),
        }
    }
}

#[derive(drv::Input)]
pub struct LostServerInput<'a> {
    pub lost_server: Option<&'a std::sync::Arc<str>>,
}

impl<'a> LostServerInput<'a> {
    pub fn new(s: &'a UiSession) -> Self {
        Self {
            lost_server: s.lost_server.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct ScreenKindInput {
    pub on_now_playing: bool,
    pub on_lost_modal: bool,
}

impl ScreenKindInput {
    pub fn new(s: &Screen) -> Self {
        Self {
            on_now_playing: matches!(s, Screen::NowPlaying),
            on_lost_modal: matches!(s, Screen::ServerLostModal { .. }),
        }
    }
}

// ─── memos ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub enum DesiredLostModal {
    Hide,
    Show { server: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LostModalAction {
    Noop,
    Open { server: String },
    Close,
}

#[drv::memo(single)]
pub fn desired_lost_modal<'a>(
    link: LinkPhaseInput,
    session: LostServerInput<'a>,
) -> DesiredLostModal {
    if link.connected {
        return DesiredLostModal::Hide;
    }
    match session.lost_server {
        Some(name) => DesiredLostModal::Show {
            server: name.to_string(),
        },
        None => DesiredLostModal::Hide,
    }
}

#[drv::memo(single)]
pub fn lost_modal_action(desired: DesiredLostModal, screen: ScreenKindInput) -> LostModalAction {
    match desired {
        DesiredLostModal::Show { server } if screen.on_now_playing => {
            LostModalAction::Open { server }
        }
        DesiredLostModal::Show { .. } => LostModalAction::Noop,
        DesiredLostModal::Hide if screen.on_lost_modal => LostModalAction::Close,
        DesiredLostModal::Hide => LostModalAction::Noop,
    }
}

// ─── trampoline ─────────────────────────────────────────────────────

pub fn apply_lost_modal(sources: &mut Sources) {
    let desired = desired_lost_modal(
        LinkPhaseInput::new(&sources.link),
        LostServerInput::new(&sources.session),
    );
    let action = lost_modal_action(desired, ScreenKindInput::new(&sources.screen));
    match action {
        LostModalAction::Noop => {}
        LostModalAction::Open { server } => {
            sources.screen = Screen::ServerLostModal {
                server: Arc::from(server),
            };
        }
        LostModalAction::Close => {
            sources.screen = Screen::NowPlaying;
        }
    }
}
