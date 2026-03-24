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
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let w = wait::fixed(Duration::from_millis(100));
/// # let state = tenacious::RetryState::new(1, None);
/// assert_eq!(w.next_wait(&state), Duration::from_millis(100));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitFixed {
    duration: Duration,
}

/// Produces a strategy that always returns `dur` regardless of attempt number.
#[must_use]
pub fn fixed(dur: Duration) -> WaitFixed {
    WaitFixed { duration: dur }
}

impl Wait for WaitFixed {
    fn next_wait(&self, _state: &RetryState) -> Duration {
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
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let w = wait::linear(Duration::from_millis(100), Duration::from_millis(50));
/// # let state = tenacious::RetryState::new(3, None);
/// // 100ms + (3-1)*50ms = 200ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(200));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    fn next_wait(&self, state: &RetryState) -> Duration {
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
/// [`.cap(max)`](super::Wait::cap) to clamp the result.
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let w = wait::exponential(Duration::from_millis(100));
/// # let state = tenacious::RetryState::new(3, None);
/// // 100ms * 2^2 = 400ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(400));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
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

impl WaitExponential {
    /// Changes the exponential base from the default of `2.0`.
    ///
    /// Non-finite values (`NaN`, `Infinity`) are clamped to `1.0`.
    /// Values below `1.0` are clamped to `1.0` without panicking. A base of
    /// `1.0` produces a constant delay equal to `initial` on every attempt.
    #[must_use]
    pub fn base(mut self, base: f64) -> Self {
        self.base = clamp_exponential_base(base);
        self
    }
}

impl Wait for WaitExponential {
    fn next_wait(&self, state: &RetryState) -> Duration {
        let exponent = state.attempt.saturating_sub(1);
        let multiplier = pow_nonnegative_f64(self.base, exponent);
        saturating_duration_mul_f64(self.initial, multiplier)
    }
}
