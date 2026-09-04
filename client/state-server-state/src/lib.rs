//! External-fact source: mirrored server `PlayState`.
//!
//! The link driver emits a broadcast `ServerMsg::StateUpdate(PlayState)`
//! whenever the server's playback changes. The runtime's ingest
//! phase overwrites `play` wholesale — server broadcasts are the
//! canonical truth.

use mkproto::PlayState;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerState {
    /// `None` until the first `StateUpdate` arrives post-connect.
    pub play: Option<PlayState>,
    /// Backend name as reported by the server (MusicKit / Tidal).
    /// `None` until a `BackendChanged` frame (or an equivalent) lands.
    pub backend: Option<std::sync::Arc<str>>,
}
