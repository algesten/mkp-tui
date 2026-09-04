//! A tiny cross-driver wake primitive.
//!
//! The runtime's main loop blocks on a shared mpsc receiver until
//! *any* driver signals that it has fresh events. Each native worker
//! holds a [`Notifier`] (cloned from the runtime) and calls
//! [`Notifier::notify`] after every successful send on its own event
//! channel. This replaces a sleep-loop with zero-latency wake-on-event.
//!
//! Tests that don't care about wake-up can use [`Notifier::noop`].

use std::sync::mpsc::Sender;

/// Cheap, cloneable handle a driver uses to nudge the runtime.
#[derive(Clone, Debug)]
pub struct Notifier(Option<Sender<()>>);

impl Notifier {
    pub fn new(tx: Sender<()>) -> Self {
        Self(Some(tx))
    }

    pub fn noop() -> Self {
        Self(None)
    }

    /// Send a wake signal. Silently drops when the runtime has hung up.
    pub fn notify(&self) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(());
        }
    }
}

impl Default for Notifier {
    fn default() -> Self {
        Self::noop()
    }
}
