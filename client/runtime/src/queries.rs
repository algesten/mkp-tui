//! Read-only queries over `Sources`. Used by dispatch handlers
//! (which need to look up the current row's song id, the filtered
//! row count, etc.) and by render-layer modal helpers.
//!
//! These are *not* memos — they materialise small `Vec`s on every
//! call and aren't intended to feed the diffed `ViewBridge` path.
//! Memo view-models in `views/` already cover that. These helpers
//! exist so the same screen-routing logic can answer questions like
//! "how many rows are visible right now?" without re-deriving the
//! filter projection at every call site.

use mkproto::{SearchType, ServerMsg};

use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_screen::{ActionItem, ActionKind, ActionOrigin};
use mkpclient_state_ui_selection::SelectionContext;

use crate::sources::Sources;

/// One navigable item in the artist detail view. Maps a flat
/// index into the concatenated list (top_songs + top_albums +
/// latest_albums + paged + similar) to the underlying domain
/// object so cursor activations dispatch the right thing.
pub enum ArtistDetailItem {
    Song(mkproto::Song),
    Album(mkproto::Album),
    Artist(mkproto::Artist),
}

pub fn search_type_str(s: SearchType) -> &'static str {
    match s {
        SearchType::Song => "song",
        SearchType::Album => "album",
        SearchType::Artist => "artist",
    }
}

pub fn parse_search_type(s: &str) -> SearchType {
    match s {
        "album" => SearchType::Album,
        "artist" => SearchType::Artist,
        _ => SearchType::Song,
    }
}

pub fn action_item_for_song(song: &mkproto::Song) -> ActionItem {
    // Songs in the wire protocol carry no album_id / artist_id —
    // Go-to-Album / Go-to-Artist resolves via ClientMsg::Navigate
    // (the server looks up the ids). We do hold the names on the
    // Song row so the title bar can render immediately while the
    // detail response is in flight.
    ActionItem::new(song.id.clone(), ActionKind::Song, song.title.clone())
        .with_url(song.url.clone())
        .with_album(None, Some(song.album_title.clone()))
        .with_artist(None, Some(song.artist_name.clone()))
}

pub fn album_detail_songs(
    awaiting_seq: Option<u64>,
    sources: &Sources,
) -> Option<&[mkproto::Song]> {
    let seq = awaiting_seq?;
    let resp = sources.responses.by_seq.get(&seq)?;
    match &**resp {
        ServerMsg::AlbumDetail { songs, .. } => Some(songs.as_slice()),
        _ => None,
    }
}

pub fn artist_detail_songs(
    awaiting_seq: Option<u64>,
    sources: &Sources,
) -> Option<&[mkproto::Song]> {
    let seq = awaiting_seq?;
    let resp = sources.responses.by_seq.get(&seq)?;
    match &**resp {
        ServerMsg::ArtistDetail { top_songs, .. } => Some(top_songs.as_slice()),
        _ => None,
    }
}

/// (artist, top_songs, top_albums, latest_albums) — unpacked once
/// per call by `artist_detail_*` helpers. Returned by value because
/// the borrow comes from `Arc<ServerMsg>` inside the response cache,
/// which we can't lend out without leaking `Arc` lifetimes through
/// the query API.
type ArtistDetailParts = (
    mkproto::Artist,
    Vec<mkproto::Song>,
    Vec<mkproto::Album>,
    Vec<mkproto::Album>,
);

fn artist_detail_parts(awaiting_seq: Option<u64>, sources: &Sources) -> Option<ArtistDetailParts> {
    let seq = awaiting_seq?;
    let resp = sources.responses.by_seq.get(&seq)?;
    match &**resp {
        ServerMsg::ArtistDetail { artist, top_songs } => {
            let detail = artist.detail.clone().unwrap_or(mkproto::ArtistDetail {
                editorial_notes_short: None,
                top_albums: vec![],
                latest_albums: vec![],
            });
            Some((
                artist.clone(),
                top_songs.clone(),
                detail.top_albums,
                detail.latest_albums,
            ))
        }
        _ => None,
    }
}

pub fn artist_detail_total(awaiting_seq: Option<u64>, sources: &Sources) -> usize {
    let Some((artist, top_songs, top_albums, latest_albums)) =
        artist_detail_parts(awaiting_seq, sources)
    else {
        return 0;
    };
    let similar = sources
        .artist_extras
        .similar_for(&artist.id)
        .map(|v| v.len())
        .unwrap_or(0);
    let paged = sources
        .artist_extras
        .paged_albums_for(&artist.id)
        .map(|v| v.len())
        .unwrap_or(0);
    top_songs.len() + top_albums.len() + latest_albums.len() + paged + similar
}

pub fn artist_detail_item(
    awaiting_seq: Option<u64>,
    sources: &Sources,
    idx: usize,
) -> Option<ArtistDetailItem> {
    let (artist, top_songs, top_albums, latest_albums) =
        artist_detail_parts(awaiting_seq, sources)?;
    let mut cursor = idx;
    if cursor < top_songs.len() {
        return Some(ArtistDetailItem::Song(top_songs[cursor].clone()));
    }
    cursor -= top_songs.len();
    if cursor < top_albums.len() {
        return Some(ArtistDetailItem::Album(top_albums[cursor].clone()));
    }
    cursor -= top_albums.len();
    if cursor < latest_albums.len() {
        return Some(ArtistDetailItem::Album(latest_albums[cursor].clone()));
    }
    cursor -= latest_albums.len();
    let paged = sources
        .artist_extras
        .paged_albums_for(&artist.id)
        .cloned()
        .unwrap_or_default();
    if cursor < paged.len() {
        return Some(ArtistDetailItem::Album((*paged[cursor]).clone()));
    }
    cursor -= paged.len();
    let similar = sources
        .artist_extras
        .similar_for(&artist.id)
        .cloned()
        .unwrap_or_default();
    similar
        .get(cursor)
        .map(|a| ArtistDetailItem::Artist((**a).clone()))
}

/// Indices (into the underlying data vector) of the rows that pass
/// the middle-pane filter. When no filter is set, returns `0..len`.
pub fn middle_filtered_indices(sources: &Sources) -> Vec<usize> {
    let filter = sources.filter.middle.to_lowercase();
    let all_match = filter.is_empty();
    match &sources.history.mode {
        MiddleMode::PlaylistSongs => {
            let loaded_id = sources.playlist_tracks.playlist_id.as_deref().unwrap_or("");
            sources
                .playlist_tracks
                .songs
                .iter()
                .enumerate()
                .filter(|(_, slot)| {
                    // Optimistic remove: a pending-remove song id is
                    // treated as already gone from the cursor space.
                    if let Some(s) = slot.as_ref() {
                        if sources.pending_playlists.is_removing_song(loaded_id, &s.id) {
                            return false;
                        }
                    }
                    match slot {
                        Some(s) if !all_match => {
                            s.title.to_lowercase().contains(&filter)
                                || s.artist_name.to_lowercase().contains(&filter)
                                || s.album_title.to_lowercase().contains(&filter)
                        }
                        _ => all_match || slot.is_some(),
                    }
                })
                .map(|(i, _)| i)
                .collect()
        }
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            let Some(songs) = album_detail_songs(*awaiting_seq, sources) else {
                return vec![];
            };
            songs
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    all_match
                        || s.title.to_lowercase().contains(&filter)
                        || s.artist_name.to_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect()
        }
        MiddleMode::ArtistDetail { awaiting_seq, .. } => {
            // Artist detail flattens five sub-lists into one nav
            // list; filter is rarely useful here so legacy ignores
            // it and returns every index.
            (0..artist_detail_total(*awaiting_seq, sources)).collect()
        }
        MiddleMode::SearchResults { search_type, .. } => match search_type {
            SearchType::Song => sources
                .search
                .songs
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    all_match
                        || s.title.to_lowercase().contains(&filter)
                        || s.artist_name.to_lowercase().contains(&filter)
                        || s.album_title.to_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect(),
            SearchType::Album => sources
                .search
                .albums
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    all_match
                        || a.name.to_lowercase().contains(&filter)
                        || a.artist_name.to_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect(),
            SearchType::Artist => sources
                .search
                .artists
                .iter()
                .enumerate()
                .filter(|(_, a)| all_match || a.name.to_lowercase().contains(&filter))
                .map(|(i, _)| i)
                .collect(),
        },
    }
}

pub fn queue_filtered_indices(sources: &Sources) -> Vec<usize> {
    let filter = sources.filter.queue.to_lowercase();
    let all = filter.is_empty();
    sources
        .queue
        .items
        .iter()
        .enumerate()
        .filter(|(_, s)| all || s.title.to_lowercase().contains(&filter))
        .map(|(i, _)| i)
        .collect()
}

pub fn middle_row_count(sources: &Sources) -> usize {
    middle_filtered_indices(sources).len()
}

pub fn hovered_queue_song(sources: &Sources) -> Option<mkproto::Song> {
    let orig = *queue_filtered_indices(sources).get(sources.cursor.queue)?;
    sources.queue.items.get(orig).map(|a| (**a).clone())
}

pub fn hovered_middle_song(sources: &Sources) -> Option<mkproto::Song> {
    let orig_idx = *middle_filtered_indices(sources).get(sources.cursor.middle)?;
    match &sources.history.mode {
        MiddleMode::PlaylistSongs => sources
            .playlist_tracks
            .songs
            .get(orig_idx)
            .cloned()
            .flatten()
            .map(|a| (*a).clone()),
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            album_detail_songs(*awaiting_seq, sources).and_then(|s| s.get(orig_idx).cloned())
        }
        MiddleMode::ArtistDetail { awaiting_seq, .. } => {
            // Only top_songs are previewable — albums/artists in
            // the artist view aren't songs.
            match artist_detail_item(*awaiting_seq, sources, orig_idx)? {
                ArtistDetailItem::Song(s) => Some(s),
                _ => None,
            }
        }
        MiddleMode::SearchResults { .. } => {
            sources.search.songs.get(orig_idx).map(|a| (**a).clone())
        }
    }
}

pub fn current_queue_action_item(sources: &Sources) -> Option<ActionItem> {
    let visible = queue_filtered_indices(sources);
    let &orig = visible.get(sources.cursor.queue)?;
    let song = sources.queue.items.iter().nth(orig)?;
    Some(action_item_for_song(song).with_origin(ActionOrigin::Queue))
}

pub fn current_middle_action_item(sources: &Sources) -> Option<ActionItem> {
    let orig_idx = *middle_filtered_indices(sources).get(sources.cursor.middle)?;
    match &sources.history.mode {
        MiddleMode::PlaylistSongs => {
            let slot = sources.playlist_tracks.songs.get(orig_idx)?;
            slot.as_ref().map(|s| {
                let mut item = action_item_for_song(s);
                if let Some(pid) = sources.playlist_tracks.playlist_id.clone() {
                    item = item.with_playlist(pid.to_string(), orig_idx);
                }
                item
            })
        }
        MiddleMode::AlbumDetail { awaiting_seq, .. } => {
            let songs = album_detail_songs(*awaiting_seq, sources)?;
            songs.get(orig_idx).map(action_item_for_song)
        }
        MiddleMode::ArtistDetail { awaiting_seq, .. } => {
            // Action item depends on which sub-list the row is in.
            match artist_detail_item(*awaiting_seq, sources, orig_idx)? {
                ArtistDetailItem::Song(s) => Some(action_item_for_song(&s)),
                ArtistDetailItem::Album(a) => Some(
                    ActionItem::new(a.id.clone(), ActionKind::Album, a.name.clone())
                        .with_url(a.url.clone())
                        .with_album(Some(a.id.clone()), Some(a.name.clone()))
                        .with_artist(Some(a.artist_id.clone()), Some(a.artist_name.clone())),
                ),
                ArtistDetailItem::Artist(a) => Some(
                    ActionItem::new(a.id.clone(), ActionKind::Artist, a.name.clone())
                        .with_url(a.url.clone())
                        .with_artist(Some(a.id.clone()), Some(a.name.clone())),
                ),
            }
        }
        MiddleMode::SearchResults { search_type, .. } => match search_type {
            SearchType::Song => sources
                .search
                .songs
                .get(orig_idx)
                .map(|s| action_item_for_song(s)),
            SearchType::Album => sources.search.albums.get(orig_idx).map(|a| {
                ActionItem::new(a.id.clone(), ActionKind::Album, a.name.clone())
                    .with_url(a.url.clone())
                    .with_album(Some(a.id.clone()), Some(a.name.clone()))
                    .with_artist(Some(a.artist_id.clone()), Some(a.artist_name.clone()))
            }),
            SearchType::Artist => sources.search.artists.get(orig_idx).map(|a| {
                ActionItem::new(a.id.clone(), ActionKind::Artist, a.name.clone())
                    .with_url(a.url.clone())
                    .with_artist(Some(a.id.clone()), Some(a.name.clone()))
            }),
        },
    }
}

/// Visible server-side playlist rows in the left column, in render
/// order. Mirrors the filter chain in `views::left::left_column_model`
/// exactly: skip rows the user has optimistically deleted, apply the
/// name filter against the *displayed* name (so a pending rename
/// flips filter membership immediately), then yield the underlying
/// `Playlist`.
///
/// Single source of truth for "row index → playlist" — the view memo
/// and every dispatch helper that maps the left-column cursor to a
/// playlist must read the visible list from here. Without this, the
/// cursor index and the rendered list can disagree (e.g. a row hidden
/// by a pending delete is still reachable through the cursor),
/// landing actions on the wrong playlist.
fn visible_server_playlists<'a>(sources: &'a Sources, filter: &str) -> Vec<&'a mkproto::Playlist> {
    let lower = filter.to_lowercase();
    let pending = &sources.pending_playlists;
    sources
        .playlists
        .items
        .iter()
        .filter(|p| !pending.is_deleting(&p.id))
        .filter(|p| {
            if lower.is_empty() {
                return true;
            }
            let display = pending.name_override_for(&p.id).unwrap_or(p.name.as_str());
            display.to_lowercase().contains(&lower)
        })
        .map(|v| &**v)
        .collect()
}

pub fn filtered_playlist<'a>(
    sources: &'a Sources,
    filter: &str,
    index: usize,
) -> Option<&'a mkproto::Playlist> {
    visible_server_playlists(sources, filter)
        .into_iter()
        .nth(index)
}

pub fn filtered_playlist_count(sources: &Sources, filter: &str) -> usize {
    let server_visible = visible_server_playlists(sources, filter).len();
    let lower = filter.to_lowercase();
    let pending_visible = sources
        .pending_playlists
        .creating
        .iter()
        .filter(|c| lower.is_empty() || c.name.to_lowercase().contains(&lower))
        .count();
    server_visible + pending_visible
}

pub fn selected_playlist_id(sources: &Sources, filter: &str, index: usize) -> Option<String> {
    filtered_playlist(sources, filter, index).map(|p| p.id.clone())
}

pub fn selection_action_menu(sources: &Sources) -> Vec<(char, &'static str)> {
    let mut v = vec![
        ('n', "Play Next"),
        ('e', "Play Last"),
        ('a', "Add to Playlist"),
    ];
    match sources.selection.context {
        Some(SelectionContext::Queue) => v.push(('d', "Delete from Queue")),
        Some(SelectionContext::Middle) => {
            // Only offer Delete when middle is showing playlist
            // tracks — search/album/artist views can't be edited.
            if matches!(sources.history.mode, MiddleMode::PlaylistSongs) {
                v.push(('d', "Remove from Playlist"));
            }
        }
        None => {}
    }
    v
}

pub fn selection_row_count(sources: &Sources, ctx: SelectionContext) -> usize {
    match ctx {
        SelectionContext::Middle => middle_row_count(sources),
        SelectionContext::Queue => sources.queue.items.len(),
    }
}

pub fn gather_selection_song_ids(sources: &Sources, ctx: SelectionContext) -> Vec<String> {
    let sel = &sources.selection;
    match ctx {
        SelectionContext::Middle => match &sources.history.mode {
            MiddleMode::PlaylistSongs => sel
                .selected
                .iter()
                .filter_map(|i| {
                    sources
                        .playlist_tracks
                        .songs
                        .get(*i)
                        .and_then(|slot| slot.as_ref())
                        .map(|s| s.id.clone())
                })
                .collect(),
            MiddleMode::AlbumDetail { awaiting_seq, .. } => {
                let Some(songs) = album_detail_songs(*awaiting_seq, sources) else {
                    return vec![];
                };
                sel.selected
                    .iter()
                    .filter_map(|i| songs.get(*i).map(|s| s.id.clone()))
                    .collect()
            }
            MiddleMode::ArtistDetail { awaiting_seq, .. } => {
                let Some(songs) = artist_detail_songs(*awaiting_seq, sources) else {
                    return vec![];
                };
                sel.selected
                    .iter()
                    .filter_map(|i| songs.get(*i).map(|s| s.id.clone()))
                    .collect()
            }
            MiddleMode::SearchResults { .. } => sel
                .selected
                .iter()
                .filter_map(|i| sources.search.songs.get(*i).map(|s| s.id.clone()))
                .collect(),
        },
        SelectionContext::Queue => sel
            .selected
            .iter()
            .filter_map(|i| sources.queue.items.iter().nth(*i))
            .map(|s| s.id.clone())
            .collect(),
    }
}

pub fn sorted_queue_indices(sources: &Sources) -> Vec<usize> {
    let mut v: Vec<usize> = sources.selection.selected.iter().copied().collect();
    v.sort_unstable();
    v
}

/// Find the current server's mDNS name by matching the active
/// probe's fingerprint against discovery.
pub fn current_server_name(sources: &Sources) -> Option<String> {
    use mkpclient_state_probes::ProbeOutcome;
    let fp = sources.link.target.as_ref()?;
    sources
        .discovery
        .servers
        .iter()
        .find(|s| {
            let addr = format!("{}:{}", s.addr, s.port);
            matches!(sources.probes.get(&addr), Some(ProbeOutcome::Fingerprint(o)) if o.as_str() == &**fp)
        })
        .map(|s| s.name.clone())
}
