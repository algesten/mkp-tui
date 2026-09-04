//! Non-interactive search and playlist commands for the `mkp` binary.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand, ValueEnum};
use mkpclient_core::Notifier;
use mkpclient_driver_discovery_core::{DiscoveryEvent, NoopTrace as DiscoveryTrace, ServerAd};
use mkpclient_driver_discovery_native_std as discovery_native;
use mkpclient_driver_link_core::{LinkCmd, LinkEvent, LinkKind, NoopTrace as LinkTrace};
use mkpclient_driver_link_native_std as link_native;
use mkproto::{
    ClientMsg, ListTarget, Peer, Playlist, Response, SearchResults, SearchType, ServerMsg, Song,
    TaskId, PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search the active music service.
    Search(SearchArgs),
    /// Inspect and manipulate playlists.
    Playlist(PlaylistArgs),
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    #[arg(value_enum)]
    kind: SearchKind,
    term: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SearchKind {
    Track,
    Album,
    Artist,
}

impl From<SearchKind> for SearchType {
    fn from(value: SearchKind) -> Self {
        match value {
            SearchKind::Track => SearchType::Song,
            SearchKind::Album => SearchType::Album,
            SearchKind::Artist => SearchType::Artist,
        }
    }
}

#[derive(Args, Debug)]
pub struct PlaylistArgs {
    #[command(subcommand)]
    command: PlaylistCommand,
}

#[derive(Subcommand, Debug)]
enum PlaylistCommand {
    /// List playlists and their IDs.
    List,
    /// Show every track in a playlist.
    Show { playlist_id: String },
    /// Create a playlist.
    Create { name: String },
    /// Rename a playlist.
    Rename { playlist_id: String, name: String },
    /// Delete a playlist.
    Delete {
        playlist_id: String,
        #[arg(long, required = true)]
        yes: bool,
    },
    /// Add one or more track IDs to a playlist.
    Add {
        playlist_id: String,
        #[arg(required = true)]
        track_ids: Vec<String>,
    },
    /// Remove tracks, using TRACK_ID or TRACK_ID:INDEX.
    Remove {
        playlist_id: String,
        #[arg(required = true)]
        tracks: Vec<TrackRef>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackRef {
    id: String,
    index: Option<usize>,
}

impl FromStr for TrackRef {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some((id, index)) = value.rsplit_once(':') {
            if id.is_empty() {
                return Err("track ID cannot be empty".into());
            }
            let index = index
                .parse::<usize>()
                .map_err(|_| format!("invalid playlist index in {value:?}"))?;
            Ok(Self {
                id: id.to_string(),
                index: Some(index),
            })
        } else if value.is_empty() {
            Err("track ID cannot be empty".into())
        } else {
            Ok(Self {
                id: value.to_string(),
                index: None,
            })
        }
    }
}

#[derive(Debug)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

impl From<String> for CliError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

pub fn run(command: Command, server: Option<&str>, json_output: bool) -> Result<(), CliError> {
    let hostname = selected_hostname(server)?;
    let (wake_tx, wake_rx) = mpsc::channel();
    let notify = Notifier::new(wake_tx);
    let ad = discover(&hostname, notify.clone(), &wake_rx)?;
    let (link, link_marker) = link_native::spawn(Arc::new(LinkTrace), notify);
    let credentials = authenticate(&ad, &link, &wake_rx)?;
    let mut session = Session::connect(ad, credentials, link, link_marker, wake_rx)?;

    match command {
        Command::Search(args) => {
            let results = session.search(&args.term, args.kind.into())?;
            print_search(results, json_output)?;
        }
        Command::Playlist(args) => run_playlist(&mut session, args.command, json_output)?,
    }
    Ok(())
}

fn config_dir() -> Result<PathBuf, CliError> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("mkp"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CliError("cannot determine the configuration directory".into()))?;
    Ok(PathBuf::from(home).join(".config").join("mkp"))
}

fn pairing_dir(fingerprint: &str) -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CliError("cannot determine the pairing credential directory".into()))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("mkp")
        .join("pairing")
        .join(fingerprint))
}

fn selected_hostname(server: Option<&str>) -> Result<String, CliError> {
    let raw = match server {
        Some(host) => {
            let host = host.trim();
            if !host.to_ascii_lowercase().ends_with(".local") {
                return Err(CliError(
                    "--server must be an mDNS hostname such as mini.local".into(),
                ));
            }
            host.to_string()
        }
        None => fs::read_to_string(config_dir()?.join("last_server"))
            .map_err(|_| {
                CliError("no default server; connect through the TUI first or use --server".into())
            })?
            .trim()
            .to_string(),
    };
    if raw.is_empty() {
        return Err(CliError("server hostname cannot be empty".into()));
    }
    Ok(if raw.to_ascii_lowercase().ends_with(".local") {
        raw
    } else {
        format!("{raw}.local")
    })
}

fn discover(
    hostname: &str,
    notify: Notifier,
    wake_rx: &mpsc::Receiver<()>,
) -> Result<ServerAd, CliError> {
    let (driver, _native) = discovery_native::spawn(Arc::new(DiscoveryTrace), notify);
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    loop {
        for event in driver.process() {
            let ad = match event {
                DiscoveryEvent::Added(ad) | DiscoveryEvent::Refreshed(ad) => ad,
                DiscoveryEvent::Removed { .. } => continue,
            };
            if normalize_hostname(&ad.host) == normalize_hostname(hostname) {
                return Ok(ad);
            }
        }
        wait_until(wake_rx, deadline).map_err(|_| {
            CliError(format!(
                "server {hostname} was not discovered within 10 seconds"
            ))
        })?;
    }
}

fn normalize_hostname(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

struct Credentials {
    fingerprint: String,
    server_cert_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
}

fn authenticate(
    ad: &ServerAd,
    link: &mkpclient_driver_link_core::LinkDriver,
    wake_rx: &mpsc::Receiver<()>,
) -> Result<Credentials, CliError> {
    let addr = format!("{}:{}", ad.addr, ad.port);
    link.execute([&LinkCmd::ProbeFingerprint { addr: addr.clone() }]);
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let fingerprint = 'probe: loop {
        for event in link.process() {
            if let LinkEvent::ProbeResult { result, .. } = event {
                break 'probe result
                    .map_err(|e| CliError(format!("cannot inspect {0}: {e}", ad.host)))?;
            }
        }
        wait_until(wake_rx, deadline)
            .map_err(|_| CliError(format!("timed out inspecting {}", ad.host)))?;
    };

    let dir = pairing_dir(&fingerprint)?;
    let read = |name: &str| {
        fs::read_to_string(dir.join(name)).map_err(|_| {
            CliError(format!(
                "server {} is not paired; pair it through the TUI first",
                ad.host
            ))
        })
    };
    Ok(Credentials {
        fingerprint,
        server_cert_pem: read("server_cert.pem")?,
        client_cert_pem: read("client_cert.pem")?,
        client_key_pem: read("client_key.pem")?,
    })
}

fn wait_until(wake_rx: &mpsc::Receiver<()>, deadline: Instant) -> Result<(), ()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(());
    }
    match wake_rx.recv_timeout(remaining) {
        Ok(()) => Ok(()),
        Err(_) => Err(()),
    }
}

struct Session {
    link: mkpclient_driver_link_core::LinkDriver,
    _native: link_native::LinkNative,
    wake_rx: mpsc::Receiver<()>,
    next_seq: u64,
    next_task: TaskId,
}

impl Session {
    fn connect(
        ad: ServerAd,
        credentials: Credentials,
        link: mkpclient_driver_link_core::LinkDriver,
        native: link_native::LinkNative,
        wake_rx: mpsc::Receiver<()>,
    ) -> Result<Self, CliError> {
        let addr = format!("{}:{}", ad.addr, ad.port);
        let host = ad.host.clone();
        link.execute([&LinkCmd::ConnectClient {
            addr,
            server_cert_pem: credentials.server_cert_pem,
            client_cert_pem: credentials.client_cert_pem,
            client_key_pem: credentials.client_key_pem,
            fingerprint: credentials.fingerprint,
        }]);
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            for event in link.process() {
                match event {
                    LinkEvent::Connected {
                        kind: LinkKind::Client,
                    } => {
                        let mut session = Self {
                            link,
                            _native: native,
                            wake_rx,
                            next_seq: 0,
                            next_task: 0,
                        };
                        let peer = Peer {
                            user: std::env::var("USER").unwrap_or_else(|_| "mkp-cli".into()),
                            host: sysinfo::System::host_name().unwrap_or_else(|| "mkp-cli".into()),
                        };
                        let hello_seq = session.send(
                            ClientMsg::Hello {
                                peer,
                                version: PROTOCOL_VERSION,
                            },
                            None,
                        );
                        session.wait_for_hello(hello_seq, &host)?;
                        return Ok(session);
                    }
                    LinkEvent::Closed { error } => {
                        return Err(CliError(format!(
                            "cannot connect to {}: {}",
                            ad.host,
                            error.unwrap_or_else(|| "connection closed".into())
                        )));
                    }
                    _ => {}
                }
            }
            wait_until(&wake_rx, deadline)
                .map_err(|_| CliError(format!("timed out connecting to {}", ad.host)))?;
        }
    }

    fn send(&mut self, msg: ClientMsg, task_id: Option<TaskId>) -> u64 {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.link.execute([&LinkCmd::Send { seq, task_id, msg }]);
        seq
    }

    fn task_id(&mut self) -> TaskId {
        self.next_task += 1;
        self.next_task
    }

    fn next_events(&self, deadline: Instant) -> Result<Vec<LinkEvent>, CliError> {
        loop {
            let events = self.link.process();
            if !events.is_empty() {
                return Ok(events);
            }
            wait_until(&self.wake_rx, deadline)
                .map_err(|_| CliError("server request timed out".into()))?;
        }
    }

    fn wait_for_hello(&self, hello_seq: u64, host: &str) -> Result<(), CliError> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            for event in self.next_events(deadline)? {
                match event {
                    LinkEvent::Frame(response) => match response.msg {
                        ServerMsg::BackendChanged { .. } if response.seq == hello_seq => {
                            return Ok(())
                        }
                        ServerMsg::Error { message } if response.seq == hello_seq => {
                            return Err(CliError(format!("{host} rejected the client: {message}")))
                        }
                        _ => {}
                    },
                    LinkEvent::Closed { error } => return Err(closed(error)),
                    _ => {}
                }
            }
        }
    }

    fn request(&mut self, msg: ClientMsg) -> Result<ServerMsg, CliError> {
        let seq = self.send(msg, None);
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            for event in self.next_events(deadline)? {
                match event {
                    LinkEvent::Frame(response) if response.seq == seq => {
                        return response_result(*response);
                    }
                    LinkEvent::Closed { error } => return Err(closed(error)),
                    _ => {}
                }
            }
        }
    }

    fn search(&mut self, term: &str, search_type: SearchType) -> Result<SearchResults, CliError> {
        let task = self.task_id();
        let seq = self.send(
            ClientMsg::Search {
                term: term.to_string(),
                search_type,
            },
            Some(task),
        );
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut result = empty_search(search_type);
        loop {
            for event in self.next_events(deadline)? {
                match event {
                    LinkEvent::Frame(response) => match response.msg {
                        ServerMsg::Search(page) if response.seq == seq => {
                            append_search(&mut result, page)?
                        }
                        ServerMsg::SearchMore(page) if response.task_id == Some(task) => {
                            append_search(&mut result, page)?
                        }
                        ServerMsg::TaskCompleted { task_id }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return Ok(result)
                        }
                        ServerMsg::TaskFailed { task_id, message }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return Err(CliError(message))
                        }
                        ServerMsg::Error { message } if response.seq == seq => {
                            return Err(CliError(message))
                        }
                        _ => {}
                    },
                    LinkEvent::Closed { error } => return Err(closed(error)),
                    _ => {}
                }
            }
        }
    }

    fn playlists(&mut self) -> Result<Vec<Playlist>, CliError> {
        let task = self.task_id();
        let seq = self.send(ClientMsg::GetPlaylists, Some(task));
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut playlists: Option<Vec<Playlist>> = None;
        loop {
            for event in self.next_events(deadline)? {
                match event {
                    LinkEvent::Frame(response) => match response.msg {
                        ServerMsg::Playlists { playlists: initial } if response.seq == seq => {
                            playlists = Some(initial);
                        }
                        ServerMsg::PlaylistTrackCount {
                            playlist_id,
                            track_count,
                        } if response.task_id == Some(task) => {
                            let Some(items) = playlists.as_mut() else {
                                return Err(CliError(
                                    "server streamed a playlist count before the playlist list"
                                        .into(),
                                ));
                            };
                            set_playlist_track_count(items, &playlist_id, track_count);
                        }
                        ServerMsg::TaskCompleted { task_id }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return playlists.ok_or_else(|| {
                                CliError(
                                    "server completed playlist loading without returning a list"
                                        .into(),
                                )
                            });
                        }
                        ServerMsg::TaskFailed { task_id, message }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return Err(CliError(message));
                        }
                        ServerMsg::Error { message } if response.seq == seq => {
                            return Err(CliError(message));
                        }
                        _ => {}
                    },
                    LinkEvent::Closed { error } => return Err(closed(error)),
                    _ => {}
                }
            }
        }
    }

    fn playlist_tracks(&mut self, playlist_id: &str) -> Result<Vec<Song>, CliError> {
        let task = self.task_id();
        let seq = self.send(
            ClientMsg::GetPlaylist {
                id: playlist_id.to_string(),
                focus: 0,
            },
            Some(task),
        );
        let target = ListTarget::Playlist {
            id: playlist_id.to_string(),
        };
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut songs: Vec<Option<Song>> = Vec::new();
        loop {
            for event in self.next_events(deadline)? {
                match event {
                    LinkEvent::Frame(response) => match response.msg {
                        ServerMsg::ListBegin {
                            target: got, total, ..
                        } if got == target
                            && (response.seq == seq || response.task_id == Some(task)) =>
                        {
                            songs.resize(total, None);
                        }
                        ServerMsg::ListChunk {
                            target: got,
                            offset,
                            songs: chunk,
                        } if got == target && response.task_id == Some(task) => {
                            if songs.len() < offset + chunk.len() {
                                songs.resize(offset + chunk.len(), None);
                            }
                            for (index, song) in chunk.into_iter().enumerate() {
                                songs[offset + index] = Some(song);
                            }
                        }
                        ServerMsg::TaskCompleted { task_id }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return songs
                                .into_iter()
                                .enumerate()
                                .map(|(index, song)| {
                                    song.ok_or_else(|| {
                                        CliError(format!(
                                            "playlist response omitted track at index {index}"
                                        ))
                                    })
                                })
                                .collect();
                        }
                        ServerMsg::TaskFailed { task_id, message }
                            if task_is_scoped(response.task_id, task_id, task) =>
                        {
                            return Err(CliError(message))
                        }
                        ServerMsg::Error { message } if response.seq == seq => {
                            return Err(CliError(message))
                        }
                        _ => {}
                    },
                    LinkEvent::Closed { error } => return Err(closed(error)),
                    _ => {}
                }
            }
        }
    }
}

fn response_result(response: Response) -> Result<ServerMsg, CliError> {
    match response.msg {
        ServerMsg::Error { message } => Err(CliError(message)),
        msg => Ok(msg),
    }
}

fn closed(error: Option<String>) -> CliError {
    CliError(error.unwrap_or_else(|| "server connection closed".into()))
}

fn task_is_scoped(
    response_task_id: Option<TaskId>,
    event_task_id: TaskId,
    expected: TaskId,
) -> bool {
    response_task_id == Some(expected) && event_task_id == expected
}

fn empty_search(kind: SearchType) -> SearchResults {
    match kind {
        SearchType::Song => SearchResults::Songs { songs: Vec::new() },
        SearchType::Album => SearchResults::Albums { albums: Vec::new() },
        SearchType::Artist => SearchResults::Artists {
            artists: Vec::new(),
        },
    }
}

fn append_search(target: &mut SearchResults, page: SearchResults) -> Result<(), CliError> {
    match (target, page) {
        (SearchResults::Songs { songs }, SearchResults::Songs { songs: page }) => {
            songs.extend(page)
        }
        (SearchResults::Albums { albums }, SearchResults::Albums { albums: page }) => {
            albums.extend(page)
        }
        (SearchResults::Artists { artists }, SearchResults::Artists { artists: page }) => {
            artists.extend(page)
        }
        _ => {
            return Err(CliError(
                "server returned the wrong search result type".into(),
            ))
        }
    }
    Ok(())
}

fn set_playlist_track_count(playlists: &mut [Playlist], id: &str, track_count: usize) {
    if let Some(playlist) = playlists.iter_mut().find(|playlist| playlist.id == id) {
        playlist.track_count = track_count;
    }
}

fn run_playlist(
    session: &mut Session,
    command: PlaylistCommand,
    json_output: bool,
) -> Result<(), CliError> {
    match command {
        PlaylistCommand::List => print_playlists(&session.playlists()?, json_output),
        PlaylistCommand::Show { playlist_id } => {
            let songs = session.playlist_tracks(&playlist_id)?;
            print_tracks(&songs, true, json_output)
        }
        PlaylistCommand::Create { name } => {
            match session.request(ClientMsg::CreatePlaylist { name })? {
                ServerMsg::PlaylistCreated { playlist } => {
                    print_mutation("created", Some(&playlist), json_output)
                }
                _ => Err(CliError(
                    "server returned an unexpected create response".into(),
                )),
            }
        }
        PlaylistCommand::Rename { playlist_id, name } => {
            expect_ok(session.request(ClientMsg::RenamePlaylist {
                id: playlist_id.clone(),
                name,
            })?)?;
            print_status("renamed", &playlist_id, json_output)
        }
        PlaylistCommand::Delete {
            playlist_id,
            yes: _,
        } => {
            expect_ok(session.request(ClientMsg::DeletePlaylist {
                id: playlist_id.clone(),
            })?)?;
            print_status("deleted", &playlist_id, json_output)
        }
        PlaylistCommand::Add {
            playlist_id,
            track_ids,
        } => {
            expect_ok(session.request(ClientMsg::AddToPlaylist {
                playlist_id: playlist_id.clone(),
                song_ids: track_ids.clone(),
                album_ids: Vec::new(),
            })?)?;
            if json_output {
                print_json(
                    &json!({"status": "added", "playlist_id": playlist_id, "track_ids": track_ids}),
                )
            } else {
                println!("Added {} track(s) to {}", track_ids.len(), playlist_id);
                Ok(())
            }
        }
        PlaylistCommand::Remove {
            playlist_id,
            tracks,
        } => {
            let songs = session.playlist_tracks(&playlist_id)?;
            let items = resolve_removals(&songs, &tracks)?;
            expect_ok(session.request(ClientMsg::RemoveFromPlaylist {
                playlist_id: playlist_id.clone(),
                items: items.clone(),
            })?)?;
            if json_output {
                let removed: Vec<_> = items
                    .into_iter()
                    .map(|(track_id, index)| {
                        json!({
                            "track_id": track_id,
                            "index": index,
                        })
                    })
                    .collect();
                print_json(
                    &json!({"status": "removed", "playlist_id": playlist_id, "tracks": removed}),
                )
            } else {
                println!("Removed {} track(s) from {}", items.len(), playlist_id);
                Ok(())
            }
        }
    }
}

fn expect_ok(message: ServerMsg) -> Result<(), CliError> {
    if matches!(message, ServerMsg::Ok) {
        Ok(())
    } else {
        Err(CliError(
            "server returned an unexpected mutation response".into(),
        ))
    }
}

fn resolve_removals(songs: &[Song], refs: &[TrackRef]) -> Result<Vec<(String, usize)>, CliError> {
    let mut occurrences: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, song) in songs.iter().enumerate() {
        occurrences.entry(&song.id).or_default().push(index);
    }

    let mut items = Vec::with_capacity(refs.len());
    for track in refs {
        let indices = occurrences
            .get(track.id.as_str())
            .ok_or_else(|| CliError(format!("track {} is not in the playlist", track.id)))?;
        let index = match track.index {
            Some(index) if indices.contains(&index) => index,
            Some(index) => {
                return Err(CliError(format!(
                    "track {} is not at playlist index {}",
                    track.id, index
                )))
            }
            None if indices.len() == 1 => indices[0],
            None => {
                return Err(CliError(format!(
                    "track {} occurs more than once; specify one of: {}",
                    track.id,
                    indices
                        .iter()
                        .map(|i| format!("{}:{}", track.id, i))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        };
        if items.iter().any(|(_, existing)| *existing == index) {
            return Err(CliError(format!(
                "playlist index {index} was specified more than once"
            )));
        }
        items.push((track.id.clone(), index));
    }
    Ok(items)
}

fn print_search(results: SearchResults, json_output: bool) -> Result<(), CliError> {
    if json_output {
        return print_json(&results);
    }
    match results {
        SearchResults::Songs { songs } => print_tracks(&songs, false, false),
        SearchResults::Albums { albums } => {
            let mut rows = vec![vec![
                "ID".into(),
                "NAME".into(),
                "ARTIST".into(),
                "TRACKS".into(),
            ]];
            for album in albums {
                rows.push(vec![
                    album.id,
                    album.name,
                    album.artist_name,
                    album.track_count.to_string(),
                ]);
            }
            print_table(&rows);
            Ok(())
        }
        SearchResults::Artists { artists } => {
            let mut rows = vec![vec!["ID".into(), "NAME".into()]];
            for artist in artists {
                rows.push(vec![artist.id, artist.name]);
            }
            print_table(&rows);
            Ok(())
        }
    }
}

fn print_playlists(playlists: &[Playlist], json_output: bool) -> Result<(), CliError> {
    if json_output {
        return print_json(playlists);
    }
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "TRACKS".into()]];
    for playlist in playlists {
        rows.push(vec![
            playlist.id.clone(),
            playlist.name.clone(),
            playlist.track_count.to_string(),
        ]);
    }
    print_table(&rows);
    Ok(())
}

#[derive(Serialize)]
struct IndexedTrack<'a> {
    index: usize,
    #[serde(flatten)]
    song: &'a Song,
}

fn print_tracks(songs: &[Song], indexed: bool, json_output: bool) -> Result<(), CliError> {
    if json_output {
        if indexed {
            let rows: Vec<_> = songs
                .iter()
                .enumerate()
                .map(|(index, song)| IndexedTrack { index, song })
                .collect();
            return print_json(&rows);
        }
        return print_json(songs);
    }
    let mut rows = if indexed {
        vec![vec![
            "INDEX".into(),
            "ID".into(),
            "TITLE".into(),
            "ARTIST".into(),
            "ALBUM".into(),
            "TRACK".into(),
            "DURATION".into(),
        ]]
    } else {
        vec![vec![
            "ID".into(),
            "TITLE".into(),
            "ARTIST".into(),
            "ALBUM".into(),
            "TRACK".into(),
            "DURATION".into(),
        ]]
    };
    for (index, song) in songs.iter().enumerate() {
        let track = song
            .track_number
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into());
        let duration = format_duration(song.duration);
        if indexed {
            rows.push(vec![
                index.to_string(),
                song.id.clone(),
                song.title.clone(),
                song.artist_name.clone(),
                song.album_title.clone(),
                track,
                duration,
            ]);
        } else {
            rows.push(vec![
                song.id.clone(),
                song.title.clone(),
                song.artist_name.clone(),
                song.album_title.clone(),
                track,
                duration,
            ]);
        }
    }
    print_table(&rows);
    Ok(())
}

fn print_table(rows: &[Vec<String>]) {
    print!("{}", format_table(rows));
}

fn format_table(rows: &[Vec<String>]) -> String {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; column_count];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }

    let mut output = String::new();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            output.push_str(cell);
            if column + 1 < row.len() {
                let padding = widths[column] - UnicodeWidthStr::width(cell.as_str()) + 2;
                output.extend(std::iter::repeat_n(' ', padding));
            }
        }
        output.push('\n');
    }
    output
}

fn format_duration(seconds: f32) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn print_mutation(
    status: &str,
    playlist: Option<&Playlist>,
    json_output: bool,
) -> Result<(), CliError> {
    if json_output {
        print_json(&json!({"status": status, "playlist": playlist}))
    } else {
        let id = playlist.map(|p| p.id.as_str()).unwrap_or_default();
        println!("{} {}", capitalize(status), id);
        Ok(())
    }
}

fn print_status(status: &str, playlist_id: &str, json_output: bool) -> Result<(), CliError> {
    if json_output {
        print_json(&json!({"status": status, "playlist_id": playlist_id}))
    } else {
        println!("{} {}", capitalize(status), playlist_id);
        Ok(())
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<(), CliError> {
    let stdout = std::io::stdout();
    serde_json::to_writer_pretty(stdout.lock(), value).map_err(|e| CliError(e.to_string()))?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            title: id.into(),
            artist_name: String::new(),
            album_title: String::new(),
            duration: 0.0,
            track_number: None,
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        }
    }

    fn playlist(id: &str, name: &str, track_count: usize) -> Playlist {
        Playlist {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            track_count,
        }
    }

    #[test]
    fn track_ref_accepts_optional_index() {
        assert_eq!(
            "123".parse::<TrackRef>().unwrap(),
            TrackRef {
                id: "123".into(),
                index: None
            }
        );
        assert_eq!(
            "123:7".parse::<TrackRef>().unwrap(),
            TrackRef {
                id: "123".into(),
                index: Some(7)
            }
        );
    }

    #[test]
    fn explicit_server_requires_local_hostname() {
        assert_eq!(selected_hostname(Some("mini.local")).unwrap(), "mini.local");
        assert_eq!(
            selected_hostname(Some("mini")).unwrap_err().to_string(),
            "--server must be an mDNS hostname such as mini.local"
        );
    }

    #[test]
    fn global_or_other_client_task_events_are_not_request_completion() {
        assert!(task_is_scoped(Some(1), 1, 1));
        assert!(!task_is_scoped(None, 1, 1));
        assert!(!task_is_scoped(Some(2), 1, 1));
        assert!(!task_is_scoped(Some(1), 2, 1));
    }

    #[test]
    fn streamed_count_updates_the_matching_playlist() {
        let mut playlists = vec![playlist("one", "One", 0), playlist("two", "Two", 0)];
        set_playlist_track_count(&mut playlists, "two", 12);
        assert_eq!(playlists[0].track_count, 0);
        assert_eq!(playlists[1].track_count, 12);
    }

    #[test]
    fn hello_wait_ignores_uncorrelated_backend_broadcast_before_rejection() {
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (_wake_tx, wake_rx) = mpsc::channel();
        let link =
            mkpclient_driver_link_core::LinkDriver::new(cmd_tx, event_rx, Arc::new(LinkTrace));
        let (_unused_link, native) = link_native::spawn(Arc::new(LinkTrace), Notifier::noop());
        let session = Session {
            link,
            _native: native,
            wake_rx,
            next_seq: 0,
            next_task: 0,
        };
        let hello_seq = 7;

        event_tx
            .send(LinkEvent::Frame(Box::new(Response {
                seq: 0,
                task_id: None,
                msg: ServerMsg::BackendChanged {
                    backend: "tidal".into(),
                },
            })))
            .unwrap();
        event_tx
            .send(LinkEvent::Frame(Box::new(Response {
                seq: hello_seq,
                task_id: None,
                msg: ServerMsg::Error {
                    message: "Protocol version mismatch".into(),
                },
            })))
            .unwrap();

        let error = session
            .wait_for_hello(hello_seq, "old-server.local")
            .expect_err("an unrelated broadcast must not acknowledge Hello");
        assert!(error.to_string().contains("Protocol version mismatch"));
    }

    #[test]
    fn bare_track_id_resolves_when_unique() {
        let songs = vec![song("a"), song("b")];
        assert_eq!(
            resolve_removals(&songs, &["b".parse().unwrap()]).unwrap(),
            vec![("b".into(), 1)]
        );
    }

    #[test]
    fn duplicate_track_id_requires_index() {
        let songs = vec![song("a"), song("b"), song("a")];
        let err = resolve_removals(&songs, &["a".parse().unwrap()]).unwrap_err();
        assert!(err.to_string().contains("a:0, a:2"));
        assert_eq!(
            resolve_removals(&songs, &["a:2".parse().unwrap()]).unwrap(),
            vec![("a".into(), 2)]
        );
    }

    #[test]
    fn indexed_track_must_match_id() {
        let songs = vec![song("a"), song("b")];
        let err = resolve_removals(&songs, &["a:1".parse().unwrap()]).unwrap_err();
        assert_eq!(err.to_string(), "track a is not at playlist index 1");
    }

    #[test]
    fn table_columns_are_padded_to_display_width() {
        let rows = vec![
            vec!["ID".into(), "NAME".into(), "TRACKS".into()],
            vec!["p.1".into(), "brains out".into(), "0".into()],
            vec!["p.long".into(), "swänska".into(), "12".into()],
        ];

        assert_eq!(
            format_table(&rows),
            "ID      NAME        TRACKS\n\
             p.1     brains out  0\n\
             p.long  swänska     12\n"
        );
    }

    #[test]
    fn table_padding_uses_unicode_terminal_width() {
        let rows = vec![
            vec!["NAME".into(), "COUNT".into()],
            vec!["音楽".into(), "1".into()],
            vec!["abc".into(), "2".into()],
        ];

        assert_eq!(
            format_table(&rows),
            "NAME  COUNT\n\
             音楽  1\n\
             abc   2\n"
        );
    }
}
