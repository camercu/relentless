//! The Tokio-backed async clock.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use super::{AsyncClock, Clock};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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
/// duration, so the [`Clock`](super::Clock) contract's "advance `now()` by at least the
/// waited duration" does not hold above that ceiling. Nothing panics and
/// nothing fires immediately; the wait is merely shorter than asked. This is
/// reachable without absurd input — [`wait::exponential`](crate::wait::exponential)
/// saturates to [`Duration::MAX`] under [`stop::never`](crate::stop::never).
/// Bound the strategy with `.cap(...)` if a run must not depend on where that
/// ceiling sits.
///
/// # Not on `wasm32-unknown-unknown`
///
/// That target has no OS clock, and `tokio::time::Instant::now` panics there.
/// The type still exists but has no constructor and implements no clock
/// capability, so using it fails to compile. In a browser, use `GlooClock`
/// (feature `gloo-timers-clock`).
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), allow(dead_code))]
    origin: tokio::time::Instant,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Clock for TokioClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl AsyncClock for TokioClock {
    type Wait = tokio::time::Sleep;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        tokio::time::sleep(dur)
    }
}

// `tokio::time::Instant::now` compiles on wasm32-unknown-unknown but panics.
// This fails to compile if `TokioClock` ever implements `Clock` on that
// target, so the panic cannot return as a runtime surprise.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
const _: () = {
    use super::Clock;
    trait AmbiguousIfClock<A> {
        fn check() {}
    }
    struct IsClock;
    impl<T: ?Sized> AmbiguousIfClock<()> for T {}
    impl<T: ?Sized + Clock> AmbiguousIfClock<IsClock> for T {}
    let _ = <TokioClock as AmbiguousIfClock<_>>::check;
};
