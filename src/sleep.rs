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
/// use tenacious::Sleeper;
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

/// Embassy uses microseconds as a `u64`; saturate rather than panic on very large durations.
#[cfg(feature = "embassy-sleep")]
fn to_embassy_duration(dur: Duration) -> embassy_time::Duration {
    const MAX_U64_AS_U128: u128 = u64::MAX as u128;
    let micros_u128 = dur.as_micros();
    let micros = if micros_u128 > MAX_U64_AS_U128 {
        u64::MAX
    } else {
        micros_u128 as u64
    };
    embassy_time::Duration::from_micros(micros)
}
