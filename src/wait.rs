//! Wait trait and built-in wait strategies.
//!
//! Wait strategies determine the delay between retry attempts. They compose
//! with `+` ([`WaitCombine`]) and chain via [`.chain()`](WaitChain).

#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::compat::Duration;
use crate::state::RetryState;
use core::ops::Add;
#[cfg(all(feature = "jitter", target_has_atomic = "ptr"))]
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "jitter")]
use rand::{Rng, SeedableRng, rngs::SmallRng};

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

/// Extension methods for any [`Wait`] strategy.
///
/// This trait enables fluent composition for custom wait strategies that
/// implement [`Wait`], not only the built-in wait types in this module.
pub trait WaitExt: Wait + Sized {
    /// Clamps the computed wait to at most `max`.
    #[must_use]
    fn cap(self, max: Duration) -> WaitCapped<Self> {
        WaitCapped { inner: self, max }
    }

    /// Switches to `other` after `after` attempts.
    #[must_use]
    fn chain<W2>(self, other: W2, after: u32) -> WaitChain<Self, W2> {
        WaitChain::new(self, other, after)
    }

    /// Adds uniformly distributed jitter in `[0, max_jitter]`.
    #[cfg(feature = "jitter")]
    #[must_use]
    fn jitter(self, max_jitter: Duration) -> WaitJitter<Self> {
        WaitJitter::new(self, max_jitter)
    }
}

impl<W> WaitExt for W where W: Wait + Sized {}

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
/// [`.cap(max)`](WaitExt::cap) to clamp the result.
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaitExponential {
    initial: Duration,
    base: f64,
}

/// The default exponential base multiplier.
const DEFAULT_EXPONENTIAL_BASE: f64 = 2.0;

/// The minimum allowed exponential base (values below this are clamped).
const MIN_EXPONENTIAL_BASE: f64 = 1.0;

/// Fixed seed used by jitter-enabled wait strategies.
#[cfg(feature = "jitter")]
const DEFAULT_JITTER_SEED: [u8; 32] = [0x5A; 32];

/// Monotonic jitter nonce counter used to decorrelate independent policies.
#[cfg(all(feature = "jitter", target_has_atomic = "ptr"))]
static JITTER_NONCE_COUNTER: AtomicUsize = AtomicUsize::new(1);

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
    /// Values below `1.0` are clamped to `1.0` without panicking. A base of
    /// `1.0` produces a constant delay equal to `initial` on every attempt.
    #[must_use]
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
// Jitter wrapper (feature-gated)
// ---------------------------------------------------------------------------

/// A wrapper that adds uniformly distributed jitter in `[0, max_jitter]` to
/// the inner strategy output.
///
/// Enabled with the `jitter` feature and created by calling `.jitter(max)` on
/// any wait strategy.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "jitter")]
/// # {
/// use tenacious::{Wait, WaitExt, RetryState, wait};
/// use core::time::Duration;
///
/// let mut strategy = wait::fixed(Duration::from_millis(50))
///     .jitter(Duration::from_millis(10));
/// let state = RetryState {
///     attempt: 1,
///     elapsed: None,
///     next_delay: Duration::ZERO,
///     total_wait: Duration::ZERO,
/// };
///
/// let next = strategy.next_wait(&state);
/// assert!(next >= Duration::from_millis(50));
/// assert!(next <= Duration::from_millis(60));
/// # }
/// ```
#[cfg(feature = "jitter")]
#[derive(Debug, Clone)]
pub struct WaitJitter<W> {
    inner: W,
    max_jitter: Duration,
    nonce: u64,
    rng: SmallRng,
}

#[cfg(feature = "jitter")]
impl<W> WaitJitter<W> {
    fn new(inner: W, max_jitter: Duration) -> Self {
        Self {
            inner,
            max_jitter,
            nonce: next_jitter_nonce(),
            rng: SmallRng::from_seed(DEFAULT_JITTER_SEED),
        }
    }
}

#[cfg(feature = "jitter")]
impl<W: Wait> Wait for WaitJitter<W> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        let base = self.inner.next_wait(state);
        let jitter = random_jitter_duration(self.max_jitter, &mut self.rng, self.nonce);
        base.saturating_add(jitter)
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.nonce = self.nonce.wrapping_add(1);
        self.rng = SmallRng::from_seed(DEFAULT_JITTER_SEED);
    }
}

#[cfg(all(feature = "jitter", feature = "serde"))]
impl<W> serde::Serialize for WaitJitter<W>
where
    W: serde::Serialize,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("WaitJitter", 2)?;
        state.serialize_field("inner", &self.inner)?;
        state.serialize_field("max_jitter", &self.max_jitter)?;
        state.end()
    }
}

#[cfg(all(feature = "jitter", feature = "serde"))]
impl<'de, W> serde::Deserialize<'de> for WaitJitter<W>
where
    W: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SerializedWaitJitter<W> {
            inner: W,
            max_jitter: Duration,
        }

        let serialized = SerializedWaitJitter::deserialize(deserializer)?;
        Ok(Self {
            inner: serialized.inner,
            max_jitter: serialized.max_jitter,
            nonce: next_jitter_nonce(),
            rng: SmallRng::from_seed(DEFAULT_JITTER_SEED),
        })
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
/// use tenacious::{Wait, WaitExt};
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
/// use tenacious::{Wait, WaitExt};
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaitCombine<A, B> {
    left: A,
    right: B,
}

impl<A, B> WaitCombine<A, B> {
    /// Creates a composite that returns the sum of `left` and `right`.
    #[must_use]
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
/// use tenacious::{Wait, WaitExt};
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaitChain<A, B> {
    first: A,
    second: B,
    after: u32,
}

impl<A, B> WaitChain<A, B> {
    /// Creates a chain that uses `first` for the first `after` attempts,
    /// then switches to `second`.
    #[must_use]
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

impl<W> WaitCapped<W> {
    /// Adds jitter while preserving cap-after-jitter semantics.
    ///
    /// Even when called after `.cap(max)`, the cap remains the final operation.
    #[cfg(feature = "jitter")]
    #[must_use]
    pub fn jitter(self, max_jitter: Duration) -> WaitCapped<WaitJitter<W>> {
        let WaitCapped { inner, max } = self;
        WaitCapped {
            inner: WaitJitter::new(inner, max_jitter),
            max,
        }
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

#[cfg(feature = "jitter")]
impl<W: Wait, Rhs: Wait> Add<Rhs> for WaitJitter<W> {
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

/// Generates a random jitter duration in `[0, max_jitter]`.
#[cfg(feature = "jitter")]
fn random_jitter_duration(max_jitter: Duration, rng: &mut SmallRng, nonce: u64) -> Duration {
    if max_jitter.is_zero() {
        return Duration::ZERO;
    }

    const MAX_JITTER_NANOS: u128 = u64::MAX as u128;
    let upper = max_jitter.as_nanos().min(MAX_JITTER_NANOS) as u64;
    let random = rng.gen_range(0..=upper);
    let offset = nonce;
    let adjusted = if upper == u64::MAX {
        random.wrapping_add(offset)
    } else {
        let modulus = upper + 1;
        (random + (offset % modulus)) % modulus
    };

    Duration::from_nanos(adjusted)
}

#[cfg(all(feature = "jitter", target_has_atomic = "ptr"))]
fn next_jitter_nonce() -> u64 {
    let counter = JITTER_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed) as u64;

    #[cfg(feature = "std")]
    {
        use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(StdDuration::ZERO);
        counter ^ (now.as_nanos() as u64)
    }

    #[cfg(not(feature = "std"))]
    {
        counter
    }
}

#[cfg(all(feature = "jitter", not(target_has_atomic = "ptr")))]
fn next_jitter_nonce() -> u64 {
    1
}
