//! The `gloo-timers`-backed async clock (wasm32 only).

use super::{AsyncClock, Clock};
use crate::compat::Duration;

/// An [`AsyncClock`] pairing `gloo-timers` waits with a caller-supplied `now`
/// source (wasm32 only).
///
/// Wasm has no `std::time::Instant`, so the monotonic reader must be supplied
/// explicitly — e.g. a closure over a `web_sys` `Performance` handle whose
/// `performance.now()` reading is converted to a [`Duration`]. The supplied
/// function must be monotonically non-decreasing and must observe real time
/// passing during the `gloo` waits, or `timeout`/`stop::elapsed` will
/// misbehave.
#[derive(Debug, Clone, Copy)]
pub struct GlooClock<F = fn() -> Duration> {
    now: F,
}

impl<F: Fn() -> Duration> GlooClock<F> {
    /// Creates a clock waiting through `gloo-timers` and reading `now` —
    /// any `Fn() -> Duration`, including closures capturing state.
    #[must_use]
    pub fn with_now(now: F) -> Self {
        Self { now }
    }
}

impl<F: Fn() -> Duration> Clock for GlooClock<F> {
    fn now(&self) -> Duration {
        (self.now)()
    }
}

impl<F: Fn() -> Duration> AsyncClock for GlooClock<F> {
    type Wait = gloo_timers::future::TimeoutFuture;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        // `gloo`'s `sleep` helper panics past u32::MAX milliseconds, and
        // anything above i32::MAX milliseconds (~24.8 days) reaches JS
        // `setTimeout` as a negative number, which the platform clamps to an
        // *immediate* fire. Saturate to the largest wait the platform can
        // actually perform. A wait must never panic, and an immediately
        // completing "wait" would violate the Clock contract.
        const MAX_TIMEOUT_MILLIS: u128 = i32::MAX as u128;
        let millis = u32::try_from(dur.as_millis().min(MAX_TIMEOUT_MILLIS))
            .expect("value clamped to i32::MAX fits in u32");
        gloo_timers::future::TimeoutFuture::new(millis)
    }
}
