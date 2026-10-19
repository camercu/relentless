use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;

/// A wait strategy that returns the same duration on every attempt.
///
/// Created by [`fixed`].
///
/// # Examples
///
/// ```
/// use relentless::wait;
/// use relentless::Wait;
/// use core::time::Duration;
///
/// let w = wait::fixed(Duration::from_millis(100));
/// # let state = relentless::RetryState::for_attempt(1);
/// assert_eq!(w.next_wait(&state), Duration::from_millis(100));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitFixed {
    duration: Duration,
}

/// Creates a wait strategy that waits a fixed duration between attempts.
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
/// use relentless::wait;
/// use relentless::Wait;
/// use core::time::Duration;
///
/// let w = wait::linear(Duration::from_millis(100), Duration::from_millis(50));
/// # let state = relentless::RetryState::for_attempt(3);
/// // 100ms + (3-1)*50ms = 200ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(200));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitLinear {
    initial: Duration,
    increment: Duration,
}

/// Creates a linearly increasing wait strategy.
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
/// use relentless::wait;
/// use relentless::Wait;
/// use core::time::Duration;
///
/// let w = wait::exponential(Duration::from_millis(100));
/// # let state = relentless::RetryState::for_attempt(3);
/// // 100ms * 2^2 = 400ms
/// assert_eq!(w.next_wait(&state), Duration::from_millis(400));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitExponential {
    initial: Duration,
    base: f64,
}

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

// Duration and exponent arithmetic shared by the strategies above: every
// operation saturates instead of overflowing or panicking.

/// The minimum allowed exponential base (values below this are clamped).
const MIN_EXPONENTIAL_BASE: f64 = 1.0;

fn clamp_exponential_base(base: f64) -> f64 {
    if !base.is_finite() {
        return MIN_EXPONENTIAL_BASE;
    }
    f64::max(base, MIN_EXPONENTIAL_BASE)
}

fn saturating_duration_mul(dur: Duration, mul: u32) -> Duration {
    dur.checked_mul(mul).unwrap_or(Duration::MAX)
}

/// Multiplies a `Duration` by an `f64`, saturating on overflow.
///
/// Returns `Duration::ZERO` for a zero duration (regardless of multiplier)
/// and for non-positive or NaN multipliers.
/// Returns `Duration::MAX` when the result overflows.
fn saturating_duration_mul_f64(dur: Duration, mul: f64) -> Duration {
    const NANOS_PER_SEC: u128 = 1_000_000_000;

    if dur == Duration::ZERO {
        return Duration::ZERO;
    }
    // NaN and non-positive multipliers need no guard: the `as u128` cast
    // below saturates NaN and negative products to 0, yielding ZERO. An
    // infinite multiplier produces an infinite product, caught by the
    // overflow check.
    let nanos = dur.as_nanos() as f64 * mul;
    // `Duration::MAX.as_nanos()` rounds *up* to the next f64, so any `nanos`
    // below this threshold truncates to at most u64::MAX whole seconds —
    // `Duration::new` below cannot overflow.
    if nanos >= Duration::MAX.as_nanos() as f64 {
        return Duration::MAX;
    }
    let total_nanos = nanos as u128;
    Duration::new(
        (total_nanos / NANOS_PER_SEC) as u64,
        (total_nanos % NANOS_PER_SEC) as u32,
    )
}

/// Raises `base` to an integer `exponent`, returning `f64::INFINITY` on overflow.
///
/// Bases below [`MIN_EXPONENTIAL_BASE`] are treated as `1.0`, so the
/// result is always `>= 1.0` for non-zero exponents.
fn pow_nonnegative_f64(base: f64, exponent: u32) -> f64 {
    if exponent == 0 {
        return 1.0;
    }
    if base <= MIN_EXPONENTIAL_BASE {
        return 1.0;
    }

    let mut result = 1.0;
    let mut factor = base;
    let mut remaining = exponent;

    loop {
        if remaining & 1 == 1 {
            result *= factor;
            if !result.is_finite() {
                return f64::INFINITY;
            }
        }
        remaining >>= 1;
        if remaining == 0 {
            return result;
        }
        factor *= factor;
        if !factor.is_finite() {
            factor = f64::INFINITY;
        }
    }
}
