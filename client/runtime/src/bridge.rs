//! `ViewBridge<T>` — the model-cache half of the SwiftUI-style bridge
//! described in `EXAMPLE-ARCH.md` ("Bridging to a reactive UI
//! framework"), adapted to ratatui.
//!
//! ## What this *does*
//!
//! Stores the last `T` rendered into a slot, alongside an
//! `is_unchanged(&T)` test. Memo outputs are `PartialEq` and small,
//! so callers can ask "did this view's model change since the last
//! frame?" with an `O(field-eq)` check on every paint.
//!
//! ## What this *does not*
//!
//! In SwiftUI the bridge skips the `@Observable` push when the model
//! is unchanged. ratatui has no such moving target — `Frame`'s
//! `Buffer` is always written cell-by-cell on every `terminal.draw`,
//! and ratatui's terminal-level diff handles "no actual stdout
//! writes when buffers match" all by itself. Skipping the consumer
//! call would leave the buffer cells blank.
//!
//! So in this codebase the bridge is a model cache and a place to
//! hang per-view diagnostics — not a paint guard. The redraw skip
//! lives at two cheaper layers: the drv memo cache (model
//! construction is O(1) on hit) and ratatui's buffer diff (no
//! terminal traffic on equal frames).

use std::marker::PhantomData;

#[derive(Debug)]
pub struct ViewBridge<T> {
    last: Option<T>,
    _marker: PhantomData<T>,
}

// Manual `Default` rather than derived: the derived impl requires
// `T: Default`, but we only need `Option<T>: Default` (always
// available). Lets `ViewBridge<NowPlayingModel>` etc. live in a
// `#[derive(Default)]` `ViewBridges` aggregator.
impl<T> Default for ViewBridge<T> {
    fn default() -> Self {
        Self {
            last: None,
            _marker: PhantomData,
        }
    }
}

impl<T: Clone + PartialEq> ViewBridge<T> {
    pub fn new() -> Self {
        Self {
            last: None,
            _marker: PhantomData,
        }
    }

    /// True iff `model == last`. Callers can branch on this for
    /// per-view tracing or to short-circuit non-paint side-effects;
    /// the actual ratatui paint must still run every frame.
    pub fn is_unchanged(&self, model: &T) -> bool {
        self.last.as_ref() == Some(model)
    }

    /// Record `model` as the most recent frame's value. Call once
    /// per frame after drawing.
    pub fn record(&mut self, model: T) {
        self.last = Some(model);
    }

    pub fn last(&self) -> Option<&T> {
        self.last.as_ref()
    }
}
