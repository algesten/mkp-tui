pub mod codec;
#[cfg(feature = "mdns")]
pub mod mdns;
pub mod msg;
pub mod state;
pub mod verify;

pub use codec::{decode_frame, encode_frame};
pub use msg::{
    Activity, ClientMsg, ListTarget, NavigateTarget, PairClientMsg, PairServerMsg, Peer,
    PeerActivity, PlaylistMutation, Request, Response, ServerMsg, TaskActivity, TaskId,
    PROTOCOL_VERSION,
};
pub use state::{
    Album, AlbumDetail, Artist, ArtistDetail, MediaKind, PlayState, PlaybackState, Playlist,
    QueueDelta, QueueEntry, QueueEntryId, QueuePosition, RepeatMode, SearchResults, SearchType,
    Song,
};
pub use verify::compute_pairing_code;
