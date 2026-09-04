//! External-fact source: mirrored playback queue.
//!
//! The server sends `QueueDelta` broadcasts per change plus a
//! `QueueCatchUp` bulk bundle on reconnect. Each carries a
//! monotonic `(queue_id, version)` pair: when `queue_id` changes
//! the queue has been replaced; when `version` jumps non-
//! contiguously we've missed deltas and should expect a catch-up.
//!
//! The ingest phase applies deltas in order. Consumers read the
//! `items` vector plus `current_index` to render a "now / next"
//! list.

use std::sync::Arc;

use imbl::Vector;
use mkproto::{QueueDelta, QueueEntry, QueueEntryId, Song};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    /// Opaque per-session id. Resets when the server starts a new
    /// queue — we drop everything and start over.
    pub queue_id: Option<u64>,
    /// Last applied delta version. Used to detect missing deltas.
    pub version: u64,
    /// The queue itself. `Arc<Song>` keeps cache-hit clones O(1)
    /// (refcount bump) and lets the drv pointer-equality fast path
    /// short-circuit per-row comparison after ingestion-stable rows.
    pub items: Vector<Arc<Song>>,
    /// Stable server-assigned identifiers aligned with `items`.
    pub entry_ids: Vector<QueueEntryId>,
    /// Index into `items` of the currently-playing entry, if any.
    pub current_index: Option<usize>,
    /// Server hint for how long the queue will grow to. UI uses it
    /// to size scrollbars / show loading progress.
    pub expected_total: Option<usize>,
}

impl Queue {
    /// Reset to an empty queue under a new `queue_id`. Called by the
    /// ingest phase when a broadcast carries a new `queue_id`.
    pub fn reset(&mut self, queue_id: u64) {
        self.queue_id = Some(queue_id);
        self.version = 0;
        self.items.clear();
        self.entry_ids.clear();
        self.current_index = None;
        self.expected_total = None;
    }

    /// Apply one delta. Silently no-ops on out-of-bounds indexes —
    /// a following `QueueCatchUp` is expected to resync.
    pub fn apply(&mut self, delta: QueueDelta) {
        match delta {
            QueueDelta::Insert { index, entry } | QueueDelta::InsertPending { index, entry } => {
                if index <= self.items.len() {
                    self.items.insert(index, Arc::new(entry.song));
                    self.entry_ids.insert(index, entry.id);
                    if let Some(ci) = self.current_index {
                        if index <= ci {
                            self.current_index = Some(ci + 1);
                        }
                    }
                }
            }
            QueueDelta::Remove { index } => {
                if index < self.items.len() {
                    self.items.remove(index);
                    self.entry_ids.remove(index);
                    match self.current_index {
                        Some(ci) if ci == index => self.current_index = None,
                        Some(ci) if ci > index => self.current_index = Some(ci - 1),
                        _ => {}
                    }
                }
            }
            QueueDelta::SetIndex { index } => {
                self.current_index = index.filter(|index| *index < self.items.len());
            }
            QueueDelta::Resolve { id: _ } => {
                // `InsertPending` → `Insert` transition hint. We
                // don't track the pending/resolved split (both land
                // in `items` as ordinary songs above).
            }
            QueueDelta::ExpectedTotal { total } => {
                self.expected_total = Some(total);
            }
        }
    }

    pub fn chunk(&mut self, offset: usize, entries: Vec<QueueEntry>) {
        for (i, entry) in entries.into_iter().enumerate() {
            let index = offset + i;
            if index == self.items.len() {
                self.items.push_back(Arc::new(entry.song));
                self.entry_ids.push_back(entry.id);
            } else if index < self.items.len() {
                self.items[index] = Arc::new(entry.song);
                self.entry_ids[index] = entry.id;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mkproto::{QueueDelta, QueueEntry, Song};

    use super::Queue;

    fn song(id: &str) -> Arc<Song> {
        Arc::new(Song {
            id: id.to_string(),
            title: id.to_string(),
            artist_name: String::new(),
            album_title: String::new(),
            duration: 0.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        })
    }

    #[test]
    fn backend_can_clear_a_stale_current_index() {
        let mut queue = Queue::default();
        queue.items.push_back(song("a"));
        queue.current_index = Some(0);

        queue.apply(QueueDelta::SetIndex { index: None });

        assert_eq!(queue.current_index, None);
    }

    #[test]
    fn out_of_bounds_backend_index_is_not_exposed() {
        let mut queue = Queue::default();
        queue.items.push_back(song("a"));

        queue.apply(QueueDelta::SetIndex { index: Some(1) });

        assert_eq!(queue.current_index, None);
    }

    #[test]
    fn queue_entry_ids_stay_aligned_with_songs() {
        let mut queue = Queue::default();
        queue.chunk(
            0,
            vec![
                QueueEntry {
                    id: 10,
                    song: (*song("a")).clone(),
                },
                QueueEntry {
                    id: 11,
                    song: (*song("b")).clone(),
                },
            ],
        );
        queue.apply(QueueDelta::Insert {
            index: 1,
            entry: QueueEntry {
                id: 12,
                song: (*song("c")).clone(),
            },
        });
        queue.apply(QueueDelta::Remove { index: 0 });

        assert_eq!(
            queue
                .items
                .iter()
                .map(|song| song.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "b"]
        );
        assert_eq!(
            queue.entry_ids.iter().copied().collect::<Vec<_>>(),
            [12, 11]
        );
    }
}
