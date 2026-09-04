//! View model for the bottom "Selection" bar shown while
//! multi-select is active.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The bar projects the
//! queue and playlist-tracks song durations through narrow inputs;
//! both sources hold `Arc<Song>` so cache-hit checks ptr-eq the
//! underlying allocations.

use std::sync::Arc;

use imbl::{OrdSet, Vector};
use mkproto::Song;

use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_queue::Queue;

use super::util::format_duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, drv::Input)]
pub enum SelectionBarContext {
    Middle,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SelectionBarModel {
    pub count: usize,
    /// Pre-formatted "{count} selected • {duration}" or "{count}
    /// selected" when total is zero. `None` when count is 0 (the
    /// renderer omits the right-hand info entirely).
    pub info: Option<Arc<str>>,
}

#[derive(drv::Input)]
pub struct SelectionBarSongsInput<'a> {
    pub queue_items: &'a Vector<Arc<Song>>,
    pub playlist_songs: &'a Vector<Option<Arc<Song>>>,
}

impl<'a> SelectionBarSongsInput<'a> {
    pub fn new(q: &'a Queue, t: &'a PlaylistTracks) -> Self {
        Self {
            queue_items: &q.items,
            playlist_songs: &t.songs,
        }
    }
}

#[drv::memo(single)]
pub fn selection_bar_model<'a>(
    context: SelectionBarContext,
    selected: &OrdSet<usize>,
    songs: SelectionBarSongsInput<'a>,
    is_playlist_songs: bool,
) -> SelectionBarModel {
    let count = selected.len();
    if count == 0 {
        return SelectionBarModel {
            count: 0,
            info: None,
        };
    }

    let total_secs: f32 = match context {
        SelectionBarContext::Middle if is_playlist_songs => selected
            .iter()
            .filter_map(|i| songs.playlist_songs.get(*i))
            .filter_map(|s| s.as_ref())
            .map(|s| s.duration)
            .sum(),
        SelectionBarContext::Middle => 0.0,
        SelectionBarContext::Queue => selected
            .iter()
            .filter_map(|i| songs.queue_items.iter().nth(*i))
            .map(|s| s.duration)
            .sum(),
    };

    let info = if total_secs > 0.0 {
        format!("{count} selected \u{2022} {}", format_duration(total_secs))
    } else {
        format!("{count} selected")
    };
    SelectionBarModel {
        count,
        info: Some(Arc::from(info)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mkproto::Song;

    fn s(dur: f32) -> Song {
        Song {
            id: "x".into(),
            title: "t".into(),
            artist_name: "".into(),
            album_title: "".into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn run(
        ctx: SelectionBarContext,
        sel: &OrdSet<usize>,
        q: &Queue,
        pt: &PlaylistTracks,
        is_pl: bool,
    ) -> SelectionBarModel {
        selection_bar_model(ctx, sel, SelectionBarSongsInput::new(q, pt), is_pl)
    }

    #[test]
    fn empty_selection_yields_no_info() {
        let q = Queue::default();
        let pt = PlaylistTracks::default();
        let m = run(
            SelectionBarContext::Queue,
            &OrdSet::<usize>::new(),
            &q,
            &pt,
            false,
        );
        assert_eq!(m.count, 0);
        assert_eq!(m.info, None);
    }

    #[test]
    fn queue_selection_sums_durations() {
        let mut q = Queue::default();
        q.items.push_back(Arc::new(s(60.0)));
        q.items.push_back(Arc::new(s(120.0)));
        q.items.push_back(Arc::new(s(30.0)));
        let pt = PlaylistTracks::default();
        let mut sel = OrdSet::new();
        sel.insert(0);
        sel.insert(2);
        let m = run(SelectionBarContext::Queue, &sel, &q, &pt, false);
        // 60 + 30 = 90s = 01:30
        assert_eq!(m.info.as_deref(), Some("2 selected \u{2022} 01:30"));
    }

    #[test]
    fn middle_non_playlist_songs_yields_count_only() {
        let q = Queue::default();
        let pt = PlaylistTracks::default();
        let mut sel = OrdSet::new();
        sel.insert(0);
        sel.insert(1);
        let m = run(SelectionBarContext::Middle, &sel, &q, &pt, false);
        assert_eq!(m.info.as_deref(), Some("2 selected"));
    }
}
