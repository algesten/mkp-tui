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
    /// Backend the rest of the sources were actually built from.
    ///
    /// The server dictates the backend and can swap it under a live
    /// link, so this is not an intent the client holds — it is the
    /// other half of a fact pair. `backend` is what the server says
    /// it is playing from; this is what we have loaded. When they
    /// disagree, everything derived from the catalogue is stale, and
    /// `backend_session_action` says so.
    pub built_from: Option<std::sync::Arc<str>>,
}
