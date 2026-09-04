//! Bundle of every source, mutable through one `&mut Sources`. Passed
//! down the ingest / dispatch call chains with disjoint per-field
//! borrows where needed.

use mkpclient_driver_clipboard_core::ClipboardState;
use mkpclient_driver_persist_core::Persist;
use mkpclient_state_activity::Activity;
use mkpclient_state_artist_detail::ArtistDetailExtras;
use mkpclient_state_clock::Clock;
use mkpclient_state_credentials::Credentials;
use mkpclient_state_discovery::Discovery;
use mkpclient_state_intent::Intent;
use mkpclient_state_link::Link;
use mkpclient_state_pairing::Pairing;
use mkpclient_state_playlist_tracks::PlaylistTracks;
use mkpclient_state_playlists::Playlists;
use mkpclient_state_probes::Probes;
use mkpclient_state_queue::Queue;
use mkpclient_state_request_queue::RequestQueue;
use mkpclient_state_responses::Responses;
use mkpclient_state_search::Search;
use mkpclient_state_server_state::ServerState;
use mkpclient_state_ui_cursor::Cursor;
use mkpclient_state_ui_filter::UiFilter;
use mkpclient_state_ui_history::UiHistory;
use mkpclient_state_ui_keybindings::Keybindings;
use mkpclient_state_ui_picker::UiPicker;
use mkpclient_state_ui_playlists_pending::PendingPlaylists;
use mkpclient_state_ui_preview::UiPreview;
use mkpclient_state_ui_screen::Screen;
use mkpclient_state_ui_selection::UiSelection;
use mkpclient_state_ui_session::UiSession;
use mkpclient_state_ui_toast::UiToast;

#[derive(Debug, Default)]
pub struct Sources {
    /// Wall-clock source — `Runtime::tick` writes `clock.now =
    /// Instant::now()` once per iteration; every dispatch handler /
    /// ingest helper / memo that needs a "now" reads it from here
    /// (`EXAMPLE-ARCH.md` § "Time is a source field").
    pub clock: Clock,
    // External-fact sources.
    pub discovery: Discovery,
    pub link: Link,
    pub pairing: Pairing,
    pub playlists: Playlists,
    pub playlist_tracks: PlaylistTracks,
    pub probes: Probes,
    pub queue: Queue,
    pub responses: Responses,
    pub search: Search,
    pub artist_extras: ArtistDetailExtras,
    pub activity: Activity,
    pub server: ServerState,
    /// In-flight persist loads + write-pending counter. Driver-owned;
    /// the runtime reads `is_loading(...)` to dedupe before issuing a
    /// `Load*` cmd, and the ingest phase clears the matching key on
    /// the corresponding `*Loaded` event.
    pub persist: Persist,
    /// In-flight clipboard write + last outcome. Owned by the
    /// clipboard driver; dispatch sets `pending` to enqueue, the
    /// driver's `execute` consumes it, and the toast lifecycle reads
    /// `last_outcome` to fire a Toast on success.
    pub clipboard: ClipboardState,
    // User-decision sources.
    pub intent: Intent,
    pub requests: RequestQueue,
    pub credentials: Credentials,
    pub preview: UiPreview,
    pub toast: UiToast,
    pub cursor: Cursor,
    pub filter: UiFilter,
    pub selection: UiSelection,
    pub history: UiHistory,
    pub keybindings: Keybindings,
    pub screen: Screen,
    pub picker: UiPicker,
    /// Optimistic-update breadcrumbs for in-flight `CreatePlaylist`
    /// and `DeletePlaylist`. Merged with `playlists.items` by the
    /// left-column view memo.
    pub pending_playlists: PendingPlaylists,
    pub session: UiSession,
}
