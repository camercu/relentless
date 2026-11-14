//! The Tokio-backed async clock.

#[cfg(os_clock)]
use super::{AsyncClock, Clock};
#[cfg(os_clock)]
use crate::compat::Duration;

/// An [`AsyncClock`](super::AsyncClock) backed by the Tokio runtime's timer.
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
/// duration, so the [`Clock`](super::Clock) contract's "advance `now()` by at
/// least the waited duration" does not hold above that ceiling. Nothing panics
/// and nothing fires immediately; the wait is merely shorter than asked. This
/// is reachable without absurd input:
/// [`wait::exponential`](crate::wait::exponential) saturates to
/// [`Duration::MAX`](core::time::Duration::MAX) under
/// [`stop::never`](crate::stop::never). Bound the strategy with `.cap(...)` if
/// a run must not depend on where that ceiling sits.
///
/// # Not on `wasm32-unknown-unknown`
///
/// That target has no OS clock, and `tokio::time::Instant::now` panics there.
/// The type still exists but has no constructor and implements no clock
/// capability, so using it fails to compile. In a browser, use `GlooClock`
/// (feature `gloo-timers-clock`).
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    #[cfg_attr(not(os_clock), allow(dead_code))]
    origin: tokio::time::Instant,
}

#[cfg(os_clock)]
impl TokioClock {
    /// Creates a clock anchored at the current Tokio instant.
    ///
    /// Construction works outside a runtime; the waits (and paused-time
    /// reads) must run within a Tokio runtime context.
    #[must_use]
    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

#[cfg(os_clock)]
impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(os_clock)]
impl Clock for TokioClock {
    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(os_clock)]
impl AsyncClock for TokioClock {
    type Wait = tokio::time::Sleep;

    #[cfg_attr(
        os_clock,
        allow(clippy::disallowed_methods, reason = "gated on cfg(os_clock)")
    )]
    fn wait_async(&self, dur: Duration) -> Self::Wait {
        tokio::time::sleep(dur)
    }
}

assert_no_clock_without_os_clock!(TokioClock);
