//! Wait trait and built-in wait strategies.
//!
//! Wait strategies determine the delay between retry attempts. They compose
//! with `+` ([`WaitCombine`]) and chain via [`.chain()`](WaitChain).

use crate::compat::Duration;
use crate::state::RetryState;
#[cfg(feature = "alloc")]
use alloc::boxed::Box;
use core::ops::Add;

/// Computes the delay duration between retry attempts.
///
/// Implementations examine the current [`RetryState`] and return a
/// [`Duration`] representing how long to wait before the next attempt.
/// The state contains only timing and counting fields — wait strategies
/// never need to inspect the operation's outcome.
///
/// # Examples
///
/// ```
/// use tenacious::{Wait, RetryState};
/// use core::time::Duration;
///
/// struct FixedDelay(Duration);
///
/// impl Wait for FixedDelay {
///     fn next_wait(&mut self, _state: &RetryState) -> Duration {
///         self.0
///     }
/// }
/// ```
pub trait Wait {
    /// Returns the duration to wait before the next retry attempt.
    fn next_wait(&mut self, state: &RetryState) -> Duration;

    /// Resets internal state so the strategy can be reused across independent
    /// retry loops. The default implementation is a no-op.
    fn reset(&mut self) {}
}

#[cfg(feature = "alloc")]
impl<W> Wait for Box<W>
where
    W: Wait + ?Sized,
{
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        (**self).next_wait(state)
    }

    fn reset(&mut self) {
        (**self).reset();
    }
}

// ---------------------------------------------------------------------------
// Built-in strategies
// ---------------------------------------------------------------------------

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
/// let mut w = wait::fixed(Duration::from_millis(100));
/// # let state = tenacious::RetryState {
/// #     attempt: 1, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// assert_eq!(w.next_wait(&state), Duration::from_millis(100));
/// ```
#[derive(Debug, Clone)]
pub struct WaitFixed {
    duration: Duration,
}

/// Produces a strategy that always returns `dur` regardless of attempt number.
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
/// use tenacious::Wait;
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
pub struct WaitLinear {
    initial: Duration,
    increment: Duration,
}

/// Produces a linearly increasing strategy: `initial + (n - 1) * increment`.
///
/// Overflow saturates at [`Duration::MAX`].
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
/// [`.cap(max)`](WaitExponential::cap) to clamp the result.
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::Wait;
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

/// The minimum allowed exponential base (values below this are clamped).
const MIN_EXPONENTIAL_BASE: f64 = 1.0;

/// Produces an exponentially increasing strategy: `initial * 2^(n-1)`.
///
/// Use [`.base(f)`](WaitExponential::base) to change the multiplier from `2`.
/// Overflow saturates at [`Duration::MAX`].
pub fn exponential(initial: Duration) -> WaitExponential {
    WaitExponential {
        initial,
        base: DEFAULT_EXPONENTIAL_BASE,
    }
}

impl WaitExponential {
    /// Changes the exponential base from the default of `2.0`.
    ///
    /// Values below `1.0` are clamped to `1.0` without panicking. A base of
    /// `1.0` produces a constant delay equal to `initial` on every attempt.
    pub fn base(mut self, base: f64) -> Self {
        self.base = f64::max(base, MIN_EXPONENTIAL_BASE);
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

// ---------------------------------------------------------------------------
// Cap wrapper
// ---------------------------------------------------------------------------

/// A wrapper that clamps the inner strategy's output to a maximum duration.
///
/// Created by calling `.cap(max)` on any wait strategy.
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let mut w = wait::exponential(Duration::from_millis(100))
///     .cap(Duration::from_millis(500));
/// # let state = tenacious::RetryState {
/// #     attempt: 10, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// assert_eq!(w.next_wait(&state), Duration::from_millis(500));
/// ```
#[derive(Debug, Clone)]
pub struct WaitCapped<W> {
    inner: W,
    max: Duration,
}

impl<W: Wait> Wait for WaitCapped<W> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        self.inner.next_wait(state).min(self.max)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

// ---------------------------------------------------------------------------
// Composition: WaitCombine (Add) and WaitChain
// ---------------------------------------------------------------------------

/// Composite strategy that returns the **sum** of two strategies' outputs.
///
/// Created by combining two [`Wait`] strategies with `+`, or via
/// [`WaitCombine::new`]. Overflow saturates at [`Duration::MAX`].
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let mut w = wait::fixed(Duration::from_millis(100))
///     + wait::fixed(Duration::from_millis(50));
/// # let state = tenacious::RetryState {
/// #     attempt: 1, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// assert_eq!(w.next_wait(&state), Duration::from_millis(150));
/// ```
#[derive(Debug, Clone)]
pub struct WaitCombine<A, B> {
    left: A,
    right: B,
}

impl<A, B> WaitCombine<A, B> {
    /// Creates a composite that returns the sum of `left` and `right`.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Wait, B: Wait> Wait for WaitCombine<A, B> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        let left = self.left.next_wait(state);
        let right = self.right.next_wait(state);
        left.saturating_add(right)
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}

/// A strategy that uses one wait strategy for the first `after` attempts,
/// then switches to another.
///
/// Created by calling `.chain(other, after)` on a wait strategy.
///
/// # Examples
///
/// ```
/// use tenacious::wait;
/// use tenacious::Wait;
/// use core::time::Duration;
///
/// let mut w = wait::exponential(Duration::from_millis(100))
///     .chain(wait::fixed(Duration::from_secs(5)), 3);
/// # let state = tenacious::RetryState {
/// #     attempt: 4, elapsed: None,
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// // Attempt 4 > 3, so uses the fixed fallback.
/// assert_eq!(w.next_wait(&state), Duration::from_secs(5));
/// ```
#[derive(Debug, Clone)]
pub struct WaitChain<A, B> {
    first: A,
    second: B,
    after: u32,
}

impl<A, B> WaitChain<A, B> {
    /// Creates a chain that uses `first` for the first `after` attempts,
    /// then switches to `second`.
    pub fn new(first: A, second: B, after: u32) -> Self {
        Self {
            first,
            second,
            after,
        }
    }
}

impl<A: Wait, B: Wait> Wait for WaitChain<A, B> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        if state.attempt <= self.after {
            self.first.next_wait(state)
        } else {
            self.second.next_wait(state)
        }
    }

    fn reset(&mut self) {
        self.first.reset();
        self.second.reset();
    }
}

// ---------------------------------------------------------------------------
// Builder methods (.cap, .chain) and Add impls via macros
// ---------------------------------------------------------------------------

/// Generates `.cap()` and `.chain()` builder methods for a wait strategy type.
macro_rules! impl_wait_builders {
    ($($ty:ty),+ $(,)?) => {$(
        impl $ty {
            /// Clamps the computed wait to at most `max`.
            pub fn cap(self, max: Duration) -> WaitCapped<Self> {
                WaitCapped { inner: self, max }
            }

            /// Switches to `other` after `after` attempts.
            pub fn chain<W2>(self, other: W2, after: u32) -> WaitChain<Self, W2> {
                WaitChain::new(self, other, after)
            }
        }
    )+};
}

impl_wait_builders!(WaitFixed, WaitLinear, WaitExponential);

// Cap and chain on composite types (generic impls).
impl<A, B> WaitCombine<A, B> {
    /// Clamps the combined output to at most `max`.
    pub fn cap(self, max: Duration) -> WaitCapped<Self> {
        WaitCapped { inner: self, max }
    }

    /// Switches to `other` after `after` attempts.
    pub fn chain<W2>(self, other: W2, after: u32) -> WaitChain<Self, W2> {
        WaitChain::new(self, other, after)
    }
}

impl<A, B> WaitChain<A, B> {
    /// Clamps the chain's output to at most `max`.
    pub fn cap(self, max: Duration) -> WaitCapped<Self> {
        WaitCapped { inner: self, max }
    }
}

impl<W> WaitCapped<W> {
    /// Switches to `other` after `after` attempts.
    pub fn chain<W2>(self, other: W2, after: u32) -> WaitChain<Self, W2> {
        WaitChain::new(self, other, after)
    }
}

/// Generates `Add<Rhs>` impl for a concrete (non-generic) [`Wait`] type,
/// producing a [`WaitCombine`].
macro_rules! impl_wait_add {
    ($($ty:ty),+ $(,)?) => {$(
        impl<Rhs: Wait> Add<Rhs> for $ty {
            type Output = WaitCombine<Self, Rhs>;

            fn add(self, rhs: Rhs) -> Self::Output {
                WaitCombine::new(self, rhs)
            }
        }
    )+};
}

impl_wait_add!(WaitFixed, WaitLinear, WaitExponential);

// Add impl for generic composite types.
impl<A: Wait, B: Wait, Rhs: Wait> Add<Rhs> for WaitCombine<A, B> {
    type Output = WaitCombine<Self, Rhs>;

    fn add(self, rhs: Rhs) -> Self::Output {
        WaitCombine::new(self, rhs)
    }
}

impl<A: Wait, B: Wait, Rhs: Wait> Add<Rhs> for WaitChain<A, B> {
    type Output = WaitCombine<Self, Rhs>;

    fn add(self, rhs: Rhs) -> Self::Output {
        WaitCombine::new(self, rhs)
    }
}

impl<W: Wait, Rhs: Wait> Add<Rhs> for WaitCapped<W> {
    type Output = WaitCombine<Self, Rhs>;

    fn add(self, rhs: Rhs) -> Self::Output {
        WaitCombine::new(self, rhs)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Multiplies a `Duration` by a `u32`, saturating on overflow.
fn saturating_duration_mul(dur: Duration, mul: u32) -> Duration {
    dur.checked_mul(mul).unwrap_or(Duration::MAX)
}

/// Multiplies a `Duration` by an `f64`, saturating on overflow.
///
/// Returns `Duration::ZERO` for non-positive or NaN multipliers.
/// Returns `Duration::MAX` when the result overflows.
fn saturating_duration_mul_f64(dur: Duration, mul: f64) -> Duration {
    if !mul.is_finite() || mul <= 0.0 {
        if mul == f64::INFINITY {
            return Duration::MAX;
        }
        return Duration::ZERO;
    }
    let nanos = dur.as_nanos() as f64 * mul;
    if nanos >= Duration::MAX.as_nanos() as f64 {
        return Duration::MAX;
    }
    let total_nanos = nanos as u128;
    const NANOS_PER_SEC: u128 = 1_000_000_000;
    if total_nanos / NANOS_PER_SEC > u64::MAX as u128 {
        return Duration::MAX;
    }
    Duration::new(
        (total_nanos / NANOS_PER_SEC) as u64,
        (total_nanos % NANOS_PER_SEC) as u32,
    )
}

/// Raises a non-negative base to a non-negative integer exponent.
///
/// Uses exponentiation by squaring and returns infinity on overflow.
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

    while remaining > 0 {
        if remaining & 1 == 1 {
            result *= factor;
            if !result.is_finite() {
                return f64::INFINITY;
            }
        }
        remaining >>= 1;
        if remaining > 0 {
            factor *= factor;
            if !factor.is_finite() {
                factor = f64::INFINITY;
            }
        }
    }

    result
}
