//! Streamed artist-detail extras keyed by `artist_id`.
//!
//! The first response (`ServerMsg::ArtistDetail`) lands in the
//! responses map; the follow-up broadcasts `SimilarArtists` and
//! `ArtistAlbumsChunk` are folded here so the UI can render them
//! without re-walking the response history.

use std::sync::Arc;

use imbl::{HashMap, Vector};
use mkproto::{Album, Artist};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistDetailExtras {
    pub similar: HashMap<String, Vector<Arc<Artist>>>,
    /// Streamed paged albums for an artist (legacy: server emits
    /// `ArtistAlbumsChunk` to grow `top_albums`/`latest_albums`).
    pub paged_albums: HashMap<String, Vector<Arc<Album>>>,
}

impl ArtistDetailExtras {
    pub fn set_similar(&mut self, artist_id: String, artists: Vec<Artist>) {
        self.similar
            .insert(artist_id, artists.into_iter().map(Arc::new).collect());
    }

    pub fn append_albums(&mut self, artist_id: String, albums: Vec<Album>) {
        self.paged_albums
            .entry(artist_id)
            .or_default()
            .extend(albums.into_iter().map(Arc::new));
    }

    pub fn similar_for(&self, artist_id: &str) -> Option<&Vector<Arc<Artist>>> {
        self.similar.get(artist_id)
    }

    pub fn paged_albums_for(&self, artist_id: &str) -> Option<&Vector<Arc<Album>>> {
        self.paged_albums.get(artist_id)
    }

    pub fn clear(&mut self) {
        self.similar.clear();
        self.paged_albums.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(id: &str) -> Artist {
        Artist {
            id: id.into(),
            name: id.into(),
            detail: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn album(id: &str) -> Album {
        Album {
            id: id.into(),
            name: id.into(),
            artist_id: String::new(),
            artist_name: String::new(),
            track_count: 0,
            detail: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    #[test]
    fn similar_set_and_lookup() {
        let mut e = ArtistDetailExtras::default();
        e.set_similar("a".into(), vec![artist("x"), artist("y")]);
        assert_eq!(e.similar_for("a").map(|v| v.len()), Some(2));
        assert!(e.similar_for("missing").is_none());
    }

    #[test]
    fn paged_albums_append_keyed_by_artist() {
        let mut e = ArtistDetailExtras::default();
        e.append_albums("a".into(), vec![album("x")]);
        e.append_albums("a".into(), vec![album("y"), album("z")]);
        e.append_albums("b".into(), vec![album("w")]);
        assert_eq!(e.paged_albums_for("a").map(|v| v.len()), Some(3));
        assert_eq!(e.paged_albums_for("b").map(|v| v.len()), Some(1));
    }

    #[test]
    fn clear_drops_everything() {
        let mut e = ArtistDetailExtras::default();
        e.set_similar("a".into(), vec![artist("x")]);
        e.append_albums("a".into(), vec![album("x")]);
        e.clear();
        assert!(e.similar_for("a").is_none());
        assert!(e.paged_albums_for("a").is_none());
    }
}
