use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;
use super::math::{
    clamp_exponential_base, pow_nonnegative_f64, saturating_duration_mul,
    saturating_duration_mul_f64,
};

/// A wait strategy that always returns the same duration.
///
/// Created by [`fixed`].
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::{Wait, WaitExt};
/// use core::time::Duration;
///
/// let mut w = wait::fixed(Duration::from_millis(100));
/// # let state = tenacious::RetryState {
/// #     attempt: 1, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// assert_eq!(w.next_wait(&state), Duration::from_millis(100));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaitFixed {
    duration: Duration,
}

/// Produces a strategy that always returns `dur` regardless of attempt number.
#[must_use]
pub fn fixed(dur: Duration) -> WaitFixed {
    WaitFixed { duration: dur }
}

impl Wait for WaitFixed {
    fn next_wait(&mut self, _state: &RetryState) -> Duration {
        self.duration
    }
}

/// A linearly increasing wait strategy.
///
/// Created by [`linear`]. The wait after attempt `n` is
/// `initial + (n - 1) * increment`. Overflow saturates at [`Duration::MAX`].
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::{Wait, WaitExt};
/// use core::time::Duration;
///
/// let mut w = wait::linear(Duration::from_millis(100), Duration::from_millis(50));
/// # let state = tenacious::RetryState {
/// #     attempt: 3, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// // 100ms + (3-1)*50ms = 200ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(200));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaitLinear {
    initial: Duration,
    increment: Duration,
}

/// Produces a linearly increasing strategy: `initial + (n - 1) * increment`.
///
/// Overflow saturates at [`Duration::MAX`].
#[must_use]
pub fn linear(initial: Duration, increment: Duration) -> WaitLinear {
    WaitLinear { initial, increment }
}

impl Wait for WaitLinear {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        let multiplier = state.attempt.saturating_sub(1);
        let step = saturating_duration_mul(self.increment, multiplier);
        self.initial.saturating_add(step)
    }
}

/// An exponentially increasing wait strategy.
///
/// Created by [`exponential`]. The wait after attempt `n` is
/// `initial * base^(n-1)` where `base` defaults to `2.0`. Overflow saturates
/// at [`Duration::MAX`].
///
/// Use [`.base(f)`](WaitExponential::base) to change the multiplier and
/// [`.cap(max)`](super::WaitExt::cap) to clamp the result.
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::{Wait, WaitExt};
/// use core::time::Duration;
///
/// let mut w = wait::exponential(Duration::from_millis(100));
/// # let state = tenacious::RetryState {
/// #     attempt: 3, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// // 100ms * 2^2 = 400ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(400));
/// ```
#[derive(Debug, Clone)]
pub struct WaitExponential {
    initial: Duration,
    base: f64,
}

/// The default exponential base multiplier.
const DEFAULT_EXPONENTIAL_BASE: f64 = 2.0;

/// Produces an exponentially increasing strategy: `initial * 2^(n-1)`.
///
/// Use [`.base(f)`](WaitExponential::base) to change the multiplier from `2`.
/// Overflow saturates at [`Duration::MAX`].
#[must_use]
pub fn exponential(initial: Duration) -> WaitExponential {
    WaitExponential {
        initial,
        base: DEFAULT_EXPONENTIAL_BASE,
    }
}

#[cfg(feature = "serde")]
fn deserialize_exponential_base(base: f64) -> Result<f64, &'static str> {
    if !base.is_finite() {
        return Err("wait::exponential base must be finite");
    }

    Ok(clamp_exponential_base(base))
}

impl WaitExponential {
    /// Changes the exponential base from the default of `2.0`.
    ///
    /// Values below `1.0` are clamped to `1.0` without panicking. A base of
    /// `1.0` produces a constant delay equal to `initial` on every attempt.
    #[must_use]
    pub fn base(mut self, base: f64) -> Self {
        self.base = clamp_exponential_base(base);
        self
    }
}

impl Wait for WaitExponential {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        let exponent = state.attempt.saturating_sub(1);
        // Avoid relying on `f64::powi`, which is not available in all no_std
        // environments. Exponentiation by squaring is O(log n) and saturates
        // to infinity on overflow.
        let multiplier = pow_nonnegative_f64(self.base, exponent);
        saturating_duration_mul_f64(self.initial, multiplier)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for WaitExponential {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("WaitExponential", 2)?;
        state.serialize_field("initial", &self.initial)?;
        state.serialize_field("base", &self.base)?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for WaitExponential {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SerializedWaitExponential {
            initial: Duration,
            base: f64,
        }

        let serialized = SerializedWaitExponential::deserialize(deserializer)?;
        let base =
            deserialize_exponential_base(serialized.base).map_err(serde::de::Error::custom)?;
        Ok(exponential(serialized.initial).base(base))
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_validation_tests {
    use super::*;

    const ARBITRARY_INITIAL_WAIT: Duration = Duration::from_millis(5);
    const NON_FINITE_BASE: f64 = f64::INFINITY;
    const SUBUNIT_BASE: f64 = 0.5;

    #[test]
    fn deserialize_exponential_base_rejects_non_finite() {
        let result = deserialize_exponential_base(NON_FINITE_BASE);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_exponential_base_clamps_subunit_values() {
        let mut strategy = exponential(ARBITRARY_INITIAL_WAIT)
            .base(deserialize_exponential_base(SUBUNIT_BASE).expect("base should parse"));
        let first = strategy.next_wait(&RetryState {
            attempt: 1,
            elapsed: None,
            next_delay: Duration::ZERO,
            total_wait: Duration::ZERO,
        });
        let second = strategy.next_wait(&RetryState {
            attempt: 2,
            elapsed: None,
            next_delay: Duration::ZERO,
            total_wait: Duration::ZERO,
        });
        assert_eq!(first, second);
    }
}
