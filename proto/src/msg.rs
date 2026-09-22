use serde::{Deserialize, Serialize};

use crate::state::{
    Album, Artist, MediaKind, PlayState, Playlist, QueueDelta, QueueEntry, QueueEntryId,
    QueuePosition, RepeatMode, SearchResults, SearchType, Song,
};

pub const PROTOCOL_VERSION: u64 = 4;

pub type TaskId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigateTarget {
    Album,
    Artist,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ListTarget {
    Queue { queue_id: u64, version: u64 },
    Playlist { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Peer {
    pub user: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Activity {
    Searching,
    Idle,
}

#[derive(Debug, Clone)]
pub struct PeerActivity {
    pub peer: Peer,
    pub activity: Activity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TaskActivity {
    Searching { term: String },
    BrowsingAlbum { id: String },
    BrowsingArtist { id: String },
    Navigating { target: NavigateTarget },
    LoadingPlaylists,
    LoadingPlaylist { playlist_id: String },
    Playing { id: String, kind: MediaKind },
    Skipping,
    Transport,
    RemovingSongs { playlist_id: String, count: usize },
    DeletingPlaylist { id: String, playlist_name: String },
    RenamingPlaylist { id: String, playlist_name: String },
    AddingToPlaylist { playlist_id: String },
    CreatingPlaylist { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PlaylistMutation {
    SongAdded { songs: Vec<Song> },
    SongRemoved { song_id: String, index: usize },
    Deleted,
    Renamed { new_name: String },
    Modified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMsg {
    Hello {
        peer: Peer,
        version: u64,
    },
    Search {
        term: String,
        search_type: SearchType,
    },
    GetAlbumDetail {
        id: String,
    },
    GetArtistDetail {
        id: String,
    },
    Play {
        id: String,
        kind: MediaKind,
        position: QueuePosition,
        start_index: Option<usize>,
    },
    SetRepeat {
        mode: RepeatMode,
    },
    SetPaused {
        paused: bool,
    },
    Skip,
    Previous,
    Seek {
        position: f64,
    },
    SeekRelative {
        offset: f64,
    },
    SkipToQueueEntry {
        queue_id: u64,
        entry_id: QueueEntryId,
    },
    RemoveFromQueue {
        queue_id: u64,
        entry_id: QueueEntryId,
    },
    GetQueueSince {
        queue_id: u64,
        version: u64,
        focus: usize,
    },
    GetPlaylists,
    GetPlaylist {
        id: String,
        focus: usize,
    },
    /// Refresh interest when the viewed playlist changes; renew while visible.
    /// An empty ID releases interest without changing the wire message shape.
    ViewingPlaylist {
        id: String,
    },
    Navigate {
        target: NavigateTarget,
        song_id: String,
    },
    AddToPlaylist {
        playlist_id: String,
        song_ids: Vec<String>,
        album_ids: Vec<String>,
    },
    PlaySongs {
        song_ids: Vec<String>,
        position: QueuePosition,
    },
    RemoveFromPlaylist {
        playlist_id: String,
        items: Vec<(String, usize)>,
    },
    CreatePlaylist {
        name: String,
    },
    DeletePlaylist {
        id: String,
    },
    RenamePlaylist {
        id: String,
        name: String,
    },
    GetState,
    Ping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMsg {
    Search(SearchResults),
    SearchMore(SearchResults),
    AlbumDetail {
        album: Album,
        songs: Vec<Song>,
    },
    ArtistDetail {
        artist: Artist,
        top_songs: Vec<Song>,
    },
    StateUpdate(PlayState),
    ListBegin {
        target: ListTarget,
        total: usize,
        focus: usize,
    },
    ListChunk {
        target: ListTarget,
        offset: usize,
        songs: Vec<Song>,
    },
    QueueChunk {
        queue_id: u64,
        offset: usize,
        entries: Vec<QueueEntry>,
    },
    QueueDelta {
        queue_id: u64,
        version: u64,
        delta: QueueDelta,
    },
    QueueCatchUp {
        queue_id: u64,
        deltas: Vec<(u64, QueueDelta)>,
    },
    Playlists {
        playlists: Vec<Playlist>,
    },
    PlaylistTrackCount {
        playlist_id: String,
        track_count: usize,
    },
    ArtistAlbumsChunk {
        artist_id: String,
        albums: Vec<Album>,
    },
    SimilarArtists {
        artist_id: String,
        artists: Vec<Artist>,
    },
    Activity {
        peer: Peer,
        activity: Activity,
    },
    BackendChanged {
        backend: String,
    },
    PlaylistMutated {
        playlist_id: String,
        mutation: PlaylistMutation,
    },
    PlaylistCreated {
        playlist: Playlist,
    },
    TaskStarted {
        task_id: TaskId,
        peer: Peer,
        activity: TaskActivity,
    },
    TaskCompleted {
        task_id: TaskId,
    },
    TaskFailed {
        task_id: TaskId,
        message: String,
    },
    ServerShutdown,
    Ok,
    Error {
        message: String,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PairClientMsg {
    PairRequest { csr_pem: String },
    PairConfirm,
    PairReject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PairServerMsg {
    PairResponse { client_cert_pem: String },
    PairError { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub msg: ClientMsg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub msg: ServerMsg,
}

impl ClientMsg {
    /// Payload-free message name for diagnostic correlation.
    pub fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "Hello",
            Self::Search { .. } => "Search",
            Self::GetAlbumDetail { .. } => "GetAlbumDetail",
            Self::GetArtistDetail { .. } => "GetArtistDetail",
            Self::Play { .. } => "Play",
            Self::SetRepeat { .. } => "SetRepeat",
            Self::SetPaused { .. } => "SetPaused",
            Self::Skip => "Skip",
            Self::Previous => "Previous",
            Self::Seek { .. } => "Seek",
            Self::SeekRelative { .. } => "SeekRelative",
            Self::SkipToQueueEntry { .. } => "SkipToQueueEntry",
            Self::RemoveFromQueue { .. } => "RemoveFromQueue",
            Self::GetQueueSince { .. } => "GetQueueSince",
            Self::GetPlaylists => "GetPlaylists",
            Self::GetPlaylist { .. } => "GetPlaylist",
            Self::ViewingPlaylist { .. } => "ViewingPlaylist",
            Self::Navigate { .. } => "Navigate",
            Self::AddToPlaylist { .. } => "AddToPlaylist",
            Self::PlaySongs { .. } => "PlaySongs",
            Self::RemoveFromPlaylist { .. } => "RemoveFromPlaylist",
            Self::CreatePlaylist { .. } => "CreatePlaylist",
            Self::DeletePlaylist { .. } => "DeletePlaylist",
            Self::RenamePlaylist { .. } => "RenamePlaylist",
            Self::GetState => "GetState",
            Self::Ping => "Ping",
        }
    }
}

impl ServerMsg {
    /// Payload-free message name for diagnostic correlation.
    pub fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::Search(..) => "Search",
            Self::SearchMore(..) => "SearchMore",
            Self::AlbumDetail { .. } => "AlbumDetail",
            Self::ArtistDetail { .. } => "ArtistDetail",
            Self::StateUpdate(..) => "StateUpdate",
            Self::ListBegin { .. } => "ListBegin",
            Self::ListChunk { .. } => "ListChunk",
            Self::QueueChunk { .. } => "QueueChunk",
            Self::QueueDelta { .. } => "QueueDelta",
            Self::QueueCatchUp { .. } => "QueueCatchUp",
            Self::Playlists { .. } => "Playlists",
            Self::PlaylistTrackCount { .. } => "PlaylistTrackCount",
            Self::ArtistAlbumsChunk { .. } => "ArtistAlbumsChunk",
            Self::SimilarArtists { .. } => "SimilarArtists",
            Self::Activity { .. } => "Activity",
            Self::BackendChanged { .. } => "BackendChanged",
            Self::PlaylistMutated { .. } => "PlaylistMutated",
            Self::PlaylistCreated { .. } => "PlaylistCreated",
            Self::TaskStarted { .. } => "TaskStarted",
            Self::TaskCompleted { .. } => "TaskCompleted",
            Self::TaskFailed { .. } => "TaskFailed",
            Self::ServerShutdown => "ServerShutdown",
            Self::Ok => "Ok",
            Self::Error { .. } => "Error",
            Self::Pong => "Pong",
        }
    }
}
