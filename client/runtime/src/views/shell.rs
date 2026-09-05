//! Which top-level surface is up: the pairing confirmation, the
//! pre-connect screen (server list / connecting status), or the
//! main three-column view.
//!
//! Per `EXAMPLE-ARCH.md` § "Anti-pattern: derived UI state" this
//! is a memo, not an `if` ladder in each UI. The one rule that is
//! easy to get wrong lives here: a link that is down is *not* the
//! same as no session. While the runtime is reconnecting to a
//! server it lost, the main view stays up (with the server-lost
//! modal over it) so the user sits through the outage rather than
//! being dropped onto the server list.

use std::sync::Arc;

use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_pairing::{Pairing, PairingPhase};
use mkpclient_state_ui_session::UiSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ShellModel {
    /// Pairing code confirmation takes over the whole surface.
    Pairing,
    /// No session: discovering, connecting, or picking a server.
    PreConnect,
    /// The three-column view, with whatever `Screen` modal on top.
    Main,
}

#[derive(drv::Input)]
pub struct ShellInput<'a> {
    pub pairing_awaiting_confirmation: bool,
    pub link_connected: bool,
    /// The server a live session was lost to; the runtime is trying
    /// to get it back and the view it belongs to is kept.
    pub lost_server: Option<&'a Arc<str>>,
}

impl<'a> ShellInput<'a> {
    pub fn new(pairing: &'a Pairing, link: &'a Link, session: &'a UiSession) -> Self {
        Self {
            pairing_awaiting_confirmation: pairing.phase == PairingPhase::AwaitingConfirmation,
            link_connected: link.phase == LinkPhase::Connected,
            lost_server: session.lost_server.as_ref(),
        }
    }
}

#[drv::memo(single)]
pub fn shell_model<'a>(input: ShellInput<'a>) -> ShellModel {
    if input.pairing_awaiting_confirmation {
        return ShellModel::Pairing;
    }
    if input.link_connected || input.lost_server.is_some() {
        return ShellModel::Main;
    }
    ShellModel::PreConnect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(pairing: &Pairing, link: &Link, session: &UiSession) -> ShellModel {
        shell_model(ShellInput::new(pairing, link, session))
    }

    #[test]
    fn no_session_is_pre_connect() {
        let (p, l, s) = (Pairing::default(), Link::default(), UiSession::default());
        assert_eq!(shell(&p, &l, &s), ShellModel::PreConnect);
    }

    #[test]
    fn connected_is_main() {
        let p = Pairing::default();
        let l = Link {
            phase: LinkPhase::Connected,
            ..Default::default()
        };
        let s = UiSession::default();
        assert_eq!(shell(&p, &l, &s), ShellModel::Main);
    }

    #[test]
    fn a_lost_session_stays_on_main_until_given_up() {
        let p = Pairing::default();
        let mut s = UiSession {
            lost_server: Some(Arc::from("home")),
            ..Default::default()
        };
        for phase in [
            LinkPhase::Closed,
            LinkPhase::Idle,
            LinkPhase::Connecting,
            LinkPhase::Closing,
        ] {
            let l = Link {
                phase: phase.clone(),
                ..Default::default()
            };
            assert_eq!(shell(&p, &l, &s), ShellModel::Main, "{phase:?}");
        }
        // Giving up (dispatch clears `lost_server`) hands the picker back.
        s.lost_server = None;
        let l = Link {
            phase: LinkPhase::Closed,
            ..Default::default()
        };
        assert_eq!(shell(&p, &l, &s), ShellModel::PreConnect);
    }

    #[test]
    fn pairing_confirmation_wins() {
        let p = Pairing {
            phase: PairingPhase::AwaitingConfirmation,
            ..Default::default()
        };
        let l = Link {
            phase: LinkPhase::Connected,
            ..Default::default()
        };
        let s = UiSession::default();
        assert_eq!(shell(&p, &l, &s), ShellModel::Pairing);
    }
}
