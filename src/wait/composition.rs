use core::ops::Add;

use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;
#[cfg(feature = "jitter")]
use super::WaitJitter;
use super::strategies::{WaitExponential, WaitFixed, WaitLinear};

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
    pub(super) inner: W,
    pub(super) max: Duration,
}

impl<W: Wait> Wait for WaitCapped<W> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        self.inner.next_wait(state).min(self.max)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

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
