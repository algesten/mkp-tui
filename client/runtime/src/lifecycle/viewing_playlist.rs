//! Connection-scoped playlist interest. Navigation announces a new playlist
//! immediately; heartbeats keep it active without fetching its contents again.
use std::sync::Arc;
use std::time::Duration;

use mkpclient_state_link::{LinkKind, LinkPhase};
use mkpclient_state_ui_history::MiddleMode;
use mkproto::ClientMsg;

use crate::sources::Sources;

pub const HEARTBEAT: Duration = Duration::from_secs(60);

#[derive(drv::Input)]
pub struct ViewingPlaylistInput<'a> {
    connected: bool,
    visible: bool,
    selected: Option<&'a Arc<str>>,
}

#[drv::memo(single)]
pub fn desired_viewing_playlist(input: ViewingPlaylistInput<'_>) -> Option<Arc<str>> {
    if input.connected && input.visible {
        input.selected.cloned()
    } else {
        None
    }
}

#[derive(drv::Input)]
pub struct ViewingPlaylistActionInput<'a> {
    connected: bool,
    desired: Option<&'a Arc<str>>,
    announced: Option<&'a Arc<str>>,
    heartbeat_due: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewingPlaylistAction {
    Clear,
    Send(Option<Arc<str>>),
    Noop,
}

#[drv::memo(single)]
pub fn viewing_playlist_action(input: ViewingPlaylistActionInput<'_>) -> ViewingPlaylistAction {
    if !input.connected {
        return ViewingPlaylistAction::Clear;
    }
    if input.desired != input.announced || input.heartbeat_due {
        ViewingPlaylistAction::Send(input.desired.cloned())
    } else {
        ViewingPlaylistAction::Noop
    }
}

pub fn apply_viewing_playlist(sources: &mut Sources) {
    let connected =
        sources.link.phase == LinkPhase::Connected && sources.link.kind == Some(LinkKind::Client);
    let desired = desired_viewing_playlist(ViewingPlaylistInput {
        connected,
        visible: matches!(sources.history.mode, MiddleMode::PlaylistSongs),
        selected: sources.playlist_tracks.playlist_id.as_ref(),
    });
    let action = viewing_playlist_action(ViewingPlaylistActionInput {
        connected,
        desired: desired.as_ref(),
        announced: sources.session.viewing_playlist.as_ref(),
        heartbeat_due: sources
            .session
            .viewing_playlist_due
            .is_some_and(|deadline| deadline <= sources.clock.now),
    });
    match action {
        ViewingPlaylistAction::Clear => {
            sources.session.viewing_playlist = None;
            sources.session.viewing_playlist_due = None;
        }
        ViewingPlaylistAction::Send(id) => {
            sources.requests.push(
                ClientMsg::ViewingPlaylist {
                    id: id.as_deref().unwrap_or_default().to_owned(),
                },
                None,
            );
            sources.session.viewing_playlist_due =
                id.as_ref().map(|_| sources.clock.now + HEARTBEAT);
            sources.session.viewing_playlist = id;
        }
        ViewingPlaylistAction::Noop => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(s: &mut Sources) -> String {
        match s.requests.pop_front().expect("viewing request").msg {
            ClientMsg::ViewingPlaylist { id } => id,
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn navigation_heartbeats_and_leaving_have_distinct_effects() {
        let mut s = Sources::default();
        s.link.phase = LinkPhase::Connected;
        s.link.kind = Some(LinkKind::Client);
        s.playlist_tracks.playlist_id = Some(Arc::from("a"));
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "a");
        apply_viewing_playlist(&mut s);
        assert!(s.requests.pending.is_empty());
        s.clock.now += HEARTBEAT;
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "a");
        s.playlist_tracks.playlist_id = Some(Arc::from("b"));
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "b");
        s.history.mode = MiddleMode::AlbumDetail {
            album_id: "album".into(),
            album_title: String::new(),
            awaiting_seq: None,
        };
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "");
        assert!(s.session.viewing_playlist_due.is_none());
        apply_viewing_playlist(&mut s);
        assert!(s.requests.pending.is_empty());
    }

    #[test]
    fn reconnect_announces_even_the_same_playlist_again() {
        let mut s = Sources::default();
        s.link.phase = LinkPhase::Connected;
        s.link.kind = Some(LinkKind::Client);
        s.playlist_tracks.playlist_id = Some(Arc::from("a"));
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "a");
        s.link.phase = LinkPhase::Closed;
        apply_viewing_playlist(&mut s);
        assert!(s.session.viewing_playlist_due.is_none());
        s.link.phase = LinkPhase::Connected;
        s.link.kind = Some(LinkKind::Client);
        apply_viewing_playlist(&mut s);
        assert_eq!(sent(&mut s), "a");
    }
}
