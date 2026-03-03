//! Wait trait and built-in wait strategies.
//!
//! Wait strategies determine the delay between retry attempts. They compose
//! with `+` ([`WaitCombine`]) and chain via [`.chain()`](WaitChain).

#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::compat::Duration;
use crate::state::RetryState;

mod composition;
mod math;
mod strategies;

#[cfg(feature = "jitter")]
mod jitter;

pub use composition::{WaitCapped, WaitChain, WaitCombine};
#[cfg(feature = "jitter")]
pub use jitter::WaitJitter;
pub use strategies::{WaitExponential, WaitFixed, WaitLinear, exponential, fixed, linear};

/// Computes the delay duration between retry attempts.
///
/// Implementations examine the current [`RetryState`] and return a
/// [`Duration`] representing how long to wait before the next attempt.
/// The state contains only timing and counting fields - wait strategies
/// never need to inspect the operation's outcome.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryState, Wait};
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
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::{RetryState, Wait, WaitExt, wait};
///
/// #[derive(Clone, Copy)]
/// struct StepWait {
///     base: Duration,
/// }
///
/// impl Wait for StepWait {
///     fn next_wait(&mut self, state: &RetryState) -> Duration {
///         self.base
///             .checked_mul(state.attempt)
///             .unwrap_or(Duration::MAX)
///     }
/// }
///
/// let mut strategy = StepWait {
///     base: Duration::from_millis(10),
/// }
/// .cap(Duration::from_millis(25))
/// .chain(wait::fixed(Duration::from_millis(30)), 2);
///
/// let state = RetryState {
///     attempt: 3,
///     elapsed: None,
///     next_delay: Duration::ZERO,
///     total_wait: Duration::ZERO,
/// };
/// assert_eq!(strategy.next_wait(&state), Duration::from_millis(30));
/// ```
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
