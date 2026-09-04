//! External-fact source: responses keyed by request seq.
//!
//! The link driver emits a `Frame` event for every `Response` it
//! decodes off the wire. Ingest checks the seq: non-zero lands
//! here; zero is a broadcast and is routed to the source(s) that
//! mirror live server state.
//!
//! Consumers poll `by_seq` for their known seq and call `take` once
//! they've observed the entry, preventing unbounded growth when a
//! UI component fires a request and then dies.

use std::sync::Arc;

use imbl::HashMap;
use mkproto::ServerMsg;

/// External-fact source: response payloads keyed by request seq.
/// Values are `Arc`-wrapped so memo inputs can ptr-eq fast-path the
/// response (the server replies once and the entry is stable until
/// the consumer takes / disconnect clears it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Responses {
    pub by_seq: HashMap<u64, Arc<ServerMsg>>,
}

impl Responses {
    pub fn insert(&mut self, seq: u64, msg: ServerMsg) {
        self.by_seq.insert(seq, Arc::new(msg));
    }

    pub fn take(&mut self, seq: u64) -> Option<Arc<ServerMsg>> {
        self.by_seq.remove(&seq)
    }

    /// Clear everything — used on disconnect, since a pending
    /// request can never receive a reply from a vanished server.
    pub fn clear(&mut self) {
        self.by_seq.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_take_round_trips() {
        let mut r = Responses::default();
        r.insert(1, ServerMsg::Pong);
        let msg = r.take(1);
        assert!(matches!(msg.as_deref(), Some(ServerMsg::Pong)));
        assert!(r.take(1).is_none());
    }

    #[test]
    fn clear_drops_everything() {
        let mut r = Responses::default();
        r.insert(1, ServerMsg::Pong);
        r.insert(2, ServerMsg::Ok);
        r.clear();
        assert!(r.by_seq.is_empty());
    }
}
