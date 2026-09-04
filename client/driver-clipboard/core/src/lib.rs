//! Sync core of the clipboard driver.
//!
//! Per `EXAMPLE-ARCH.md` §"Stateless drivers still need an in-flight
//! source": a clipboard write is a one-shot platform call with no
//! persistent external-fact source, but it does have in-flight
//! state (a write requested by dispatch, awaiting confirmation from
//! the worker). That state lives on `ClipboardState`.
//!
//! The runtime's lifecycle reads `last_outcome` to fire a toast on
//! success.
//!
//! # ABI
//!
//! `ClipboardCmd::Write { seq, text, success_toast }` — the worker
//! attempts the OS write and replies with
//! `ClipboardEvent::WriteOutcome { seq, success_toast, ok }`. The
//! `success_toast` round-trips through the worker so the lifecycle
//! that fires the toast doesn't need a parallel map keyed by `seq`.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

// ─── Source ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardState {
    /// User-decision: a freshly-dispatched copy request the driver's
    /// `execute` will consume on the next tick. None when there's
    /// nothing pending.
    pub pending: Option<CopyRequest>,
    /// External fact: the latest outcome the worker reported.
    /// Monotonically-tagged by `seq` so the toast lifecycle can fire
    /// exactly once per request even if outcomes pile up.
    pub last_outcome: Option<CopyOutcome>,
    /// Last `seq` the toast lifecycle has already fired for. Updated
    /// synchronously by the trampoline so the next tick is Idle.
    pub last_toasted_seq: Option<u64>,
    /// Monotonic counter for new requests. Dispatch increments it
    /// when allocating a new `CopyRequest`.
    pub next_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRequest {
    pub seq: u64,
    pub text: String,
    pub success_toast: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOutcome {
    pub seq: u64,
    pub success_toast: String,
    pub ok: bool,
}

impl ClipboardState {
    /// Allocate a new sequence id and stash a `CopyRequest` on
    /// `pending`. Replaces any prior un-consumed pending (rare —
    /// requires two dispatches inside one tick before execute runs).
    pub fn enqueue(&mut self, text: String, success_toast: String) {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.pending = Some(CopyRequest {
            seq: self.next_seq,
            text,
            success_toast,
        });
    }
}

// ─── ABI ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardCmd {
    Write {
        seq: u64,
        text: String,
        success_toast: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    WriteOutcome {
        seq: u64,
        success_toast: String,
        ok: bool,
    },
}

// ─── Trace ──────────────────────────────────────────────────────────

pub trait Trace: Send + Sync {
    fn clipboard_write(&self, _seq: u64) {}
    fn clipboard_outcome(&self, _seq: u64, _ok: bool) {}
}

pub struct NoopTrace;
impl Trace for NoopTrace {}

// ─── Driver handle ──────────────────────────────────────────────────

pub struct ClipboardDriver {
    cmd_tx: Sender<ClipboardCmd>,
    event_rx: Receiver<ClipboardEvent>,
    trace: Arc<dyn Trace>,
}

impl ClipboardDriver {
    pub fn new(
        cmd_tx: Sender<ClipboardCmd>,
        event_rx: Receiver<ClipboardEvent>,
        trace: Arc<dyn Trace>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            trace,
        }
    }

    /// No-op driver for platforms without a clipboard backend (iOS).
    /// Cmds go to a sender whose receiver is dropped, so writes are
    /// silently discarded; events never arrive. The runtime's
    /// `clipboard.last_outcome` stays `None`, so the toast lifecycle
    /// never fires.
    pub fn noop() -> Self {
        let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel();
        let (_ev_tx, event_rx) = std::sync::mpsc::channel();
        Self {
            cmd_tx,
            event_rx,
            trace: Arc::new(NoopTrace),
        }
    }

    /// Drain `pending` from the source and ship a `Write` cmd. The
    /// state's `pending` is taken sync so next tick is Noop.
    pub fn execute(&self, state: &mut ClipboardState) {
        let Some(req) = state.pending.take() else {
            return;
        };
        self.trace.clipboard_write(req.seq);
        let cmd = ClipboardCmd::Write {
            seq: req.seq,
            text: req.text,
            success_toast: req.success_toast,
        };
        let _ = self.cmd_tx.send(cmd);
    }

    /// Drain worker events into `last_outcome`.
    pub fn process(&self, state: &mut ClipboardState) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                ClipboardEvent::WriteOutcome {
                    seq,
                    success_toast,
                    ok,
                } => {
                    self.trace.clipboard_outcome(seq, ok);
                    state.last_outcome = Some(CopyOutcome {
                        seq,
                        success_toast,
                        ok,
                    });
                }
            }
        }
    }
}
