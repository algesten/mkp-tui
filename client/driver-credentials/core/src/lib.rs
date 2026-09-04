//! Sync core of the credentials driver.
//!
//! Three commands cross to the native worker: `Load` at startup,
//! `Save` after a successful pairing, `Delete` when the user
//! un-pairs a server. Each produces a corresponding `Done` event
//! the runtime's ingest phase folds into the `Credentials` source.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use mkpclient_state_credentials::PairingEntry;

#[derive(Clone, Debug)]
pub enum CredCmd {
    /// Read every stored entry (called once at startup).
    Load,
    /// Persist a newly-paired entry.
    Save(PairingEntry),
    /// Remove the entry with this fingerprint.
    Delete { fingerprint: String },
}

#[derive(Debug)]
pub enum CredEvent {
    /// Initial load completed. The runtime clears the source and
    /// inserts each of these, then flips `loaded = true`.
    Loaded(Vec<PairingEntry>),
    /// A `Save` finished successfully; runtime upserts.
    Saved(PairingEntry),
    /// A `Delete` finished; runtime removes.
    Deleted { fingerprint: String },
    /// A command failed. The runtime logs; downstream state is not
    /// mutated by an error (any optimistic intent writes stay, so the
    /// user can see that their action at least reached the driver).
    Error { op: &'static str, message: String },
}

pub trait Trace: Send + Sync {
    fn cred_load_start(&self);
    fn cred_load_done(&self, n: usize);
    fn cred_save(&self, fingerprint: &str);
    fn cred_delete(&self, fingerprint: &str);
    fn cred_error(&self, op: &'static str, message: &str);
}

pub struct NoopTrace;
impl Trace for NoopTrace {
    fn cred_load_start(&self) {}
    fn cred_load_done(&self, _: usize) {}
    fn cred_save(&self, _: &str) {}
    fn cred_delete(&self, _: &str) {}
    fn cred_error(&self, _: &'static str, _: &str) {}
}

pub struct CredDriver {
    cmd_tx: Sender<CredCmd>,
    event_rx: Receiver<CredEvent>,
    trace: Arc<dyn Trace>,
}

impl CredDriver {
    pub fn new(
        cmd_tx: Sender<CredCmd>,
        event_rx: Receiver<CredEvent>,
        trace: Arc<dyn Trace>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            trace,
        }
    }

    /// Ship commands to the worker. Silently no-ops if the worker has
    /// hung up (the runtime is shutting down); the runtime observes
    /// the same condition through its own channel dance.
    pub fn execute<'a, I>(&self, cmds: I)
    where
        I: IntoIterator<Item = &'a CredCmd>,
    {
        for cmd in cmds {
            match cmd {
                CredCmd::Load => self.trace.cred_load_start(),
                CredCmd::Save(entry) => self.trace.cred_save(&entry.fingerprint),
                CredCmd::Delete { fingerprint } => self.trace.cred_delete(fingerprint),
            }
            if self.cmd_tx.send(cmd.clone()).is_err() {
                return;
            }
        }
    }

    pub fn process(&self) -> Vec<CredEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                CredEvent::Loaded(entries) => self.trace.cred_load_done(entries.len()),
                CredEvent::Error { op, message } => self.trace.cred_error(op, message),
                _ => {}
            }
            out.push(ev);
        }
        out
    }
}
