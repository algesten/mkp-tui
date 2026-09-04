//! View model for the left-hand "Playlists" column.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The playlists list,
//! the focused playlist id, and (as a fallback) the discovery /
//! probe / link state used to derive the server label all flow in
//! through narrowly-projected `drv::Input`s.

use std::sync::Arc;

use imbl::{HashMap as ImHashMap, Vector};
use mkproto::Playlist;

use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_state_discovery::Discovery;
use mkpclient_state_link::{Link, LinkPhase};
use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_playlists::Playlists;
use mkpclient_state_probes::{ProbeOutcome, Probes};
use mkpclient_state_ui_playlists_pending::{
    PendingCreate, PendingDelete, PendingPlaylists, PendingRename,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum LeftRow {
    /// "Search…" hint with the leading 'S' underlined.
    SearchHint,
    /// Blank separator.
    Blank,
    /// One playlist entry. Index in original (unfiltered) playlist
    /// list; useful if a future memo ever needs to map back.
    Playlist {
        name: String,
        is_viewing: bool,
        is_cursor: bool,
        /// `true` when the picker overlay (PlaylistPicker screen) is
        /// driving the cursor for this row. The picker takes over
        /// the column and its cursor is always shown, even when the
        /// pane is unfocused (legacy parity).
        is_picker: bool,
        /// `true` when this row is an optimistic-create placeholder
        /// awaiting `PlaylistCreated`. The painter renders a spinner
        /// glyph in the row.
        is_pending: bool,
    },
    /// "New…" entry at the bottom of the list. Renderer paints dim
    /// + cursor when applicable.
    NewPlaylist {
        is_cursor: bool,
        /// Cursor on the New row in an unfocused pane gets a dim
        /// cursor rather than nothing — matches legacy.
        focused: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LeftColumnModel {
    /// Whether the column has keyboard focus (or the picker overlay
    /// is active, which counts as focus visually).
    pub focused: bool,
    /// `" {server_name} "` — already padded for the title.
    pub server_title: Arc<str>,
    /// `true` when the cursor is on the "server" row (index 0).
    /// Renderer overlays the cursor style on the title.
    pub on_server_row: bool,
    pub rows: Vector<LeftRow>,
    /// Visual cursor index for `ListState` — `None` when no row is
    /// highlighted (server row is highlighted via the title, not via
    /// `selected`).
    pub list_cursor: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, drv::Input)]
pub struct PickerOverride {
    /// Picker's own selection (0..N = playlist row, N = "New…").
    pub selected: usize,
}

/// User-decision UI knobs the left-column memo reads. Kept as a
/// single bundle so the memo signature stays narrow — per the
/// "always bundle, never `#[allow]`" discipline (see led-rewrite
/// docs/rewrite/README.md § "Key decisions already made").
#[derive(drv::Input)]
pub struct LeftUiInput<'a> {
    pub backend_name: Option<&'a str>,
    pub column_focused: bool,
    pub left_selected: usize,
    pub playlist_filter: &'a Arc<str>,
    pub picker: Option<PickerOverride>,
    /// `true` only when the middle pane is showing the playlist's
    /// tracks. Legacy clears the green "viewing" marker on the left
    /// column whenever the user navigates away (search results, album
    /// detail, artist detail), even though `playlist_tracks.playlist_id`
    /// still points at the prior playlist.
    pub viewing_active: bool,
}

#[derive(drv::Input)]
pub struct PlaylistsInput<'a> {
    pub items: &'a Vector<Arc<Playlist>>,
}

impl<'a> PlaylistsInput<'a> {
    pub fn new(p: &'a Playlists) -> Self {
        Self { items: &p.items }
    }
}

#[derive(drv::Input)]
pub struct PendingPlaylistsInput<'a> {
    pub creating: &'a Vector<PendingCreate>,
    pub deleting: &'a Vector<PendingDelete>,
    pub renaming: &'a Vector<PendingRename>,
}

impl<'a> PendingPlaylistsInput<'a> {
    pub fn new(p: &'a PendingPlaylists) -> Self {
        Self {
            creating: &p.creating,
            deleting: &p.deleting,
            renaming: &p.renaming,
        }
    }
}

#[derive(drv::Input)]
pub struct PlaylistTracksFocusInput<'a> {
    pub playlist_id: Option<&'a std::sync::Arc<str>>,
}

impl<'a> PlaylistTracksFocusInput<'a> {
    pub fn new(t: &'a PlaylistTracks) -> Self {
        Self {
            playlist_id: t.playlist_id.as_ref(),
        }
    }
}

#[derive(drv::Input)]
pub struct ServerLabelInput<'a> {
    /// Project `LinkPhase` to a bool — only "is connected?" matters
    /// for the server-label fallback. Keeps `state-link` drv-free.
    pub link_connected: bool,
    pub link_target: Option<&'a std::sync::Arc<str>>,
    pub servers: &'a Vector<ServerAd>,
    pub probes: &'a ImHashMap<String, ProbeOutcome>,
}

impl<'a> ServerLabelInput<'a> {
    pub fn new(link: &'a Link, discovery: &'a Discovery, probes: &'a Probes) -> Self {
        Self {
            link_connected: link.phase == LinkPhase::Connected,
            link_target: link.target.as_ref(),
            servers: &discovery.servers,
            probes: &probes.by_addr,
        }
    }
}

/// Build the left-column model.
///
/// `picker` is `Some` when the user has the playlist picker open —
/// at that point the column shows the same rows but the cursor
/// follows `picker.selected` instead of `left_selected`, and is
/// always visible (even when the pane is unfocused).
#[drv::memo(single)]
pub fn left_column_model<'a, 'b, 'c, 'd, 'e>(
    playlists: PlaylistsInput<'a>,
    pending: PendingPlaylistsInput<'b>,
    tracks: PlaylistTracksFocusInput<'c>,
    server: ServerLabelInput<'d>,
    ui: LeftUiInput<'e>,
) -> LeftColumnModel {
    let server_name = match ui.backend_name {
        Some(b) => b.to_string(),
        None => derive_server_name(&server),
    };
    let server_title: Arc<str> = Arc::from(format!(" {server_name} "));

    let focused = ui.column_focused || ui.picker.is_some();
    let on_server_row = ui.left_selected == 0 && focused && ui.picker.is_none();

    let filter_lower = ui.playlist_filter.to_lowercase();
    let current_playlist_id: Option<&str> = tracks.playlist_id.map(|s| &**s);

    let mut rows: Vector<LeftRow> = Vector::new();
    rows.push_back(LeftRow::SearchHint);
    rows.push_back(LeftRow::Blank);

    // Optimistic merge: skip server entries the user is deleting,
    // override the displayed name when a rename is in flight, and
    // append placeholder rows for new creates. Per EXAMPLE-ARCH §3
    // ("shadow sources"), the user source (`pending`) and the
    // server source (`playlists`) join here in the view memo —
    // neither source knows about the other.
    let is_deleting = |p: &Arc<Playlist>| pending.deleting.iter().any(|d| d.id == p.id);
    let rename_for = |id: &str| pending.renaming.iter().find(|r| r.id == id);
    let mut playlist_count = 0usize;
    for p in playlists.items.iter().filter(|p| !is_deleting(p)) {
        // Filter against the *displayed* name (rename takes effect
        // immediately, so the filter sees the new value).
        let pending_rename = rename_for(p.id.as_str());
        let display_name: &str = pending_rename
            .map(|r| r.new_name.as_str())
            .unwrap_or(p.name.as_str());
        if !filter_lower.is_empty() && !display_name.to_lowercase().contains(&filter_lower) {
            continue;
        }
        let row_pos = playlist_count + 1; // server row is 0
        let is_viewing = ui.viewing_active && current_playlist_id == Some(p.id.as_str());
        let is_picker = ui.picker.is_some_and(|o| o.selected == playlist_count);
        let is_cursor = ui.picker.is_none() && focused && row_pos == ui.left_selected;
        rows.push_back(LeftRow::Playlist {
            name: display_name.to_string(),
            is_viewing,
            is_cursor,
            is_picker,
            is_pending: pending_rename.is_some(),
        });
        playlist_count += 1;
    }

    // Pending create rows — appended after server entries, before
    // "New…". Same filter discipline (typed name passes through the
    // playlist filter so a user filtering "foo" while creating "bar"
    // doesn't see a transient "bar" row).
    for c in pending.creating.iter() {
        if !filter_lower.is_empty() && !c.name.to_lowercase().contains(&filter_lower) {
            continue;
        }
        let row_pos = playlist_count + 1;
        let is_picker = ui.picker.is_some_and(|o| o.selected == playlist_count);
        let is_cursor = ui.picker.is_none() && focused && row_pos == ui.left_selected;
        rows.push_back(LeftRow::Playlist {
            name: c.name.clone(),
            is_viewing: false,
            is_cursor,
            is_picker,
            is_pending: true,
        });
        playlist_count += 1;
    }

    // "New…" — picker idx N, app idx N + 1.
    let new_app_row = playlist_count + 1;
    let new_picker_idx = playlist_count;
    let new_cursor = match ui.picker {
        Some(o) => o.selected == new_picker_idx,
        None => ui.left_selected == new_app_row,
    };
    rows.push_back(LeftRow::NewPlaylist {
        is_cursor: new_cursor,
        focused,
    });

    let list_cursor = match ui.picker {
        Some(o) => Some(o.selected + 2),
        None if ui.left_selected >= 1 => Some(ui.left_selected + 1),
        _ => None,
    };

    LeftColumnModel {
        focused,
        server_title,
        on_server_row,
        rows,
        list_cursor,
    }
}

fn derive_server_name(server: &ServerLabelInput) -> String {
    if !server.link_connected {
        return "server".into();
    }
    let target = match server.link_target {
        Some(t) => t,
        None => return "server".into(),
    };
    server
        .servers
        .iter()
        .find(|s| {
            let addr = format!("{}:{}", s.addr, s.port);
            matches!(
                server.probes.get(&addr),
                Some(ProbeOutcome::Fingerprint(fp)) if fp.as_str() == &**target
            )
        })
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "server".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use mkpclient_state_discovery::Discovery;
    use mkpclient_state_link::Link;
    use mkpclient_state_playlist_tracks::PlaylistTracks;
    use mkpclient_state_playlists::Playlists;
    use mkpclient_state_probes::Probes;
    use mkproto::Playlist;

    fn pl(id: &str, name: &str) -> Arc<Playlist> {
        Arc::new(Playlist {
            id: id.into(),
            name: name.into(),
            description: "".into(),
            track_count: 0,
        })
    }

    fn run(
        playlists: &Playlists,
        tracks: &PlaylistTracks,
        backend_name: Option<&str>,
        focused: bool,
        sel: usize,
        filter: &str,
        picker: Option<PickerOverride>,
    ) -> LeftColumnModel {
        let link = Link::default();
        let disco = Discovery::default();
        let probes = Probes::default();
        let pending = PendingPlaylists::default();
        let filter_arc: Arc<str> = Arc::from(filter);
        left_column_model(
            PlaylistsInput::new(playlists),
            PendingPlaylistsInput::new(&pending),
            PlaylistTracksFocusInput::new(tracks),
            ServerLabelInput::new(&link, &disco, &probes),
            LeftUiInput {
                backend_name,
                column_focused: focused,
                left_selected: sel,
                playlist_filter: &filter_arc,
                picker,
                viewing_active: true,
            },
        )
    }

    #[test]
    fn empty_playlists_yields_three_static_rows() {
        let pls = Playlists::default();
        let pt = PlaylistTracks::default();
        let m = run(&pls, &pt, Some("server"), true, 1, "", None);
        // SearchHint + Blank + NewPlaylist
        assert_eq!(m.rows.len(), 3);
        assert!(matches!(m.rows[0], LeftRow::SearchHint));
        assert!(matches!(m.rows[1], LeftRow::Blank));
        assert!(matches!(m.rows[2], LeftRow::NewPlaylist { .. }));
        assert_eq!(&*m.server_title, " server ");
    }

    #[test]
    fn three_playlists_no_filter_renders_all_with_cursor_on_first() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        pls.items.push_back(pl("b", "Beta"));
        pls.items.push_back(pl("c", "Gamma"));
        let pt = PlaylistTracks::default();
        let m = run(&pls, &pt, Some("S"), true, 1, "", None);
        // 2 header + 3 playlist + 1 New = 6
        assert_eq!(m.rows.len(), 6);
        if let LeftRow::Playlist {
            name,
            is_cursor,
            is_viewing,
            ..
        } = &m.rows[2]
        {
            assert_eq!(name, "Alpha");
            assert!(is_cursor);
            assert!(!is_viewing);
        } else {
            panic!("expected Playlist row at idx 2");
        }
        assert_eq!(m.list_cursor, Some(2));
    }

    #[test]
    fn case_insensitive_filter_drops_non_matching() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        pls.items.push_back(pl("b", "Beta"));
        pls.items.push_back(pl("c", "Bullet Train"));
        let pt = PlaylistTracks::default();
        let m = run(&pls, &pt, Some("S"), true, 1, "BU", None);
        // SearchHint + Blank + 1 playlist + New = 4
        assert_eq!(m.rows.len(), 4);
        if let LeftRow::Playlist { name, .. } = &m.rows[2] {
            assert_eq!(name, "Bullet Train");
        } else {
            panic!("expected Playlist row");
        }
    }

    #[test]
    fn current_playlist_id_marks_is_viewing() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        pls.items.push_back(pl("b", "Beta"));
        let pt = PlaylistTracks {
            playlist_id: Some("b".into()),
            ..Default::default()
        };
        let m = run(&pls, &pt, Some("S"), false, 0, "", None);
        if let LeftRow::Playlist {
            name, is_viewing, ..
        } = &m.rows[3]
        {
            assert_eq!(name, "Beta");
            assert!(is_viewing);
        } else {
            panic!("expected Playlist row at idx 3");
        }
    }

    #[test]
    fn picker_mode_overrides_cursor_indexing_and_disables_server_row() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        pls.items.push_back(pl("b", "Beta"));
        let pt = PlaylistTracks::default();
        let m = run(
            &pls,
            &pt,
            Some("S"),
            false,
            1,
            "",
            Some(PickerOverride { selected: 1 }),
        );
        // The picker forces "focused" (visual focus) regardless of
        // column_focused.
        assert!(m.focused);
        // Server row is not highlighted while picking.
        assert!(!m.on_server_row);
        if let LeftRow::Playlist {
            is_picker,
            is_cursor,
            ..
        } = &m.rows[3]
        {
            assert!(is_picker);
            assert!(!is_cursor);
        } else {
            panic!("expected Playlist row at idx 3");
        }
        // visual idx = picker.selected + 2 = 3
        assert_eq!(m.list_cursor, Some(3));
    }

    #[test]
    fn server_row_marker_when_left_selected_zero_and_focused() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        let pt = PlaylistTracks::default();
        let m = run(&pls, &pt, Some("S"), true, 0, "", None);
        assert!(m.on_server_row);
        // No list_cursor when on server row.
        assert!(m.list_cursor.is_none());
    }

    #[test]
    fn new_playlist_row_cursor_when_left_selected_at_end() {
        let mut pls = Playlists::default();
        pls.items.push_back(pl("a", "Alpha"));
        pls.items.push_back(pl("b", "Beta"));
        let pt = PlaylistTracks::default();
        // playlists at app rows 1, 2 → New is at row 3.
        let m = run(&pls, &pt, Some("S"), true, 3, "", None);
        let last_idx = m.rows.len() - 1;
        if let LeftRow::NewPlaylist { is_cursor, .. } = &m.rows[last_idx] {
            assert!(is_cursor);
        } else {
            panic!("expected NewPlaylist last row");
        }
        // Visual cursor: new app row + 1 = 4 (matches list idx 4 →
        // Search(0) + Blank(1) + 2 playlists(2,3) + New(4))
        assert_eq!(m.list_cursor, Some(4));
    }
}
