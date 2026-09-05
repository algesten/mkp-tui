//! Step 3 of the lifecycle: open the `ServerLostModal` when the link
//! drops while a backend was active, close it on reconnect.
//!
//! Spec §6: `desired_lost_modal()` answers "should the lost modal be
//! up?" given link.phase + session.lost_server; `lost_modal_action()`
//! diffs against the actual screen and returns Open/Close/Noop.
//! `apply_lost_modal()` writes the screen synchronously.
//!
//! The modal replaces whatever screen is up when the link drops. The
//! main view stays painted underneath while the runtime reconnects,
//! and the modal is what keeps the user from navigating a server
//! that is not there: any request issued in that window would be
//! sent on reconnect, ahead of the handshake, and the resumed view
//! would then overwrite the navigation that issued it.

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
    pub on_lost_modal: bool,
}

impl ScreenKindInput {
    pub fn new(s: &Screen) -> Self {
        Self {
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
        DesiredLostModal::Show { .. } if screen.on_lost_modal => LostModalAction::Noop,
        DesiredLostModal::Show { server } => LostModalAction::Open { server },
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

#[cfg(test)]
mod tests {
    use super::*;
    use mkpclient_state_ui_screen::SearchState;

    fn lost(link: &Link, session: &UiSession, screen: &Screen) -> LostModalAction {
        let desired = desired_lost_modal(LinkPhaseInput::new(link), LostServerInput::new(session));
        lost_modal_action(desired, ScreenKindInput::new(screen))
    }

    fn dropped_session() -> (Link, UiSession) {
        let link = Link {
            phase: LinkPhase::Closed,
            ..Default::default()
        };
        let session = UiSession {
            lost_server: Some(Arc::from("home")),
            ..Default::default()
        };
        (link, session)
    }

    #[test]
    fn opens_over_whatever_screen_is_up_when_the_link_drops() {
        let (link, session) = dropped_session();
        let search = Screen::SearchInput(SearchState {
            input: Arc::from("abba"),
            last_type: mkproto::SearchType::Song,
            history: imbl::Vector::new(),
            history_selected: None,
        });
        for screen in [
            Screen::NowPlaying,
            search,
            Screen::HelpOverlay { scroll: 0 },
        ] {
            assert_eq!(
                lost(&link, &session, &screen),
                LostModalAction::Open {
                    server: "home".into()
                }
            );
        }
    }

    #[test]
    fn stays_put_while_already_open_and_closes_on_reconnect() {
        let (mut link, session) = dropped_session();
        let open = Screen::ServerLostModal {
            server: Arc::from("home"),
        };
        assert_eq!(lost(&link, &session, &open), LostModalAction::Noop);

        link.phase = LinkPhase::Connected;
        assert_eq!(lost(&link, &session, &open), LostModalAction::Close);
        assert_eq!(
            lost(&link, &session, &Screen::NowPlaying),
            LostModalAction::Noop
        );
    }
}
