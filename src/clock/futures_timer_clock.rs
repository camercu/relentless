//! The `futures-timer`-backed async clock.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use super::{AsyncClock, Clock};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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
    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), allow(dead_code))]
    origin: std::time::Instant,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl FuturesTimerClock {
    /// Creates a clock anchored at the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Default for FuturesTimerClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Clock for FuturesTimerClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl AsyncClock for FuturesTimerClock {
    type Wait = futures_timer::Delay;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        futures_timer::Delay::new(dur)
    }
}

// `Instant::now` compiles on wasm32-unknown-unknown but panics. This fails to
// compile if `FuturesTimerClock` ever implements `Clock` on that target, so
// the panic cannot return as a runtime surprise.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
const _: () = {
    use super::Clock;
    trait AmbiguousIfClock<A> {
        fn check() {}
    }
    struct IsClock;
    impl<T: ?Sized> AmbiguousIfClock<()> for T {}
    impl<T: ?Sized + Clock> AmbiguousIfClock<IsClock> for T {}
    let _ = <FuturesTimerClock as AmbiguousIfClock<_>>::check;
};
