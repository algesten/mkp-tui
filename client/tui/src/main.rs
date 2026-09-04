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

struct NoopTrace;
impl mkpclient_runtime::Trace for NoopTrace {}

#[derive(Parser, Debug)]
#[command(name = "mkp", about = "Make Play TUI client")]
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
    let cli = Cli::parse();
    env_logger_init();

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

    let trace: Arc<dyn mkpclient_runtime::Trace> = Arc::new(NoopTrace);
    let peer = Peer {
        user: std::env::var("USER").unwrap_or_else(|_| "mkptui".into()),
        host: sysinfo::System::host_name().unwrap_or_else(|| "mkptui-host".into()),
    };
    let mut rt =
        runtime_desktop::start_with_options(trace, peer, RuntimeOptions { pick: cli.pick });

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
        rt.tick();
        app.tick = app.tick.wrapping_add(1);

        // 3. Render. The paint driver brackets the actual draw with
        //    its trace + in-flight bookkeeping.
        let mut draw_err: Option<io::Error> = None;
        paint_driver.execute(
            || {
                if let Err(e) = terminal.draw(|frame| render::draw(frame, app, rt)) {
                    draw_err = Some(io::Error::other(format!("ratatui draw: {e}")));
                }
            },
            &mut paint_state,
        );
        if let Some(e) = draw_err {
            return Err(e);
        }

        // 4. Block until something happens. The runtime computes
        //    the timeout from `nearest_deadline(&sources)` — anything
        //    that needs the loop to wake at a wall-clock instant
        //    (spinner cadence, toast expiry, preview timeout, …)
        //    folds itself into that one min-fold. Input + driver
        //    events nudge the wake channel and unblock immediately.
        rt.wait_for_next_deadline();
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
// (`RUST_LOG=info mkptui 2>/tmp/mkptui.log`) so it doesn't scramble
// the TUI. Silent when RUST_LOG is unset.
fn env_logger_init() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_millis()
            .try_init();
    }
}
