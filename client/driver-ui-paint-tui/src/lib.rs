//! TUI paint driver — the spec-canonical execute-pattern wrapper
//! around ratatui's `terminal.draw(...)` call.
//!
//! ## Why
//!
//! Per EXAMPLE-ARCH.md §"Pure output drivers":
//!
//! > Without a source, paint becomes a free function in the runtime
//! > that can't be mocked, can't be throttled at its natural seat,
//! > and can't be traced with the same mechanism as every other
//! > driver.
//!
//! ratatui's paint is fully synchronous — every `Frame` cell must be
//! written each tick, so there's no async worker and no acknowledgement
//! to track. But the *boundary* the spec is concerned with — a
//! mockable seam, a per-paint trace hook, an in-flight source — still
//! pays off. This crate gives that boundary a name (`TuiPaintDriver`)
//! and a state type (`PaintState`).
//!
//! ## Shape
//!
//! - Source: [`PaintState`] holds `last_frame_id` (incremented on every
//!   paint) and `frames_drawn`. The spec example tracks the same
//!   "in-flight artifact" so a future ack channel slots in cleanly.
//! - Driver: [`TuiPaintDriver`] holds the trace sink and exposes
//!   `execute(paint, &mut state)` taking a closure. The closure is the
//!   binary's call into `tui::render::draw(...)` — invoked while the
//!   driver brackets it with `paint_start` / `paint_end` trace events.
//! - No async / no Cmd / no Event channel. The spec's `process` half
//!   of the pure-output-driver pattern is empty here because ratatui
//!   doesn't ack paints; if we ever swap to an async terminal backend
//!   the channel pair plus a `process(&mut state)` method drop in
//!   without disturbing this surface.

use std::sync::Arc;
use std::time::Instant;

/// In-flight source. The spec's "in-flight artifact + acknowledgement
/// state" — without acks (ratatui paints synchronously) this collapses
/// to just the artifact id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PaintState {
    /// Monotonically increasing per-paint id. A future ack channel
    /// would compare its acked id against this.
    pub last_frame_id: u64,
    /// Total paints since the driver was created. Useful for tracing
    /// and integration assertions.
    pub frames_drawn: u64,
}

/// `--golden-trace` hook for paint events. Mirrors the per-driver
/// trace pattern used everywhere else.
pub trait Trace: Send + Sync {
    fn paint_start(&self, _frame_id: u64) {}
    fn paint_end(&self, _frame_id: u64, _elapsed_us: u64) {}
}

pub struct NoopTrace;
impl Trace for NoopTrace {}

pub struct TuiPaintDriver {
    trace: Arc<dyn Trace>,
}

impl TuiPaintDriver {
    pub fn new(trace: Arc<dyn Trace>) -> Self {
        Self { trace }
    }

    /// Execute pattern: increment the in-flight id, fire trace, run
    /// the paint, finalise. The closure is what the binary normally
    /// passes to `terminal.draw` — the driver only adds the boundary.
    pub fn execute<F: FnOnce()>(&self, paint: F, state: &mut PaintState) {
        state.last_frame_id = state.last_frame_id.wrapping_add(1);
        let frame_id = state.last_frame_id;
        let started = Instant::now();
        self.trace.paint_start(frame_id);
        paint();
        let elapsed_us = started.elapsed().as_micros() as u64;
        state.frames_drawn = state.frames_drawn.wrapping_add(1);
        self.trace.paint_end(frame_id, elapsed_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CaptureTrace {
        events: Mutex<Vec<(&'static str, u64)>>,
    }
    impl Trace for CaptureTrace {
        fn paint_start(&self, frame_id: u64) {
            self.events.lock().unwrap().push(("start", frame_id));
        }
        fn paint_end(&self, frame_id: u64, _elapsed_us: u64) {
            self.events.lock().unwrap().push(("end", frame_id));
        }
    }

    #[test]
    fn execute_runs_closure_and_brackets_with_trace() {
        let trace = Arc::new(CaptureTrace::default());
        let driver = TuiPaintDriver::new(trace.clone());
        let mut state = PaintState::default();
        let mut painted = false;

        driver.execute(|| painted = true, &mut state);

        assert!(painted, "paint closure ran");
        assert_eq!(state.frames_drawn, 1);
        assert_eq!(state.last_frame_id, 1);
        let events = trace.events.lock().unwrap();
        assert_eq!(events[0], ("start", 1));
        assert_eq!(events[1], ("end", 1));
    }

    #[test]
    fn frame_id_increments_per_call() {
        let driver = TuiPaintDriver::new(Arc::new(NoopTrace));
        let mut state = PaintState::default();
        for expected in 1..=3 {
            driver.execute(|| {}, &mut state);
            assert_eq!(state.last_frame_id, expected);
            assert_eq!(state.frames_drawn, expected);
        }
    }
}
