use crate::compat::Duration;
use core::future::Future;

/// Async sleep abstraction used by the retry engine between attempts.
///
/// Only the async execution engine uses this trait; the sync engine calls a
/// blocking sleep function directly.
///
/// # Examples
///
/// ```
/// use relentless::Sleeper;
/// use core::time::Duration;
/// use core::future::Future;
/// use core::pin::Pin;
/// use core::task::{Context, Poll};
///
/// struct NoOpSleep;
///
/// impl Future for NoOpSleep {
///     type Output = ();
///     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
///         Poll::Ready(())
///     }
/// }
///
/// struct NoOpSleeper;
///
/// impl Sleeper for NoOpSleeper {
///     type Sleep = NoOpSleep;
///     fn sleep(&self, _dur: Duration) -> Self::Sleep {
///         NoOpSleep
///     }
/// }
/// ```
pub trait Sleeper {
    /// The future returned by [`sleep`](Sleeper::sleep).
    type Sleep: Future<Output = ()>;

    /// Returns a future that completes after `dur`.
    fn sleep(&self, dur: Duration) -> Self::Sleep;
}

/// Blanket implementation so runtime sleep functions (e.g. `tokio::time::sleep`)
/// can be passed to `.sleep(...)` directly without a wrapper struct.
impl<F, Fut> Sleeper for F
where
    F: Fn(Duration) -> Fut,
    Fut: Future<Output = ()>,
{
    type Sleep = Fut;

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        (self)(dur)
    }
}

/// Returns `tokio::time::sleep` as a [`Sleeper`]-compatible function.
#[cfg(feature = "tokio-sleep")]
#[must_use]
pub fn tokio() -> fn(Duration) -> tokio::time::Sleep {
    tokio::time::sleep
}

/// Returns `gloo_timers::future::sleep` as a [`Sleeper`]-compatible function.
#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
#[must_use]
pub fn gloo() -> fn(Duration) -> gloo_timers::future::TimeoutFuture {
    gloo_timers::future::sleep
}

/// Returns `futures_timer::Delay::new` as a [`Sleeper`]-compatible function.
#[cfg(feature = "futures-timer-sleep")]
#[must_use]
pub fn futures_timer() -> fn(Duration) -> futures_timer::Delay {
    futures_timer::Delay::new
}

/// Returns an Embassy sleep function as a [`Sleeper`]-compatible function.
#[cfg(feature = "embassy-sleep")]
#[must_use]
pub fn embassy() -> fn(Duration) -> embassy_time::Timer {
    embassy_sleep_fn
}

#[cfg(feature = "embassy-sleep")]
fn embassy_sleep_fn(dur: Duration) -> embassy_time::Timer {
    embassy_time::Timer::after(to_embassy_duration(dur))
}

/// Embassy counts ticks in a `u64`; saturate rather than panic on very large durations.
///
/// Computes ticks in `u128` (mirroring `embassy_time::Duration::from_micros`,
/// including its round-up-to-a-tick behavior) because Embassy's own `u64`
/// conversion arithmetic overflows near `u64::MAX` microseconds.
#[cfg(feature = "embassy-sleep")]
fn to_embassy_duration(dur: Duration) -> embassy_time::Duration {
    const MICROS_PER_SEC: u128 = 1_000_000;
    let ticks_ceil = dur
        .as_micros()
        .saturating_mul(u128::from(embassy_time::TICK_HZ))
        .div_ceil(MICROS_PER_SEC);
    let ticks = u64::try_from(ticks_ceil).unwrap_or(u64::MAX);
    embassy_time::Duration::from_ticks(ticks)
}

#[cfg(all(test, feature = "embassy-sleep"))]
mod tests {
    use super::to_embassy_duration;
    use crate::compat::Duration;

    const ARBITRARY_MICROS: u64 = 1_500;

    #[test]
    fn to_embassy_duration_preserves_micros() {
        assert_eq!(
            to_embassy_duration(Duration::from_micros(ARBITRARY_MICROS)),
            embassy_time::Duration::from_micros(ARBITRARY_MICROS)
        );
    }

    /// Embassy counts ticks in a `u64`; larger core durations must clamp to
    /// Embassy's maximum rather than truncate or panic. (Regression: clamping
    /// *microseconds* to `u64::MAX` still panicked, because Embassy's
    /// `from_micros` ceiling-division overflows at that input.)
    #[test]
    fn to_embassy_duration_saturates_at_embassy_max() {
        assert_eq!(
            to_embassy_duration(Duration::MAX),
            embassy_time::Duration::MAX
        );
    }

    proptest::proptest! {
        /// The conversion is total: no core `Duration` may panic it, at any
        /// tick rate. (The regression above lived exactly at this edge.)
        #[test]
        fn to_embassy_duration_never_panics(secs in proptest::prelude::any::<u64>(), nanos in 0..1_000_000_000_u32) {
            let _ = to_embassy_duration(Duration::new(secs, nanos));
        }
    }
}
