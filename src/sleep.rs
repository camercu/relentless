//! Sleeper trait — abstracts how delays are performed.

use crate::compat::Duration;
use core::future::Future;

/// Abstracts the mechanism for sleeping/delaying between retry attempts.
///
/// The async execution engine calls [`Sleeper::sleep`] and `.await`s the
/// returned future. The sync execution engine uses a blocking sleep function
/// directly and does not use this trait.
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
    /// The future type returned by [`sleep`](Sleeper::sleep).
    type Sleep: Future<Output = ()>;

    /// Returns a future that completes after the given duration.
    fn sleep(&self, dur: Duration) -> Self::Sleep;
}

/// Blanket implementation for any `Fn(Duration) -> Fut` where
/// `Fut: Future<Output = ()>`. This allows passing runtime sleep functions
/// (e.g. `tokio::time::sleep`) directly without a wrapper struct.
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

/// Tokio sleep re-export convenience.
///
/// Enabled with the `tokio-sleep` feature. Equivalent to `tokio::time::sleep`.
#[cfg(feature = "tokio-sleep")]
pub use tokio::time::sleep as tokio_sleep;

/// Gloo timers sleep re-export convenience.
///
/// Enabled with the `gloo-timers-sleep` feature. Equivalent to
/// `gloo_timers::future::sleep`.
#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
pub use gloo_timers::future::sleep as gloo_sleep;

/// Futures timer sleep convenience.
///
/// Enabled with the `futures-timer-sleep` feature. Equivalent to
/// `futures_timer::Delay::new`.
#[cfg(feature = "futures-timer-sleep")]
pub fn futures_timer_sleep(dur: Duration) -> futures_timer::Delay {
    futures_timer::Delay::new(dur)
}

/// Zero-sized embassy sleeper implementation.
///
/// Enabled with the `embassy-sleep` feature.
#[cfg(feature = "embassy-sleep")]
#[derive(Debug, Clone, Copy, Default)]
pub struct EmbassySleep;

/// Embassy sleeper value for ergonomic `.sleep(embassy_sleep)` usage.
#[cfg(feature = "embassy-sleep")]
#[allow(non_upper_case_globals)]
pub const embassy_sleep: EmbassySleep = EmbassySleep;

#[cfg(feature = "embassy-sleep")]
impl Sleeper for EmbassySleep {
    type Sleep = embassy_time::Timer;

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        embassy_time::Timer::after(to_embassy_duration(dur))
    }
}

/// Converts core `Duration` to embassy `Duration`, saturating on overflow.
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
