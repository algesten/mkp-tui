//! Ingest phase: drain every driver's event queue and fold results
//! into the right sources.

use std::sync::Arc;

use log::{debug, info, warn};

use mkpclient_driver_credentials_core::CredEvent;
use mkpclient_driver_discovery_core::DiscoveryEvent;
use mkpclient_driver_link_core::{LinkEvent, LinkKind};
use mkpclient_driver_persist_core::{LoadKey, PersistEvent, ViewLoadResult};
use mkpclient_state_credentials::PairingEntry;
use mkpclient_state_link::{LinkKind as StateLinkKind, LinkPhase};
use mkpclient_state_pairing::{Pairing, PairingPhase};
use mkpclient_state_ui_history::MiddleMode;
use mkpclient_state_ui_screen::{Screen, SearchHistoryItem};
use mkproto::{ClientMsg, ListTarget, Peer, Response, ServerMsg, PROTOCOL_VERSION};

use crate::drivers::Drivers;
use crate::sources::Sources;

pub fn run(sources: &mut Sources, drivers: &Drivers, peer: &Peer) {
    ingest_discovery(&mut sources.discovery, drivers);
    ingest_credentials(&mut sources.credentials, drivers);
    ingest_link(sources, drivers, peer);
    ingest_persist(sources, drivers);
    drivers.clipboard.process(&mut sources.clipboard);
}

fn ingest_discovery(discovery: &mut mkpclient_state_discovery::Discovery, drivers: &Drivers) {
    for ev in drivers.discovery.process() {
        match ev {
            DiscoveryEvent::Added(ad) | DiscoveryEvent::Refreshed(ad) => discovery.upsert(ad),
            DiscoveryEvent::Removed { name } => discovery.remove(&name),
        }
    }
}

fn ingest_credentials(creds: &mut mkpclient_state_credentials::Credentials, drivers: &Drivers) {
    for ev in drivers.credentials.process() {
        match ev {
            CredEvent::Loaded(entries) => {
                creds.entries.clear();
                for e in entries {
                    creds.insert(e);
                }
                creds.loaded = true;
            }
            CredEvent::Saved(entry) => creds.insert(entry),
            CredEvent::Deleted { fingerprint } => creds.remove(&fingerprint),
            CredEvent::Error { op, message } => {
                warn!("credentials: {op} failed: {message}");
            }
        }
    }
}

fn ingest_link(sources: &mut Sources, drivers: &Drivers, peer: &Peer) {
    for ev in drivers.link.process() {
        match ev {
            LinkEvent::Connected { kind } => {
                sources.link.phase = LinkPhase::Connected;
                sources.link.kind = Some(match kind {
                    LinkKind::Client => StateLinkKind::Client,
                    LinkKind::Pairing => StateLinkKind::Pairing,
                });
                sources.link.last_err = None;
                match kind {
                    LinkKind::Pairing => {
                        sources.pairing.phase = PairingPhase::AwaitingResponse;
                    }
                    LinkKind::Client => {
                        // Post-connect handshake: identify. The
                        // server answers `Hello` with
                        // `BackendChanged`; folding that fact is what
                        // lets `backend_session` see a backend it
                        // hasn't built from and start the session.
                        // The execute phase ships this next tick.
                        sources.requests.push(
                            ClientMsg::Hello {
                                peer: peer.clone(),
                                version: PROTOCOL_VERSION,
                            },
                            None,
                        );
                    }
                }
            }
            LinkEvent::Frame(response) => {
                let response = *response;
                if response.seq == 0 {
                    fold_broadcast(sources, response);
                } else {
                    let consumed = mirror_response_into_source(sources, &response);
                    if !consumed {
                        sources.responses.insert(response.seq, response.msg);
                    }
                }
            }
            LinkEvent::PairingReady {
                server_cert_pem,
                client_cert_pem,
                client_key_pem,
                fingerprint,
                code,
            } => {
                sources.pairing = Pairing {
                    phase: PairingPhase::AwaitingConfirmation,
                    code: Some(Arc::from(code)),
                    server_fingerprint: Some(Arc::from(fingerprint)),
                    server_cert_pem: Some(server_cert_pem),
                    client_cert_pem: Some(client_cert_pem),
                    client_key_pem: Some(client_key_pem),
                    error: None,
                };
            }
            LinkEvent::PairFailed { message } => {
                sources.pairing.phase = PairingPhase::Failed;
                sources.pairing.error = Some(message);
            }
            LinkEvent::ProbeResult { addr, ref result } => {
                info!("ingest: ProbeResult addr={addr} result={result:?}");
                match result.clone() {
                    Ok(fingerprint) => sources.probes.set_fingerprint(addr, fingerprint),
                    Err(msg) => sources.probes.set_failed(addr, msg),
                }
                continue;
            }
            LinkEvent::Closed { error } => {
                // On disconnect, if we had an `AwaitingConfirmation`
                // pairing still stashed in sources, promote the
                // captured certs into a `Save` cmd so credentials
                // persist. The pairing session is now dead either
                // way.
                maybe_persist_confirmed_pairing(sources, drivers);

                sources.link.phase = LinkPhase::Closed;
                sources.link.kind = None;
                sources.link.last_err = error.map(Arc::from);
                reset_server_derived_state(sources);
            }
        }
    }
}

/// Drop every source that describes what the server was showing us.
///
/// Shared by the two things that invalidate it wholesale — the link
/// closing, and `backend_session` restarting on a backend it wasn't
/// built from — so the two cannot drift apart. Anything added here
/// must be true of both: state observed from the server, meaningless
/// once that server or its catalogue is gone.
pub(crate) fn reset_server_derived_state(sources: &mut Sources) {
    sources.requests.clear();
    sources.responses.clear();
    sources.server = Default::default();
    sources.queue = Default::default();
    sources.playlists = Default::default();
    sources.playlist_tracks.clear();
    sources.search.clear();
    sources.artist_extras.clear();
    sources.activity.clear();
    sources.pending_playlists.clear();
}

/// Some response variants carry live state (not just a reply) and
/// should be mirrored into the matching source even when they arrive
/// on a non-zero seq.
///
/// Returns `true` when the response has been fully handled and should
/// **not** be inserted into `sources.responses` (consumers won't see it,
/// and `apply_server_errors` won't surface it as a modal). Today this
/// applies to any `GetPlaylists` reply whose seq matches
/// `playlists.pending_request` — connect-time and refetch alike.
/// On the very first reply (`!loaded`), an error degrades to empty so
/// the rest of the UI proceeds; on a refetch error we keep the
/// existing items and just log.
fn mirror_response_into_source(sources: &mut Sources, response: &Response) -> bool {
    if Some(response.seq) == sources.playlists.pending_request {
        sources.playlists.pending_request = None;
        let was_initial = !sources.playlists.loaded;
        match &response.msg {
            ServerMsg::Playlists { playlists } => {
                sources.playlists.set_all(playlists.clone());
                sources.playlists.stale = false;
            }
            ServerMsg::Error { message } => {
                sources.playlists.pending_task = None;
                if was_initial {
                    warn!("startup GetPlaylists failed, falling back to empty list: {message}");
                    sources.playlists.set_all(Vec::new());
                } else {
                    warn!("GetPlaylists refetch failed, keeping existing items: {message}");
                }
                // Leave `stale = true` on a refetch error so the
                // lifecycle retries on next iteration once a request
                // slot is free.
            }
            other => {
                sources.playlists.pending_task = None;
                warn!("GetPlaylists got unexpected reply, falling back to empty list: {other:?}");
                if was_initial {
                    sources.playlists.set_all(Vec::new());
                }
            }
        }
        return true;
    }
    match &response.msg {
        ServerMsg::BackendChanged { backend } => {
            // Fold the fact only. `backend_session` diffs it against
            // `built_from` and restarts the session if they differ.
            sources.server.backend = Some(Arc::from(backend.as_str()));
            return true;
        }
        ServerMsg::PlaylistCreated { playlist } => {
            sources.playlists.upsert(playlist.clone());
            // Reconcile the optimistic shadow: this response confirms
            // the create issued under `response.seq`. Drop the
            // pending entry — the real playlist is now in
            // `playlists.items`.
            sources
                .pending_playlists
                .remove_creating_by_seq(response.seq);
        }
        ServerMsg::StateUpdate(play) => {
            sources.server.play = Some(play.clone());
        }
        ServerMsg::Search(results) => {
            // First page of a streaming search. We need the task_id
            // from the response envelope to correlate with the
            // SearchMore broadcasts that follow.
            if let Some(task_id) = response.task_id {
                sources.search.set_first_page(task_id, results.clone());
            }
        }
        ServerMsg::AlbumDetail { album, .. } => {
            // The "Go to Album" path from a song search result drills
            // into AlbumDetail with an empty `album_id` placeholder
            // (the server resolves the real id via Navigate). Backfill
            // it now that the response is in, so a subsequent Enter /
            // shuffle on a track sends `Play { id, kind: Album }` with
            // the real album id — otherwise MusicKit rejects the empty
            // id with `MusicDataRequest.Error code 1`.
            let resolved = album.id.as_str();
            let seq = response.seq;
            let patch = |mode: &mut MiddleMode| {
                if let MiddleMode::AlbumDetail {
                    album_id,
                    awaiting_seq,
                    ..
                } = mode
                {
                    if album_id.is_empty() && *awaiting_seq == Some(seq) {
                        *album_id = resolved.to_string();
                    }
                }
            };
            patch(&mut sources.history.mode);
            for frame in sources.history.back.iter_mut() {
                patch(&mut frame.mode);
            }
            for frame in sources.history.forward.iter_mut() {
                patch(&mut frame.mode);
            }
        }
        ServerMsg::Error { .. } => {
            // Roll back any optimistic mutation on rejection. The
            // `apply_server_errors` lifecycle still surfaces the
            // toast — we only undo the shadow entry here so the
            // visible list stops "lying."
            let pending = &mut sources.pending_playlists;
            pending.remove_creating_by_seq(response.seq);
            pending.remove_deleting_by_seq(response.seq);
            pending.remove_renaming_by_seq(response.seq);
            pending.remove_adding_by_seq(response.seq);
            pending.remove_removing_by_seq(response.seq);
        }
        _ => {}
    }
    false
}

/// Fold a broadcast `Response` (seq = 0) into the appropriate
/// state source. Frames that don't have a target source yet are
/// logged and dropped — the ports land incrementally.
fn fold_broadcast(sources: &mut Sources, response: Response) {
    let task_id = response.task_id;
    match response.msg {
        ServerMsg::StateUpdate(play) => {
            sources.server.play = Some(play);
        }
        ServerMsg::BackendChanged { backend } => {
            sources.server.backend = Some(Arc::from(backend));
        }
        ServerMsg::Playlists { playlists } => {
            sources.playlists.set_all(playlists);
        }
        ServerMsg::PlaylistTrackCount {
            playlist_id,
            track_count,
        } => {
            if task_id == sources.playlists.pending_task {
                sources.playlists.set_track_count(&playlist_id, track_count);
            }
        }
        ServerMsg::PlaylistCreated { playlist } => {
            sources.playlists.upsert(playlist);
        }
        ServerMsg::PlaylistMutated {
            playlist_id,
            mutation,
        } => match mutation {
            mkproto::PlaylistMutation::Deleted => {
                sources.playlists.remove(&playlist_id);
                // Confirm any optimistic delete for this id.
                sources
                    .pending_playlists
                    .remove_deleting_by_id(&playlist_id);
            }
            mkproto::PlaylistMutation::Renamed { new_name } => {
                sources.playlists.rename(&playlist_id, new_name);
                // Confirm any optimistic rename for this id.
                sources
                    .pending_playlists
                    .remove_renaming_by_id(&playlist_id);
            }
            mkproto::PlaylistMutation::SongAdded { songs } => {
                let count = songs.len();
                sources.playlist_tracks.extend(&playlist_id, songs);
                sources
                    .playlists
                    .adjust_track_count(&playlist_id, count as i32);
                // Drop the oldest in-flight add for this playlist.
                // Server processes adds serially so FIFO matches.
                sources
                    .pending_playlists
                    .drop_oldest_adding_for(&playlist_id);
            }
            mkproto::PlaylistMutation::SongRemoved { song_id, index } => {
                sources.playlist_tracks.remove_at(&playlist_id, index);
                sources.playlists.adjust_track_count(&playlist_id, -1);
                // Confirm the optimistic removal for this song.
                sources
                    .pending_playlists
                    .drop_removing_song(&playlist_id, &song_id);
            }
            mkproto::PlaylistMutation::Modified => {
                // The mutation says "this playlist's content may be
                // arbitrarily different from what the local mirror
                // shows; reload it." We don't reload here (Phase 1
                // is pure source-fold) — we just flip the staleness
                // flags so the playlists-refetch lifecycle picks up
                // the work in Phase 3.
                sources.playlists.stale = true;
                if sources.playlist_tracks.playlist_id.as_deref() == Some(&playlist_id) {
                    sources.playlist_tracks.stale = true;
                }
            }
        },
        ServerMsg::ListBegin {
            target,
            total,
            focus,
        } => match target {
            ListTarget::Playlist { id } => {
                sources.playlist_tracks.begin(Arc::from(id), total, focus);
            }
            ListTarget::Queue { queue_id, version } => {
                if sources.queue.queue_id != Some(queue_id) {
                    sources.queue.reset(queue_id);
                }
                sources.queue.version = version;
                sources.queue.expected_total = Some(total);
                if focus < total && sources.queue.current_index.is_none() {
                    sources.queue.current_index = Some(focus);
                }
            }
        },
        ServerMsg::ListChunk {
            target,
            offset,
            songs,
        } => match target {
            ListTarget::Playlist { id } => {
                if sources.playlist_tracks.playlist_id.as_deref() == Some(id.as_str()) {
                    sources.playlist_tracks.chunk(offset, songs);
                }
            }
            ListTarget::Queue { queue_id, .. } => {
                warn!("ingest: song-only ListChunk received for queue {queue_id}");
            }
        },
        ServerMsg::QueueChunk {
            queue_id,
            offset,
            entries,
        } => {
            if sources.queue.queue_id != Some(queue_id) {
                sources.queue.reset(queue_id);
            }
            if offset > sources.queue.items.len() {
                warn!(
                    "ingest: out-of-order queue chunk offset={offset} len={}",
                    sources.queue.items.len()
                );
            } else {
                sources.queue.chunk(offset, entries);
            }
        }
        ServerMsg::QueueDelta {
            queue_id,
            version,
            delta,
        } => {
            if sources.queue.queue_id != Some(queue_id) {
                sources.queue.reset(queue_id);
            }
            sources.queue.version = version;
            sources.queue.apply(delta);
        }
        ServerMsg::QueueCatchUp { queue_id, deltas } => {
            if sources.queue.queue_id != Some(queue_id) {
                sources.queue.reset(queue_id);
            }
            for (version, delta) in deltas {
                sources.queue.version = version;
                sources.queue.apply(delta);
            }
        }
        ServerMsg::SimilarArtists { artist_id, artists } => {
            sources.artist_extras.set_similar(artist_id, artists);
        }
        ServerMsg::ArtistAlbumsChunk { artist_id, albums } => {
            sources.artist_extras.append_albums(artist_id, albums);
        }
        ServerMsg::SearchMore(results) => {
            // Streamed continuation of a Search reply — only the
            // task_id correlates the page with the originating
            // request (seq is 0 here).
            if let Some(tid) = task_id {
                sources.search.append(tid, results);
            }
        }
        ServerMsg::TaskStarted {
            task_id: tid,
            peer,
            activity,
        } => {
            sources
                .activity
                .started(tid, peer, activity, sources.clock.now);
        }
        ServerMsg::TaskCompleted { task_id: tid } => {
            sources.search.mark_completed(tid);
            if task_id == Some(tid) && sources.playlists.pending_task == Some(tid) {
                sources.playlists.pending_task = None;
            }
            sources.activity.completed(tid);
        }
        ServerMsg::TaskFailed { task_id: tid, .. } => {
            sources.search.mark_completed(tid);
            if task_id == Some(tid) && sources.playlists.pending_task == Some(tid) {
                sources.playlists.pending_task = None;
            }
            sources.activity.completed(tid);
        }
        ServerMsg::ServerShutdown => {
            debug!("server announced shutdown");
        }
        other => {
            debug!(
                "link: dropping broadcast not yet modelled: {:?}",
                std::mem::discriminant(&other)
            );
        }
    }
}

/// When a pairing session closes *after* the user hit Confirm, we
/// already shipped PairConfirm over the wire and captured every
/// credential fragment in `sources.pairing`. Drop them into the
/// credentials driver now so they survive a restart.
fn maybe_persist_confirmed_pairing(sources: &mut Sources, drivers: &Drivers) {
    if sources.pairing.phase != PairingPhase::Confirming {
        // Either still awaiting confirm, or user rejected, or never
        // got that far. Nothing to persist.
        return;
    }
    let (Some(fingerprint), Some(server_cert_pem), Some(client_cert_pem), Some(client_key_pem)) = (
        sources.pairing.server_fingerprint.clone(),
        sources.pairing.server_cert_pem.clone(),
        sources.pairing.client_cert_pem.clone(),
        sources.pairing.client_key_pem.clone(),
    ) else {
        return;
    };
    // Host is whatever the user started pairing with. Look it up via
    // the current `intent.pair_target` if still set; otherwise empty
    // (the mdns name will refresh on next sighting anyway).
    let pair_target = sources.intent.pair_target.clone();
    let host = pair_target
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let entry = PairingEntry {
        fingerprint: fingerprint.to_string(),
        host,
        server_cert_pem,
        client_cert_pem,
        client_key_pem,
    };
    drivers
        .credentials
        .execute([&mkpclient_driver_credentials_core::CredCmd::Save(entry)]);
    sources.pairing = Pairing::default();
    sources.intent.pair_target = None;
    // Auto-connect to the just-paired server: route the same name
    // back through `intent.target` so the next tick's apply_link
    // probes (already cached) and connects once the credentials
    // driver returns `CredEvent::Saved`.
    if let Some(name) = pair_target {
        sources.intent.target = Some(name);
    }
}

// ─── persist ────────────────────────────────────────────────────────

fn ingest_persist(sources: &mut Sources, drivers: &Drivers) {
    for ev in drivers.persist.process() {
        if ingest_keybindings_persist_event(sources, &ev) {
            continue;
        }
        match ev {
            PersistEvent::KeybindingsLoaded { .. } | PersistEvent::KeybindingsSaved { .. } => {
                unreachable!("keybindings events handled above")
            }
            PersistEvent::LastServerLoaded { name } => {
                sources.persist.loads_in_flight.remove(&LoadKey::LastServer);
                // Only seed `preferred_server` on first boot — once
                // the user has connected somewhere this session,
                // dispatch already manages it.
                if sources.session.preferred_server.is_none() {
                    sources.session.preferred_server = name.map(Arc::from);
                }
            }
            PersistEvent::ViewLoaded { key, view } => {
                sources
                    .persist
                    .loads_in_flight
                    .remove(&LoadKey::View(key.clone()));
                // Stash for `lifecycle::restore`'s memo pair. The
                // trampoline reads, applies, and clears in one shot.
                sources.persist.last_view_load = Some(ViewLoadResult { key, view });
            }
            PersistEvent::SearchHistoryLoaded { backend, history } => {
                sources
                    .persist
                    .loads_in_flight
                    .remove(&LoadKey::SearchHistory(backend.clone()));
                if sources.session.backend_name.as_deref() != Some(backend.as_str()) {
                    continue;
                }
                if let Screen::SearchInput(state) = &mut sources.screen {
                    state.history = history
                        .items
                        .into_iter()
                        .map(|i| SearchHistoryItem {
                            query: Arc::from(i.query),
                            search_type: Arc::from(i.search_type),
                            ts: i.ts,
                        })
                        .collect();
                }
            }
            PersistEvent::LastAddPlaylistLoaded { backend, id } => {
                sources
                    .persist
                    .loads_in_flight
                    .remove(&LoadKey::LastAddPlaylist(backend.clone()));
                if sources.session.backend_name.as_deref() == Some(backend.as_str()) {
                    sources.picker.last_add_playlist = id.map(Arc::from);
                }
            }
            PersistEvent::SaveFailed { op, err } => {
                warn!("persist: {op} failed: {err}");
            }
        }
    }
}

fn ingest_keybindings_persist_event(sources: &mut Sources, event: &PersistEvent) -> bool {
    match event {
        PersistEvent::KeybindingsLoaded { keybindings } => {
            sources
                .persist
                .loads_in_flight
                .remove(&LoadKey::Keybindings);
            sources.keybindings = keybindings.clone();
            true
        }
        PersistEvent::KeybindingsSaved { keybindings } => {
            sources.keybindings = keybindings.clone();
            sources.toast.show(
                "Keybindings saved",
                sources.clock.now + std::time::Duration::from_secs(3),
            );
            true
        }
        PersistEvent::SaveFailed {
            op: "save_keybindings",
            err,
        } => {
            warn!("persist: save_keybindings failed: {err}");
            sources.toast.show(
                "Failed to save keybindings",
                sources.clock.now + std::time::Duration::from_secs(3),
            );
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod keybindings_persist_tests {
    use super::*;
    use mkpclient_state_ui_keybindings::{Action, KeyChord, KeyContext};

    #[test]
    fn save_ack_activates_bindings_and_reports_success() {
        let mut sources = Sources::default();
        let mut saved = sources.keybindings.clone();
        saved.replace(KeyContext::Global, Action::PlayPause, KeyChord::char('p'));
        assert!(ingest_keybindings_persist_event(
            &mut sources,
            &PersistEvent::KeybindingsSaved { keybindings: saved }
        ));
        assert_eq!(
            sources
                .keybindings
                .hint_for(KeyContext::Global, Action::PlayPause),
            "p"
        );
        assert_eq!(sources.toast.message.as_deref(), Some("Keybindings saved"));
    }

    #[test]
    fn save_failure_keeps_active_bindings_and_reports_failure() {
        let mut sources = Sources::default();
        let original = sources.keybindings.clone();
        assert!(ingest_keybindings_persist_event(
            &mut sources,
            &PersistEvent::SaveFailed {
                op: "save_keybindings",
                err: "disk full".into()
            }
        ));
        assert_eq!(sources.keybindings, original);
        assert_eq!(
            sources.toast.message.as_deref(),
            Some("Failed to save keybindings")
        );
    }
}
