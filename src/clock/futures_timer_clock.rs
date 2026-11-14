//! The `futures-timer`-backed async clock.

#[cfg(os_clock)]
use super::{AsyncClock, Clock};
#[cfg(os_clock)]
use crate::compat::Duration;

/// An [`AsyncClock`](super::AsyncClock) backed by [`futures_timer::Delay`]
/// and `std`'s monotonic [`std::time::Instant`].
///
/// `futures-timer` waits on real wall time, which `Instant` also measures, so
/// the pairing is coherent.
///
/// On `wasm32-unknown-unknown`, which has no OS clock, `Instant::now` panics.
/// There the type still exists but has no constructor and implements no clock
/// capability, so using it fails to compile. In a browser, use
/// `GlooClock` (feature `gloo-timers-clock`).
#[derive(Debug, Clone, Copy)]
pub struct FuturesTimerClock {
    #[cfg_attr(not(os_clock), allow(dead_code))]
    origin: std::time::Instant,
}

#[cfg(os_clock)]
impl FuturesTimerClock {
    /// Creates a clock anchored at the current instant.
    #[must_use]
    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(os_clock)]
impl Default for FuturesTimerClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(os_clock)]
impl Clock for FuturesTimerClock {
    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(os_clock)]
impl AsyncClock for FuturesTimerClock {
    type Wait = futures_timer::Delay;

    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    fn wait_async(&self, dur: Duration) -> Self::Wait {
        futures_timer::Delay::new(dur)
    }
}

assert_no_clock_without_os_clock!(FuturesTimerClock);
