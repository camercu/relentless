//! The Embassy-backed async clock.

use super::{AsyncClock, Clock};
use crate::compat::Duration;

/// An [`AsyncClock`] backed by [`embassy_time`]'s tick clock and timer.
///
/// `now()` reads [`embassy_time::Instant`] and `wait_async` is
/// [`embassy_time::Timer::after`], both driven by the linked embassy time
/// driver.
#[derive(Debug, Clone, Copy)]
pub struct EmbassyClock {
    origin: embassy_time::Instant,
}

impl EmbassyClock {
    /// Creates a clock anchored at the current embassy instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: embassy_time::Instant::now(),
        }
    }
}

impl Default for EmbassyClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for EmbassyClock {
    fn now(&self) -> Duration {
        Duration::from_micros(self.origin.elapsed().as_micros())
    }
}

impl AsyncClock for EmbassyClock {
    type Wait = embassy_time::Timer;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        embassy_time::Timer::after(to_embassy_duration(dur))
    }
}

/// Embassy counts ticks in a `u64`; saturate rather than panic on very large
/// durations.
///
/// Computes ticks in `u128` (mirroring `embassy_time::Duration::from_micros`,
/// including its round-up-to-a-tick behavior) because Embassy's own `u64`
/// conversion arithmetic overflows near `u64::MAX` microseconds.
fn to_embassy_duration(dur: Duration) -> embassy_time::Duration {
    const MICROS_PER_SEC: u128 = 1_000_000;
    let ticks_ceil = dur
        .as_micros()
        .saturating_mul(u128::from(embassy_time::TICK_HZ))
        .div_ceil(MICROS_PER_SEC);
    let ticks = u64::try_from(ticks_ceil).unwrap_or(u64::MAX);
    embassy_time::Duration::from_ticks(ticks)
}

#[cfg(test)]
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
