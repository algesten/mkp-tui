//! Clipboard-toast lifecycle: fire a toast once per successful
//! `ClipboardEvent::WriteOutcome` the worker reports.
//!
//! Spec §5/§6: `clipboard_toast_action()` diffs
//! `clipboard.last_outcome.seq` against `clipboard.last_toasted_seq`
//! and returns `Fire(text)` exactly once. The trampoline writes the
//! seq synchronously (so the next tick is Noop) and dispatches a
//! `Toast` event.
//!
//! The `success_toast` text is round-tripped through the worker on
//! the `Cmd`/`Event` ABI so this lifecycle doesn't need a parallel
//! map keyed by request seq.

use std::time::Duration;

use mkpclient_driver_clipboard_core::ClipboardState;

use crate::dispatch;
use crate::drivers::Drivers;
use crate::sources::Sources;
use crate::SemanticEvent;

// ─── inputs ────────────────────────────────────────────────────────

#[derive(drv::Input)]
pub struct ClipboardToastInput<'a> {
    pub last_outcome_seq: Option<u64>,
    pub last_outcome_ok: Option<bool>,
    pub last_outcome_text: Option<&'a String>,
    pub last_toasted_seq: Option<u64>,
}

impl<'a> ClipboardToastInput<'a> {
    pub fn new(c: &'a ClipboardState) -> Self {
        Self {
            last_outcome_seq: c.last_outcome.as_ref().map(|o| o.seq),
            last_outcome_ok: c.last_outcome.as_ref().map(|o| o.ok),
            last_outcome_text: c.last_outcome.as_ref().map(|o| &o.success_toast),
            last_toasted_seq: c.last_toasted_seq,
        }
    }
}

// ─── memos ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardToastAction {
    Noop,
    Fire {
        seq: u64,
        text: String,
    },
    /// Worker reported failure; just bump the seq guard so we don't
    /// loop. Currently no error toast (matches legacy behaviour).
    Skip {
        seq: u64,
    },
}

#[drv::memo(single)]
pub fn clipboard_toast_action<'a>(input: ClipboardToastInput<'a>) -> ClipboardToastAction {
    let Some(seq) = input.last_outcome_seq else {
        return ClipboardToastAction::Noop;
    };
    if input.last_toasted_seq == Some(seq) {
        return ClipboardToastAction::Noop;
    }
    match input.last_outcome_ok {
        Some(true) => ClipboardToastAction::Fire {
            seq,
            text: input.last_outcome_text.cloned().unwrap_or_default(),
        },
        Some(false) => ClipboardToastAction::Skip { seq },
        None => ClipboardToastAction::Noop,
    }
}

// ─── trampoline ────────────────────────────────────────────────────

pub fn apply_clipboard_toast(sources: &mut Sources, drivers: &Drivers) {
    let action = clipboard_toast_action(ClipboardToastInput::new(&sources.clipboard));
    match action {
        ClipboardToastAction::Noop => {}
        ClipboardToastAction::Fire { seq, text } => {
            sources.clipboard.last_toasted_seq = Some(seq);
            dispatch::dispatch(
                SemanticEvent::Toast {
                    text,
                    ttl: Duration::from_secs(3),
                },
                sources,
                drivers,
            );
        }
        ClipboardToastAction::Skip { seq } => {
            sources.clipboard.last_toasted_seq = Some(seq);
        }
    }
}
