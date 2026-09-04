//! View model for the search-input modal (`Screen::SearchInput`).
//!
//! Mirrors the input field, the active type, and the persisted
//! recent-search list. The history vector is `imbl::Vector` so cache
//! hits are root-Arc bumps; the input field is `Arc<str>` so its
//! snap is a refcount bump.

use std::sync::Arc;

use imbl::Vector;
use mkproto::SearchType;

use mkpclient_state_ui_screen::{SearchHistoryItem, SearchState};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchHistoryRow {
    pub query: Arc<str>,
    /// Pre-formatted "Song" / "Album" / "Artist" / raw — what the
    /// renderer paints in the right column.
    pub type_label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchInputModel {
    pub input: Arc<str>,
    pub last_type: SearchType,
    pub history: Vector<SearchHistoryRow>,
    pub history_selected: Option<usize>,
}

#[derive(drv::Input)]
pub struct SearchInputModalInput<'a> {
    pub input: &'a Arc<str>,
    /// Encoded as a u8 because `SearchType` isn't `drv::ToStatic` —
    /// flatten in the projection so the memo input stays simple.
    pub last_type_idx: u8,
    pub history_selected: Option<usize>,
    /// Borrow the persistent history vector directly. drv's snap
    /// for `imbl::Vector` is a refcount bump and `eq_static` takes
    /// the ptr_eq fast path, so this stays O(1) on cache hits.
    pub history: &'a Vector<SearchHistoryItem>,
}

impl<'a> SearchInputModalInput<'a> {
    pub fn new(state: &'a SearchState) -> Self {
        Self {
            input: &state.input,
            last_type_idx: encode_search_type(state.last_type),
            history_selected: state.history_selected,
            history: &state.history,
        }
    }
}

#[drv::memo(single)]
pub fn search_input_model<'a>(input: SearchInputModalInput<'a>) -> SearchInputModel {
    let history: Vector<SearchHistoryRow> = input
        .history
        .iter()
        .map(|h| SearchHistoryRow {
            query: h.query.clone(),
            type_label: type_label(&h.search_type),
        })
        .collect();
    SearchInputModel {
        input: input.input.clone(),
        last_type: decode_search_type(input.last_type_idx),
        history,
        history_selected: input.history_selected,
    }
}

fn encode_search_type(t: SearchType) -> u8 {
    match t {
        SearchType::Song => 0,
        SearchType::Artist => 1,
        SearchType::Album => 2,
    }
}

fn decode_search_type(i: u8) -> SearchType {
    match i {
        1 => SearchType::Artist,
        2 => SearchType::Album,
        _ => SearchType::Song,
    }
}

fn type_label(s: &str) -> &'static str {
    match s {
        "song" => "Song",
        "artist" => "Artist",
        "album" => "Album",
        _ => "?",
    }
}
