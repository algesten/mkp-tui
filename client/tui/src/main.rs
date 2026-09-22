//! TUI entry point built on the drv-based client runtime.
//!
//! Sync `fn main`, zero tokio. The outer loop is ingest/execute
//! (runtime's `tick`) plus ratatui rendering plus crossterm input
//! draining, with `wait_for_wake` providing the idle block.

// Pull in the modules via the lib crate so integration tests in
// `tests/` can use them too.
use mkpclient_tui::{cli, input, render};

use std::io;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use mkpclient_driver_ui_paint_tui::{NoopTrace as PaintNoopTrace, PaintState, TuiPaintDriver};
use mkpclient_runtime::{Peer, Runtime, RuntimeOptions};
use mkpclient_runtime_desktop as runtime_desktop;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use mkpclient_tui::app::AppState;

#[derive(Parser, Debug)]
#[command(
    name = "mkp",
    version = env!("MKP_VERSION"),
    about = "Make Play TUI client"
)]
struct Cli {
    /// Skip auto-reconnect to the previously used server and go
    /// straight to the server picker.
    #[arg(long)]
    pick: bool,

    /// Use this paired server instead of the TUI's persisted default.
    #[arg(long, global = true, value_name = "HOSTNAME")]
    server: Option<String>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> io::Result<()> {
    let started = std::time::Instant::now();
    env_logger_init(started);
    let cli = Cli::parse();
    log::trace!(target: "mkp_startup", "event=start pick={} version={} profile={} os={} arch={}", cli.pick, env!("MKP_VERSION"), if cfg!(debug_assertions) { "debug" } else { "release" }, std::env::consts::OS, std::env::consts::ARCH);

    if let Some(command) = cli.command {
        if cli.pick {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--pick is only available when launching the TUI",
            ));
        }
        return cli::run(command, cli.server.as_deref(), cli.json)
            .map_err(|e| io::Error::other(e.to_string()));
    }
    if cli.server.is_some() || cli.json {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--server and --json require a CLI command",
        ));
    }

    let trace: Arc<dyn mkpclient_runtime::Trace> =
        Arc::new(mkpclient_runtime::LoggingTrace::default());
    let phase = std::time::Instant::now();
    let peer = Peer {
        user: std::env::var("USER").unwrap_or_else(|_| "mkptui".into()),
        host: sysinfo::System::host_name().unwrap_or_else(|| "mkptui-host".into()),
    };
    log::trace!(target: "mkp_startup", "event=peer_identity duration_us={}", phase.elapsed().as_micros());
    let phase = std::time::Instant::now();
    let mut rt =
        runtime_desktop::start_with_options(trace, peer, RuntimeOptions { pick: cli.pick });

    log::trace!(target: "mkp_startup", "event=runtime_created duration_us={}", phase.elapsed().as_micros());
    let phase = std::time::Instant::now();
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        SetTitle("Make Play")
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let input = input::spawn_input_thread(rt.notifier());

    log::trace!(target: "mkp_startup", "event=terminal_input_ready duration_us={}", phase.elapsed().as_micros());
    let mut app = AppState::default();
    // Without `--pick`, `runtime_desktop::start_with_options` issued
    // `LoadLastServer`; the result lands via `ingest_persist` and
    // seeds `session.preferred_server` before auto-connect runs.
    let result = run_loop(&mut rt, &mut app, &mut terminal, &input);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop<B: ratatui::backend::Backend>(
    rt: &mut Runtime,
    app: &mut AppState,
    terminal: &mut Terminal<B>,
    input: &input::InputHandle,
) -> io::Result<()> {
    // Spec §"Pure output drivers": paint goes through a driver, not
    // a free function. The driver brackets each `terminal.draw` with
    // a frame-id bump + trace events; the painter helpers under
    // `tui::render::*` stay where they are.
    let paint_driver = TuiPaintDriver::new(Arc::new(PaintNoopTrace));
    let mut paint_state = PaintState::default();
    let mut previous_view = String::new();

    loop {
        // 1. Drain UI input into dispatch.
        while let Some(ev) = input.try_next() {
            if input::translate(ev, rt, app) {
                return Ok(());
            }
        }
        if app.suspend_requested {
            app.suspend_requested = false;
            suspend(terminal)?;
            rt.dispatch(mkpclient_runtime::SemanticEvent::SendRequest {
                msg: mkpclient_runtime::ClientMsg::GetState,
                task_id: None,
            });
        }

        // 2. Runtime ingest + execute. The lifecycle phase
        // (auto-connect, backend tracking, server-lost modal,
        // server-error surfacing, cursor snap, deferred add,
        // saved-view restore) runs inside `tick`'s execute step —
        // see `runtime::lifecycle`.
        let tick_started = std::time::Instant::now();
        rt.tick();
        log::trace!(target: "mkp_startup", "event=tick frame={} duration_us={}", app.tick.wrapping_add(1), tick_started.elapsed().as_micros());
        app.tick = app.tick.wrapping_add(1);

        // 3. Render. The paint driver brackets the actual draw with
        //    its trace + in-flight bookkeeping.
        let mut draw_err: Option<io::Error> = None;
        let draw_started = std::time::Instant::now();
        paint_driver.execute(
            || {
                if let Err(e) = terminal.draw(|frame| {
                    let render_started = std::time::Instant::now();
                    render::draw(frame, app, rt);
                    log::trace!(target: "mkp_startup", "event=render frame={} duration_us={}", app.tick, render_started.elapsed().as_micros());
                }) {
                    draw_err = Some(io::Error::other(format!("ratatui draw: {e}")));
                }
            },
            &mut paint_state,
        );
        log::trace!(target: "mkp_startup", "event=draw frame={} duration_us={}", app.tick, draw_started.elapsed().as_micros());
        if let Some(e) = draw_err {
            return Err(e);
        }
        mkpclient_tui::startup::trace_after_draw(&rt.sources, app);
        if log::log_enabled!(target: "mkp_startup", log::Level::Trace) {
            let s = &rt.sources;
            let view = format!("link={:?} credentials_loaded={} discovered={} preferred={:?} backend={:?} state_received={} playback={:?} now_playing={} playlists_loaded={} playlists={} restored={} mode={:?} playlist_rows={}/{} playlist_pending={:?} search_first={} search_complete={} search_rows={} queue_rows={} queue_expected={:?} queue_version={} queue_index={:?} pending_requests={} persist_pending={:?}",
                s.link.phase, s.credentials.loaded, s.discovery.servers.len(), s.session.preferred_server,
                s.server.backend, s.server.play.is_some(), s.server.play.as_ref().map(|p| &p.playback),
                s.server.play.as_ref().is_some_and(|p| p.now_playing.is_some()),
                s.playlists.loaded, s.playlists.items.len(), s.session.auto_restored_view,
                match &s.history.mode {
                    mkpclient_state_ui_history::MiddleMode::PlaylistSongs => "playlist",
                    mkpclient_state_ui_history::MiddleMode::SearchResults { .. } => "search",
                    mkpclient_state_ui_history::MiddleMode::AlbumDetail { .. } => "album",
                    mkpclient_state_ui_history::MiddleMode::ArtistDetail { .. } => "artist",
                },
                s.playlist_tracks.songs.iter().filter(|s| s.is_some()).count(), s.playlist_tracks.total,
                s.playlist_tracks.pending_task, s.search.first_page_received, s.search.completed, s.search.songs.len() + s.search.albums.len() + s.search.artists.len(), s.queue.items.len(), s.queue.expected_total, s.queue.version,
                s.queue.current_index, s.requests.pending.len(), s.persist.loads_in_flight);
            if view != previous_view {
                log::trace!(target: "mkp_startup", "event=view_drawn frame={} {view}", app.tick);
                previous_view = view;
            }
        }

        // 4. Block until something happens. The runtime computes
        //    the timeout from `nearest_deadline(&sources)` — anything
        //    that needs the loop to wake at a wall-clock instant
        //    (spinner cadence, toast expiry, preview timeout, …)
        //    folds itself into that one min-fold. Input + driver
        //    events nudge the wake channel and unblock immediately.
        let wait_started = std::time::Instant::now();
        rt.wait_for_next_deadline();
        log::trace!(target: "mkp_startup", "event=wake frame={} wait_us={}", app.tick, wait_started.elapsed().as_micros());
    }
}

fn suspend<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        Show
    )?;
    #[allow(unsafe_code)]
    unsafe {
        libc::raise(libc::SIGTSTP);
    }
    enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide,
        SetTitle("Make Play")
    )?;
    terminal
        .clear()
        .map_err(|error| io::Error::other(error.to_string()))
}

// env_logger writes to stderr — redirect in your shell
// (`RUST_LOG=mkp_startup=trace mkp 2>/tmp/mkp.log`) so it doesn't scramble
// the TUI. Silent when RUST_LOG is unset.
fn env_logger_init(started: std::time::Instant) {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = env_logger::Builder::from_default_env()
            .format(move |buf, record| {
                writeln!(
                    buf,
                    "{} +{:012.3}ms {:5} [{}] {}: {}",
                    buf.timestamp_millis(),
                    started.elapsed().as_secs_f64() * 1000.0,
                    record.level(),
                    std::thread::current().name().unwrap_or("main"),
                    record.target(),
                    record.args()
                )
            })
            .try_init();
    }
}
