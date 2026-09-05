//! View model for the right-hand "Queue" column.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The queue's primary
//! input is `imbl::Vector<Arc<Song>>` from `state-queue`; the Arc
//! pointer-equality fast path makes ingestion-stable rows O(1) on
//! cache-hit comparison and the memo only re-runs when an actual
//! mutation lands.
//!
//! User-decision parameters (cursor, filter, selection set, focus)
//! land on dedicated `state-ui-*` sources as part of step 5; until
//! then they ride along as plain memo args.

use std::sync::Arc;

use imbl::{OrdSet, Vector};
use mkproto::Song;

use mkpclient_state_queue::Queue;
use mkpclient_state_server_state::ServerState;

use super::util::format_duration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueueRow {
    /// Index into the original (unfiltered) queue. Used by
    /// dispatch / activation to address the right song.
    pub orig_index: usize,
    pub title: String,
    pub is_current: bool,
    pub is_multi_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueueColumnModel {
    pub focused: bool,
    /// `true` when the multi-select context targets the queue. The
    /// renderer paints the column border in selection-accent and
    /// uses the pink "❯" cursor.
    pub in_selection: bool,
    /// Whether a queue filter is active (drives the "Shift-F
    /// unfilter" title-bottom hint when focused).
    pub has_filter: bool,
    /// Pre-formatted "remaining" string for the title-bottom badge.
    /// `None` when `0:00`-equivalent — the renderer omits the badge.
    pub remaining: Option<Arc<str>>,
    /// Cursor index into [`Self::rows`].
    pub selected_filtered: usize,
    pub rows: Vector<QueueRow>,
}

#[derive(drv::Input)]
pub struct QueueInput<'a> {
    pub items: &'a Vector<Arc<Song>>,
    pub current_index: Option<usize>,
}

impl<'a> QueueInput<'a> {
    pub fn new(q: &'a Queue) -> Self {
        Self {
            items: &q.items,
            current_index: q.current_index,
        }
    }
}

#[derive(drv::Input)]
pub struct ServerPositionInput {
    pub position_secs: Option<f32>,
}

impl ServerPositionInput {
    pub fn new(s: &ServerState) -> Self {
        Self {
            position_secs: s.play.as_ref().map(|p| p.position as f32),
        }
    }
}

#[drv::memo(single)]
pub fn queue_column_model<'a>(
    queue: QueueInput<'a>,
    server: ServerPositionInput,
    queue_selected: usize,
    queue_filter: &Arc<str>,
    selection_in_queue: bool,
    selection_indices: &OrdSet<usize>,
    focused: bool,
) -> QueueColumnModel {
    let elapsed = server.position_secs.unwrap_or(0.0);
    let remaining = compute_remaining(queue.items, queue.current_index, elapsed);

    let filter_lower = queue_filter.to_lowercase();
    let has_filter = !filter_lower.is_empty();

    // Track the filter-space position of the now-playing track so
    // that, when the pane is unfocused, the rendered cursor follows
    // `current_index` (legacy mkp2 parity, app/server.rs:222-228).
    // Per `EXAMPLE-ARCH.md` § "Queries: desired state, not
    // transitions" this rule lives here as a query rather than as
    // an ingest-time observer mutating `cursor.queue`.
    let mut current_filter_pos: Option<usize> = None;
    let mut rows: Vector<QueueRow> = Vector::new();
    for (orig_index, song) in queue.items.iter().enumerate() {
        if has_filter
            && !song.title.to_lowercase().contains(&filter_lower)
            && !song.artist_name.to_lowercase().contains(&filter_lower)
            && !song.album_title.to_lowercase().contains(&filter_lower)
        {
            // Legacy parity: queue filter matches title OR artist OR
            // album (mkp2/.../filter_input.rs::apply_queue_filter).
            continue;
        }
        let row_pos = rows.len();
        let is_current = queue.current_index == Some(orig_index);
        if is_current {
            current_filter_pos = Some(row_pos);
        }
        rows.push_back(QueueRow {
            orig_index,
            title: song.title.clone(),
            is_current,
            is_multi_selected: selection_in_queue && selection_indices.contains(&orig_index),
        });
    }

    let selected_filtered = if focused {
        queue_selected
    } else {
        current_filter_pos.unwrap_or(0)
    };

    QueueColumnModel {
        focused,
        in_selection: selection_in_queue,
        has_filter,
        remaining,
        selected_filtered,
        rows,
    }
}

fn compute_remaining(
    items: &Vector<Arc<Song>>,
    current_index: Option<usize>,
    elapsed: f32,
) -> Option<Arc<str>> {
    let from = current_index.unwrap_or(0);
    let mut total = 0.0f32;
    if let Some(song) = items.get(from) {
        let leftover = song.duration - elapsed;
        if leftover > 0.0 {
            total += leftover;
        }
    }
    for song in items.iter().skip(from + 1) {
        total += song.duration;
    }
    if total > 0.0 {
        Some(Arc::from(format_duration(total)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use imbl::OrdSet;

    use mkpclient_state_queue::Queue;
    use mkpclient_state_server_state::ServerState;
    use mkproto::{PlayState, PlaybackState, Song};

    fn song(id: &str, title: &str, dur: f32) -> Song {
        Song {
            id: id.into(),
            title: title.into(),
            artist_name: "".into(),
            album_title: "".into(),
            duration: dur,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn build(
        songs: Vec<Song>,
        current_index: Option<usize>,
        position: f32,
    ) -> (Queue, ServerState) {
        let mut q = Queue::default();
        for s in songs {
            q.items.push_back(Arc::new(s));
        }
        q.current_index = current_index;
        let s = ServerState {
            play: Some(PlayState {
                playback: PlaybackState::Playing,
                now_playing: None,
                position: position as f64,
                position_at: 0.0,
                queue_index: current_index,
                repeat: Default::default(),
            }),
            backend: None,
            built_from: None,
        };
        (q, s)
    }

    fn run(
        q: &Queue,
        s: &ServerState,
        sel_idx: usize,
        filter: &str,
        sel_in_q: bool,
        sel: &OrdSet<usize>,
        focused: bool,
    ) -> QueueColumnModel {
        let filter_arc: Arc<str> = Arc::from(filter);
        queue_column_model(
            QueueInput::new(q),
            ServerPositionInput::new(s),
            sel_idx,
            &filter_arc,
            sel_in_q,
            sel,
            focused,
        )
    }

    #[test]
    fn empty_queue_yields_empty_model() {
        let q = Queue::default();
        let s = ServerState::default();
        let m = run(&q, &s, 0, "", false, &OrdSet::new(), false);
        assert!(m.rows.is_empty());
        assert_eq!(m.remaining, None);
        assert!(!m.in_selection);
        assert!(!m.focused);
    }

    #[test]
    fn three_songs_no_filter_yields_three_rows_in_order() {
        let songs = vec![
            song("1", "Alpha", 60.0),
            song("2", "Beta", 90.0),
            song("3", "Gamma", 30.0),
        ];
        let (q, s) = build(songs, Some(0), 0.0);
        let m = run(&q, &s, 1, "", false, &OrdSet::new(), true);
        assert_eq!(m.rows.len(), 3);
        assert!(m.rows[0].is_current);
        assert!(!m.rows[1].is_current);
        assert_eq!(m.rows[2].title, "Gamma");
        // remaining = 60 (current full) + 90 + 30 = 180
        assert_eq!(m.remaining, Some("03:00".into()));
        assert!(m.focused);
    }

    #[test]
    fn elapsed_subtracts_from_current_song_duration() {
        let songs = vec![song("1", "Alpha", 60.0), song("2", "Beta", 30.0)];
        let (q, s) = build(songs, Some(0), 20.0);
        let m = run(&q, &s, 0, "", false, &OrdSet::new(), false);
        // remaining = (60 - 20) + 30 = 70
        assert_eq!(m.remaining, Some("01:10".into()));
    }

    #[test]
    fn filter_drops_non_matching_titles_case_insensitively() {
        let songs = vec![
            song("1", "Bohemian Rhapsody", 100.0),
            song("2", "Stairway", 50.0),
            song("3", "Boulevard", 70.0),
        ];
        let (q, s) = build(songs, None, 0.0);
        let m = run(&q, &s, 0, "BO", false, &OrdSet::new(), false);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.rows[0].title, "Bohemian Rhapsody");
        assert_eq!(m.rows[0].orig_index, 0);
        assert_eq!(m.rows[1].title, "Boulevard");
        assert_eq!(m.rows[1].orig_index, 2);
    }

    #[test]
    fn multi_selection_marks_rows_when_context_is_queue() {
        let songs = vec![
            song("1", "A", 0.0),
            song("2", "B", 0.0),
            song("3", "C", 0.0),
        ];
        let (q, s) = build(songs, None, 0.0);
        let mut sel = OrdSet::new();
        sel.insert(0);
        sel.insert(2);
        let m = run(&q, &s, 0, "", true, &sel, false);
        assert!(m.rows[0].is_multi_selected);
        assert!(!m.rows[1].is_multi_selected);
        assert!(m.rows[2].is_multi_selected);
    }

    #[test]
    fn multi_selection_not_marked_when_context_is_not_queue() {
        let songs = vec![song("1", "A", 0.0), song("2", "B", 0.0)];
        let (q, s) = build(songs, None, 0.0);
        let mut sel = OrdSet::new();
        sel.insert(0);
        sel.insert(1);
        let m = run(&q, &s, 0, "", false, &sel, false);
        assert!(m.rows.iter().all(|r| !r.is_multi_selected));
    }

    #[test]
    fn remaining_zero_yields_none() {
        let (q, s) = build(vec![song("1", "A", 0.0)], Some(0), 0.0);
        let m = run(&q, &s, 0, "", false, &OrdSet::new(), false);
        assert_eq!(m.remaining, None);
    }

    #[test]
    fn unfocused_selected_follows_current_index_in_filter_space() {
        let songs = vec![
            song("1", "Alpha", 60.0),
            song("2", "Beta", 90.0),
            song("3", "Gamma", 30.0),
        ];
        let (q, s) = build(songs, Some(2), 0.0);
        // Stale `queue_selected = 0` is ignored when unfocused; the
        // model derives the cursor from `current_index = 2`.
        let m = run(&q, &s, 0, "", false, &OrdSet::new(), false);
        assert_eq!(m.selected_filtered, 2);
    }

    #[test]
    fn focused_selected_uses_user_cursor_not_current() {
        let songs = vec![
            song("1", "Alpha", 60.0),
            song("2", "Beta", 90.0),
            song("3", "Gamma", 30.0),
        ];
        let (q, s) = build(songs, Some(2), 0.0);
        // Focused: respects the user's manual cursor.
        let m = run(&q, &s, 1, "", true, &OrdSet::new(), true);
        assert_eq!(m.selected_filtered, 1);
    }

    #[test]
    fn unfocused_selected_translates_into_filter_space() {
        let songs = vec![
            song("1", "Bohemian Rhapsody", 100.0),
            song("2", "Stairway", 50.0),
            song("3", "Boulevard", 70.0),
        ];
        // current_index = 2 (Boulevard) — in the filtered list it
        // sits at row 1 (Bohemian is row 0; Stairway is filtered out).
        let (q, s) = build(songs, Some(2), 0.0);
        let m = run(&q, &s, 99, "BO", false, &OrdSet::new(), false);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.selected_filtered, 1);
    }

    #[test]
    fn unfocused_with_filtered_out_current_falls_back_to_zero() {
        let songs = vec![song("1", "Alpha", 0.0), song("2", "Beta", 0.0)];
        // current_index = 1 (Beta) — filter "alp" excludes it, so
        // `current_filter_pos` is None and the cursor lands on row 0.
        let (q, s) = build(songs, Some(1), 0.0);
        let m = run(&q, &s, 99, "alp", false, &OrdSet::new(), false);
        assert_eq!(m.selected_filtered, 0);
    }
}
