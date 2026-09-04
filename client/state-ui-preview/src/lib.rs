//! User-decision source: the song the user is currently hovering
//! ("preview"). The now-playing bar memo overrides the current
//! playback line with this song's title/artist/album for `ttl`
//! seconds, then it auto-clears.
//!
//! `expires_at` is wall-clock — `drop_if_expired` is called from the
//! runtime's per-tick ingest sweep so by the time view-model memos
//! run, `song.is_some()` ⇔ "preview is live".

use std::time::Instant;

use mkproto::Song;

#[derive(Debug, Clone, Default)]
pub struct UiPreview {
    pub song: Option<Song>,
    pub expires_at: Option<Instant>,
}

impl UiPreview {
    /// Set the preview song with an absolute expiry. The caller
    /// computes `expires_at = clock.now + ttl` so the runtime's
    /// notion of "now" stays consistent with every other time-
    /// sensitive source (`EXAMPLE-ARCH.md` § "Time is a source
    /// field").
    pub fn set(&mut self, song: Song, expires_at: Instant) {
        self.song = Some(song);
        self.expires_at = Some(expires_at);
    }

    pub fn clear(&mut self) {
        self.song = None;
        self.expires_at = None;
    }

    /// Drop the preview if its TTL has elapsed. Called by the runtime
    /// at tick start so memos see a single, consistent "is preview
    /// live?" signal: `song.is_some()`.
    pub fn drop_if_expired(&mut self, now: Instant) {
        if let Some(deadline) = self.expires_at {
            if now >= deadline {
                self.clear();
            }
        }
    }

    /// Earliest wall-clock the preview will need a wake at, so
    /// `nearest_deadline` can fold it into the loop's sleep budget.
    pub fn nearest_deadline(&self) -> Option<Instant> {
        self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Song {
        Song {
            id: "s1".into(),
            title: "t".into(),
            artist_name: "a".into(),
            album_title: "al".into(),
            duration: 0.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    use std::time::Duration;

    #[test]
    fn set_then_drop_when_expired() {
        let mut p = UiPreview::default();
        let t0 = Instant::now();
        p.set(song(), t0);
        p.drop_if_expired(t0 + Duration::from_millis(1));
        assert!(p.song.is_none());
        assert!(p.expires_at.is_none());
    }

    #[test]
    fn live_preview_survives_drop_if_not_expired() {
        let mut p = UiPreview::default();
        let t0 = Instant::now();
        p.set(song(), t0 + Duration::from_secs(10));
        p.drop_if_expired(t0);
        assert!(p.song.is_some());
    }

    #[test]
    fn clear_resets_both_fields() {
        let mut p = UiPreview::default();
        p.set(song(), Instant::now() + Duration::from_secs(1));
        p.clear();
        assert!(p.song.is_none());
        assert!(p.expires_at.is_none());
    }
}
