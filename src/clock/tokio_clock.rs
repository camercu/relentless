//! The Tokio-backed async clock.

use super::{AsyncClock, Clock};
use crate::compat::Duration;

/// An [`AsyncClock`] backed by the Tokio runtime's timer.
///
/// `now()` reads [`tokio::time::Instant`] and `wait_async` is
/// [`tokio::time::sleep`], so both seams follow Tokio's virtual time: under
/// `tokio::time::pause` (a `test-util` API of Tokio) the waits and the
/// elapsed reads stay coherent by construction, with no separate wiring.
///
/// # Ceiling on very long waits
///
/// A wait whose deadline overflows [`tokio::time::Instant`] completes at
/// Tokio's far-future instant (multiple decades out) instead of the requested
/// duration, so the [`Clock`] contract's "advance `now()` by at least the
/// waited duration" does not hold above that ceiling. Nothing panics and
/// nothing fires immediately; the wait is merely shorter than asked. This is
/// reachable without absurd input — [`wait::exponential`](crate::wait::exponential)
/// saturates to [`Duration::MAX`] under [`stop::never`](crate::stop::never).
/// Bound the strategy with `.cap(...)` if a run must not depend on where that
/// ceiling sits.
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    origin: tokio::time::Instant,
}

impl TokioClock {
    /// Creates a clock anchored at the current Tokio instant.
    ///
    /// Construction works outside a runtime; the waits (and paused-time
    /// reads) must run within a Tokio runtime context.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TokioClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl AsyncClock for TokioClock {
    type Wait = tokio::time::Sleep;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        tokio::time::sleep(dur)
    }
}
