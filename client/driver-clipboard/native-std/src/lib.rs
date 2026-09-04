//! std-thread + arboard backed clipboard native.
//!
//! The worker drains `Cmd::Write` from the runtime, opens an
//! `arboard::Clipboard`, attempts the write, and replies with an
//! outcome. arboard is desktop-only — for iOS, use a different
//! native (or a noop) since the crate doesn't build on iOS.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use log::warn;

use mkpclient_core::Notifier;
use mkpclient_driver_clipboard_core::{ClipboardCmd, ClipboardDriver, ClipboardEvent, Trace};

pub struct ClipboardNative {
    _marker: (),
}

pub fn spawn(trace: Arc<dyn Trace>, notify: Notifier) -> (ClipboardDriver, ClipboardNative) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ClipboardCmd>();
    let (event_tx, event_rx) = mpsc::channel::<ClipboardEvent>();

    thread::Builder::new()
        .name("mkp-clipboard".into())
        .spawn(move || worker_loop(cmd_rx, event_tx, notify))
        .expect("spawning clipboard worker should succeed");

    let driver = ClipboardDriver::new(cmd_tx, event_rx, trace);
    (driver, ClipboardNative { _marker: () })
}

fn worker_loop(rx: Receiver<ClipboardCmd>, tx: Sender<ClipboardEvent>, notify: Notifier) {
    while let Ok(cmd) = rx.recv() {
        let event = handle(cmd);
        if tx.send(event).is_err() {
            return;
        }
        notify.notify();
    }
}

fn handle(cmd: ClipboardCmd) -> ClipboardEvent {
    match cmd {
        ClipboardCmd::Write {
            seq,
            text,
            success_toast,
        } => {
            let ok = match arboard::Clipboard::new() {
                Ok(mut cb) => match cb.set_text(&text) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("clipboard: set_text failed: {e}");
                        false
                    }
                },
                Err(e) => {
                    warn!("clipboard: open failed: {e}");
                    false
                }
            };
            ClipboardEvent::WriteOutcome {
                seq,
                success_toast,
                ok,
            }
        }
    }
}
