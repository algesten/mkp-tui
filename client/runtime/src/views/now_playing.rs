//! View model for the bottom "Now Playing" bar.
//!
//! ## Memo + thin overlay
//!
//! The cacheable part — the song-derived line + status — is produced
//! by [`now_playing_song_model`], a `#[drv::memo]` over projected
//! `state-server-state` and `state-ui-preview` fields.
//!
//! Peer activity is overlaid on top of that result by
//! [`now_playing_model`], a free function: the foreign source it
//! reads (`Activity.tasks: imbl::HashMap<TaskId, ActiveTask>`)
//! contains a value type defined outside this crate, and per
//! guideline 11 we don't put `drv::Input` on foreign types. The
//! overlay is a couple of branches plus a single `format!` when a
//! peer is busy — well within the "tick-to-tick fold" precedent
//! `nearest_deadline` set in `deadlines.rs`.
//!
//! ## What's NOT in either layer
//!
//! Two transient pieces stay outside the model entirely because
//! they update per-tick and would defeat caching / the bridge:
//!
//!   - **Toast / `last_message`** — still lives on `AppState`
//!     (state-ui-toast comes in step 4). The renderer overlays it
//!     on the bottom-left line.
//!   - **Spinner glyph** — `app.tick` advances every loop iteration.
//!     The renderer paints the glyph at draw time; the model only
//!     carries `Meta::Peer { who, label }` (no glyph) when peer
//!     activity should display.

use std::sync::Arc;

use imbl::HashMap as ImHashMap;
#[cfg(test)]
use mkproto::PlayState;
use mkproto::{PlaybackState, RepeatMode, Song, TaskActivity, TaskId};

use mkpclient_state_activity::{ActiveTask, Activity};
use mkpclient_state_server_state::ServerState;
use mkpclient_state_ui_preview::UiPreview;

use crate::Peer;

// ─── output model ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum NowPlayingTitle {
    /// No song to display: server hasn't sent state yet, or has but
    /// `now_playing` is `None`. Renderer paints a blank line —
    /// matches legacy `mkp2 nav/player/bar.rs` which short-circuits
    /// on `display_song.is_none()`.
    Hidden,
    /// Real playback. Title styled green-bold by the renderer.
    NowPlaying(Arc<str>),
    /// Hover preview. Title styled dim-bold.
    Preview(Arc<str>),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PeerActivityFrame {
    pub who: Arc<str>,   // "user@host"
    pub label: Arc<str>, // "Searching 'foo'", etc.
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum NowPlayingMeta {
    /// Nothing useful to show. Renderer paints a blank line.
    Empty,
    /// "{artist} • {album}". `dim = true` when the source is the
    /// preview (renderer dims), false when it's the playing song
    /// (renderer paints cyan).
    Song {
        artist: Arc<str>,
        album: Arc<str>,
        dim: bool,
    },
    /// Peer activity. Renderer prepends a spinner glyph from
    /// `app.tick` and paints the whole line dim.
    Peer(PeerActivityFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum NowPlayingStatus {
    /// Nothing to show. Renderer paints a blank line — matches
    /// legacy.
    Hidden,
    /// " {duration}" dim. Used while a preview shadows playback.
    Preview { duration: Arc<str> },
    /// Real playback. Renderer paints "{icon} {pos} / {dur}" cyan.
    Playing {
        icon: char,
        position: Arc<str>,
        duration: Arc<str>,
    },
}

/// drv-friendly mirror of `mkproto::RepeatMode`. Foreign enums don't
/// carry `drv::Input` (per guideline 11 mkproto stays drv-free), so
/// we round-trip through a local copy with the same shape. Renderer
/// paints "Repeat All" / "Repeat One" right-aligned on the title
/// line for All / One; Off paints nothing — legacy parity with
/// `mkp2 nav/player/bar.rs::draw_bar_song`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, drv::Input)]
pub enum NowPlayingRepeat {
    Off,
    All,
    One,
}

impl From<RepeatMode> for NowPlayingRepeat {
    fn from(m: RepeatMode) -> Self {
        match m {
            RepeatMode::Off => NowPlayingRepeat::Off,
            RepeatMode::All => NowPlayingRepeat::All,
            RepeatMode::One => NowPlayingRepeat::One,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NowPlayingModel {
    pub title: NowPlayingTitle,
    pub meta: NowPlayingMeta,
    pub status: NowPlayingStatus,
    pub repeat: NowPlayingRepeat,
}

// ─── peer-identity input ───────────────────────────────────────────

/// Identity of the local peer, projected into a `drv::Input`-friendly
/// pair so the activity overlay can match against `me`.
#[derive(Debug, Clone, PartialEq, Eq, drv::Input)]
pub struct PeerIdInput {
    pub user: Arc<str>,
    pub host: Arc<str>,
}

impl PeerIdInput {
    pub fn new(p: &Peer) -> Self {
        Self {
            user: Arc::from(p.user.as_str()),
            host: Arc::from(p.host.as_str()),
        }
    }
}

// ─── inputs ────────────────────────────────────────────────────────

/// Projection of a single `Song` for view-model memos. Only the
/// fields the bar needs end up in the cache key; the rest of `Song`
/// (artwork URLs, track number, …) doesn't trigger recomputation.
#[derive(drv::Input)]
pub struct SongMetaInput<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub artist_name: &'a str,
    pub album_title: &'a str,
    pub duration: f32,
}

impl<'a> SongMetaInput<'a> {
    pub fn new(s: &'a Song) -> Self {
        Self {
            id: &s.id,
            title: &s.title,
            artist_name: &s.artist_name,
            album_title: &s.album_title,
            duration: s.duration,
        }
    }
}

/// Projection of `state-server-state::ServerState` — the bits the
/// now-playing bar reads. `playback_icon` is computed at projection
/// time so the memo doesn't need to depend on `PlaybackState` (a
/// foreign type without `drv::Input`).
#[derive(drv::Input)]
pub struct ServerNowPlayingInput<'a> {
    pub song: Option<SongMetaInput<'a>>,
    pub playback_icon: Option<char>,
    pub position_secs: Option<f32>,
    /// Whether the source has any `play` at all. Distinguishes
    /// "awaiting state" (none) from "idle" (some, but no song).
    pub has_play: bool,
    pub repeat: NowPlayingRepeat,
}

impl<'a> ServerNowPlayingInput<'a> {
    pub fn new(s: &'a ServerState) -> Self {
        match s.play.as_ref() {
            None => Self {
                song: None,
                playback_icon: None,
                position_secs: None,
                has_play: false,
                repeat: NowPlayingRepeat::Off,
            },
            Some(p) => Self {
                song: p.now_playing.as_ref().map(SongMetaInput::new),
                playback_icon: Some(playback_icon(p.playback)),
                position_secs: Some(p.position as f32),
                has_play: true,
                repeat: p.repeat.into(),
            },
        }
    }
}

#[derive(drv::Input)]
pub struct UiPreviewInput<'a> {
    pub song: Option<SongMetaInput<'a>>,
}

impl<'a> UiPreviewInput<'a> {
    pub fn new(p: &'a UiPreview) -> Self {
        Self {
            song: p.song.as_ref().map(SongMetaInput::new),
        }
    }
}

/// Projection of `state-activity::Activity` for the now-playing
/// overlay. The map is `imbl::HashMap<TaskId, Arc<ActiveTask>>` so
/// cache-hit checks ptr-eq the underlying buffer.
#[derive(drv::Input)]
pub struct ActivityInput<'a> {
    pub tasks: &'a ImHashMap<TaskId, Arc<ActiveTask>>,
}

impl<'a> ActivityInput<'a> {
    pub fn new(a: &'a Activity) -> Self {
        Self { tasks: &a.tasks }
    }
}

// ─── memo: the cacheable, song-only slice ──────────────────────────

#[drv::memo(single)]
pub fn now_playing_song_model<'a, 'b>(
    server: ServerNowPlayingInput<'a>,
    preview: UiPreviewInput<'b>,
) -> NowPlayingModel {
    // Preview wins over now_playing (and hides itself when it's
    // already the playing song). Mirrors legacy parity in
    // `draw_now_playing_bar`.
    let now_playing_id = server.song.as_ref().map(|s| s.id);

    let preview_song: Option<&SongMetaInput<'_>> = preview
        .song
        .as_ref()
        .filter(|s| now_playing_id != Some(s.id));

    let (title, song_meta_dim, status) = match preview_song {
        Some(s) => (
            NowPlayingTitle::Preview(Arc::from(s.title)),
            true,
            NowPlayingStatus::Preview {
                duration: Arc::from(format_duration(s.duration)),
            },
        ),
        None => match server.song.as_ref() {
            None => (NowPlayingTitle::Hidden, false, NowPlayingStatus::Hidden),
            Some(song) => {
                let icon = server.playback_icon.unwrap_or('?');
                let status = NowPlayingStatus::Playing {
                    icon,
                    position: Arc::from(format_duration(server.position_secs.unwrap_or(0.0))),
                    duration: Arc::from(format_duration(song.duration)),
                };
                (
                    NowPlayingTitle::NowPlaying(Arc::from(song.title)),
                    false,
                    status,
                )
            }
        },
    };

    let meta_song: Option<(&SongMetaInput<'_>, bool)> = match preview_song {
        Some(s) => Some((s, song_meta_dim)),
        None => server.song.as_ref().map(|s| (s, false)),
    };

    let meta = match meta_song {
        Some((s, dim)) => NowPlayingMeta::Song {
            artist: Arc::from(s.artist_name),
            album: Arc::from(s.album_title),
            dim,
        },
        None => NowPlayingMeta::Empty,
    };

    NowPlayingModel {
        title,
        meta,
        status,
        repeat: server.repeat,
    }
}

// ─── overlay: peer-activity replaces song-meta ─────────────────────

/// Build the full now-playing model — the cacheable song slice plus
/// peer-activity overlay — in one memo per spec §4.
#[drv::memo(single)]
pub fn now_playing_model<'a, 'b, 'c>(
    server: ServerNowPlayingInput<'a>,
    preview: UiPreviewInput<'b>,
    activity: ActivityInput<'c>,
    me: PeerIdInput,
) -> NowPlayingModel {
    let mut m = now_playing_song_model(server, preview);

    // Peer activity only overrides the meta line when there's no
    // active hover preview. Mirrors legacy parity in
    // `draw_now_playing_bar`.
    let has_preview = matches!(m.meta, NowPlayingMeta::Song { dim: true, .. })
        || matches!(m.title, NowPlayingTitle::Preview(_));

    if !has_preview {
        if let Some(frame) = aggregate_foreign_activity(activity.tasks, &me) {
            m.meta = NowPlayingMeta::Peer(frame);
        }
    }
    m
}

/// Pick the highest-priority activity across all foreign peers and
/// fold any peers sharing that same activity label into a single
/// frame (legacy mkp2 only ever showed one — the new TUI joins
/// names: `"Bob, Carol Searching 'foo'"`). Activity priority:
/// Searching > Browsing > anything else; ties broken by ordering
/// in the underlying map (`imbl::HashMap` is unordered, so this is
/// stable enough for a status line).
fn aggregate_foreign_activity(
    tasks: &ImHashMap<TaskId, Arc<ActiveTask>>,
    me: &PeerIdInput,
) -> Option<PeerActivityFrame> {
    let mut foreign: Vec<&ActiveTask> = tasks
        .values()
        .filter(|a| a.peer.user.as_str() != &*me.user || a.peer.host.as_str() != &*me.host)
        .map(|a| &**a)
        .collect();
    if foreign.is_empty() {
        return None;
    }
    foreign.sort_by_key(|a| activity_priority(&a.activity));
    let lead = foreign.first()?;
    let lead_label = format_task_activity(&lead.activity);
    // Group by exact label so "Searching 'foo'" + "Searching 'bar'"
    // don't collapse into one (different terms = visually different).
    let mut peers: Vec<String> = foreign
        .iter()
        .filter(|a| format_task_activity(&a.activity) == lead_label)
        .map(|a| format!("{}@{}", a.peer.user, a.peer.host))
        .collect();
    peers.sort();
    peers.dedup();
    Some(PeerActivityFrame {
        who: Arc::from(peers.join(", ")),
        label: Arc::from(lead_label),
    })
}

/// Lower number = shown first.
fn activity_priority(a: &TaskActivity) -> u8 {
    use TaskActivity::*;
    match a {
        Searching { .. } => 0,
        BrowsingAlbum { .. } | BrowsingArtist { .. } => 1,
        LoadingPlaylists | LoadingPlaylist { .. } => 2,
        AddingToPlaylist { .. }
        | RemovingSongs { .. }
        | DeletingPlaylist { .. }
        | RenamingPlaylist { .. }
        | CreatingPlaylist { .. } => 3,
        Playing { .. } | Skipping | Transport | Navigating { .. } => 4,
    }
}

// ─── helpers (kept here so the renderer doesn't fork copies) ──────

fn playback_icon(p: PlaybackState) -> char {
    match p {
        PlaybackState::Playing => '▶',
        PlaybackState::Paused => '⏸',
        PlaybackState::Stopped => '■',
        PlaybackState::Loading => '…',
    }
}

use super::util::format_duration;

fn format_task_activity(a: &TaskActivity) -> String {
    use TaskActivity::*;
    match a {
        Searching { term } => format!("Searching '{term}'"),
        BrowsingAlbum { id } => format!("Browsing album {id}"),
        BrowsingArtist { id } => format!("Browsing artist {id}"),
        Navigating { .. } => "Navigating".into(),
        LoadingPlaylists => "Loading playlists".into(),
        LoadingPlaylist { playlist_id } => format!("Loading playlist {playlist_id}"),
        Playing { kind, .. } => format!("Playing {kind:?}"),
        Skipping => "Skipping".into(),
        Transport => "Transport".into(),
        RemovingSongs { count, .. } => format!("Removing {count} songs"),
        DeletingPlaylist { playlist_name, .. } => format!("Deleting '{playlist_name}'"),
        RenamingPlaylist { playlist_name, .. } => format!("Renaming '{playlist_name}'"),
        AddingToPlaylist { playlist_id } => format!("Adding to {playlist_id}"),
        CreatingPlaylist { name } => format!("Creating '{name}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str, album: &str, dur: f32) -> Song {
        Song {
            id: id.into(),
            title: title.into(),
            artist_name: artist.into(),
            album_title: album.into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn me() -> Peer {
        Peer {
            user: "u".into(),
            host: "h".into(),
        }
    }

    #[test]
    fn awaiting_state_when_play_is_none() {
        let server = ServerState::default();
        let preview = UiPreview::default();
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert!(matches!(m.title, NowPlayingTitle::Hidden));
        assert!(matches!(m.status, NowPlayingStatus::Hidden));
        assert!(matches!(m.meta, NowPlayingMeta::Empty));
    }

    #[test]
    fn idle_when_play_is_some_but_now_playing_is_none() {
        let server = ServerState {
            play: Some(PlayState::default()),
            backend: None,
        };
        let preview = UiPreview::default();
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert!(matches!(m.title, NowPlayingTitle::Hidden));
        assert!(matches!(m.status, NowPlayingStatus::Hidden));
        assert!(matches!(m.meta, NowPlayingMeta::Empty));
    }

    #[test]
    fn now_playing_song_yields_playing_status_and_song_meta() {
        let s = song("1", "T", "A", "Al", 65.0);
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(s),
                position: 30.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: Default::default(),
            }),
            backend: None,
        };
        let preview = UiPreview::default();
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert_eq!(m.title, NowPlayingTitle::NowPlaying("T".into()));
        assert_eq!(
            m.status,
            NowPlayingStatus::Playing {
                icon: '▶',
                position: "00:30".into(),
                duration: "01:05".into(),
            }
        );
        assert_eq!(
            m.meta,
            NowPlayingMeta::Song {
                artist: "A".into(),
                album: "Al".into(),
                dim: false,
            }
        );
    }

    #[test]
    fn preview_overrides_now_playing_with_dim_styling() {
        let played = song("now", "Now", "AN", "AlN", 120.0);
        let hovered = song("hov", "Hov", "AH", "AlH", 200.0);
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(played),
                position: 30.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: Default::default(),
            }),
            backend: None,
        };
        let preview = UiPreview {
            song: Some(hovered),
            ..Default::default()
        };
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert_eq!(m.title, NowPlayingTitle::Preview("Hov".into()));
        assert_eq!(
            m.status,
            NowPlayingStatus::Preview {
                duration: "03:20".into()
            }
        );
        assert_eq!(
            m.meta,
            NowPlayingMeta::Song {
                artist: "AH".into(),
                album: "AlH".into(),
                dim: true,
            }
        );
    }

    #[test]
    fn preview_of_currently_playing_song_is_suppressed() {
        let s = song("same", "T", "A", "Al", 60.0);
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(s.clone()),
                position: 0.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: Default::default(),
            }),
            backend: None,
        };
        let preview = UiPreview {
            song: Some(s),
            ..Default::default()
        };
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        // Falls back to "now playing" — no preview override.
        assert_eq!(m.title, NowPlayingTitle::NowPlaying("T".into()));
        assert!(matches!(m.meta, NowPlayingMeta::Song { dim: false, .. }));
    }

    #[test]
    fn foreign_peer_activity_replaces_song_meta() {
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(song("1", "T", "A", "Al", 60.0)),
                position: 0.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: Default::default(),
            }),
            backend: None,
        };
        let preview = UiPreview::default();
        let mut activity = Activity::default();
        activity.started(
            7,
            Peer {
                user: "other".into(),
                host: "host2".into(),
            },
            TaskActivity::Searching {
                term: "blah".into(),
            },
            std::time::Instant::now(),
        );
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert_eq!(
            m.meta,
            NowPlayingMeta::Peer(PeerActivityFrame {
                who: "other@host2".into(),
                label: "Searching 'blah'".into(),
            })
        );
    }

    #[test]
    fn repeat_mode_propagates_into_model() {
        let s = song("1", "T", "A", "Al", 60.0);
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(s),
                position: 0.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: mkproto::RepeatMode::All,
            }),
            backend: None,
        };
        let preview = UiPreview::default();
        let activity = Activity::default();
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert_eq!(m.repeat, NowPlayingRepeat::All);
    }

    #[test]
    fn own_peer_activity_does_not_replace_song_meta() {
        let server = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: Some(song("1", "T", "A", "Al", 60.0)),
                position: 0.0,
                position_at: 0.0,
                queue_index: Some(0),
                repeat: Default::default(),
            }),
            backend: None,
        };
        let preview = UiPreview::default();
        let mut activity = Activity::default();
        activity.started(
            7,
            me(),
            TaskActivity::Searching {
                term: "blah".into(),
            },
            std::time::Instant::now(),
        );
        let m = now_playing_model(
            ServerNowPlayingInput::new(&server),
            UiPreviewInput::new(&preview),
            ActivityInput::new(&activity),
            PeerIdInput::new(&me()),
        );
        assert!(matches!(m.meta, NowPlayingMeta::Song { dim: false, .. }));
    }
}
