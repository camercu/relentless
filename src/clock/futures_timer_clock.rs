//! The `futures-timer`-backed async clock.

use super::{AsyncClock, Clock};
use crate::compat::Duration;

/// An [`AsyncClock`] backed by [`futures_timer::Delay`] and `std`'s
/// monotonic [`std::time::Instant`].
///
/// `futures-timer` waits on real wall time, which `Instant` also measures, so
/// the pairing is coherent.
#[derive(Debug, Clone, Copy)]
pub struct FuturesTimerClock {
    origin: std::time::Instant,
}

impl FuturesTimerClock {
    /// Creates a clock anchored at the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for FuturesTimerClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FuturesTimerClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl AsyncClock for FuturesTimerClock {
    type Wait = futures_timer::Delay;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        futures_timer::Delay::new(dur)
    }
}
