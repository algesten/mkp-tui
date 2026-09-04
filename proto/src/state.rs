use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SearchType {
    Artist,
    #[default]
    Song,
    Album,
}

impl SearchType {
    pub fn next(self) -> Self {
        match self {
            SearchType::Song => SearchType::Artist,
            SearchType::Artist => SearchType::Album,
            SearchType::Album => SearchType::Song,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            SearchType::Song => SearchType::Album,
            SearchType::Artist => SearchType::Song,
            SearchType::Album => SearchType::Artist,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SearchResults {
    Songs { songs: Vec<Song> },
    Albums { albums: Vec<Album> },
    Artists { artists: Vec<Artist> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist_id: String,
    pub artist_name: String,
    pub track_count: usize,
    pub detail: Option<AlbumDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_small: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_large: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub release_date: Option<String>,
    pub record_label: Option<String>,
    pub editorial_notes_short: Option<String>,
    pub editorial_notes_long: Option<String>,
    pub copyright: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtistDetail {
    pub editorial_notes_short: Option<String>,
    pub top_albums: Vec<Album>,
    pub latest_albums: Vec<Album>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub detail: Option<ArtistDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_small: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_large: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist_name: String,
    pub album_title: String,
    pub duration: f32,
    pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_small: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url_large: Option<String>,
}

pub type QueueEntryId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub song: Song,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: String,
    pub track_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
    Loading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Song,
    Album,
    Playlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePosition {
    Reset,
    Shuffle,
    Next,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum QueueDelta {
    Insert { index: usize, entry: QueueEntry },
    InsertPending { index: usize, entry: QueueEntry },
    Remove { index: usize },
    SetIndex { index: Option<usize> },
    Resolve { id: String },
    ExpectedTotal { total: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayState {
    pub playback: PlaybackState,
    pub now_playing: Option<Song>,
    pub position: f64,
    pub position_at: f64,
    pub queue_index: Option<usize>,
    pub repeat: RepeatMode,
}

impl PlayState {
    pub fn set_position(&mut self, pos: f64) {
        self.position = pos;
        self.position_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
    }
}
