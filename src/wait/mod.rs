//! Wait trait and built-in wait strategies.
//!
//! Wait strategies determine the delay between retry attempts. They compose
//! with `+` or [`.add()`](Wait::add), and chain via
//! [`.chain()`](Wait::chain).

#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::compat::Duration;
use crate::state::RetryState;

mod composition;
mod math;
mod strategies;

mod jitter;

pub use composition::{WaitCapped, WaitChain, WaitCombine};
pub use jitter::decorrelated_jitter;
pub use jitter::{WaitDecorrelatedJitter, WaitEqualJitter, WaitFullJitter, WaitJitter};
pub use strategies::{WaitExponential, WaitFixed, WaitLinear, exponential, fixed, linear};

/// Computes the delay duration between retry attempts.
///
/// Implementations examine the current [`RetryState`] and return a
/// [`Duration`] representing how long to wait before the next attempt.
/// The state contains only timing and counting fields - wait strategies
/// never need to inspect the operation's outcome.
///
/// Composition and builder methods are provided directly on the trait with
/// `where Self: Sized` bounds.
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
///     fn next_wait(&self, _state: &RetryState) -> Duration {
///         self.0
///     }
/// }
/// ```
pub trait Wait {
    /// Returns the duration to wait before the next retry attempt.
    fn next_wait(&self, state: &RetryState) -> Duration;

    /// Clamps the computed wait to at most `max`.
    #[must_use]
    fn cap(self, max: Duration) -> WaitCapped<Self>
    where
        Self: Sized,
    {
        WaitCapped { inner: self, max }
    }

    /// Switches to `other` after `after` attempts.
    #[must_use]
    fn chain<W2: Wait>(self, other: W2, after: u32) -> WaitChain<Self, W2>
    where
        Self: Sized,
    {
        WaitChain::new(self, other, after)
    }

    /// Adds another wait strategy to this one.
    ///
    /// This is the named equivalent of the `+` operator — returns
    /// the sum of both strategies' outputs (saturating on overflow).
    ///
    /// ```
    /// use tenacious::{Wait, wait};
    /// use core::time::Duration;
    ///
    /// // These are equivalent:
    /// let a = wait::fixed(Duration::from_millis(50)).add(wait::exponential(Duration::from_millis(100)));
    /// let b = wait::fixed(Duration::from_millis(50)) + wait::exponential(Duration::from_millis(100));
    /// ```
    #[must_use]
    fn add<W2: Wait>(self, other: W2) -> WaitCombine<Self, W2>
    where
        Self: Sized,
    {
        WaitCombine::new(self, other)
    }

    /// Adds uniformly distributed jitter in `[0, max_jitter]`.
    #[must_use]
    fn jitter(self, max_jitter: Duration) -> WaitJitter<Self>
    where
        Self: Sized,
    {
        WaitJitter::new(self, max_jitter)
    }

    /// Replaces the computed delay with a random value in `[0, base]`.
    ///
    /// This is the "Full Jitter" strategy from the AWS Architecture Blog.
    #[must_use]
    fn full_jitter(self) -> WaitFullJitter<Self>
    where
        Self: Sized,
    {
        WaitFullJitter::new(self)
    }

    /// Keeps half the computed delay and jitters the other half.
    ///
    /// This is the "Equal Jitter" strategy from the AWS Architecture Blog.
    #[must_use]
    fn equal_jitter(self) -> WaitEqualJitter<Self>
    where
        Self: Sized,
    {
        WaitEqualJitter::new(self)
    }
}

#[cfg(feature = "alloc")]
impl<W> Wait for Box<W>
where
    W: Wait + ?Sized,
{
    fn next_wait(&self, state: &RetryState) -> Duration {
        (**self).next_wait(state)
    }
}
