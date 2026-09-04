//! User-decision source: the runtime's notion of "now".
//!
//! Per `EXAMPLE-ARCH.md` § "Time is a source field", time is data,
//! not a global. The runtime writes `clock.now = Instant::now()` at
//! the top of every tick and every dispatch handler / ingest
//! helper / memo that needs a wall-clock reference reads from this
//! source instead. Tests bypass the real clock by mutating the
//! field directly.

use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    /// Wall-clock instant captured at the start of the current
    /// runtime tick. Stable for the duration of one ingest →
    /// query → execute → render cycle.
    pub now: Instant,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            now: Instant::now(),
        }
    }
}

impl Clock {
    /// Set "now" to `t`. Called once at the top of each tick from
    /// `Runtime::tick` with `Instant::now()`; tests call it with a
    /// controlled value.
    pub fn tick(&mut self, t: Instant) {
        self.now = t;
    }
}
