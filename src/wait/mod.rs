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
pub use jitter::Jittered;
pub use jitter::decorrelated_jitter;
pub use strategies::{WaitExponential, WaitFixed, WaitLinear, exponential, fixed, linear};

/// Computes the delay duration between retry attempts.
///
/// Implementations receive the current [`RetryState`] (attempt count and
/// elapsed time) and return the duration to sleep before the next attempt.
/// Wait strategies never inspect the operation's outcome — they depend
/// only on timing and counting, so the same strategy can be reused across
/// any operation type.
///
/// Composition and builder methods are provided directly on the trait with
/// `where Self: Sized` bounds.
///
/// # Examples
///
/// ```
/// use relentless::{RetryState, Wait};
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
    /// Returns the duration to sleep before the next retry attempt.
    fn next_wait(&self, state: &RetryState) -> Duration;

    /// Returns the ceiling this strategy's output can never exceed, if it has
    /// one.
    ///
    /// `None` means "no ceiling known", which is the safe default and what a
    /// hand-written strategy gets for free. Decorators that can inflate their
    /// inner strategy's output — [`jitter`](Self::jitter) above all — consult
    /// it so a [`cap`](Self::cap) placed anywhere beneath them stays binding.
    /// That is what keeps `.cap(max).jitter(j)` capped in a generic function,
    /// behind `Box<dyn Wait>`, or under UFCS, where the concrete type is not
    /// visible and no inherent method can intervene (SPEC 3.3.8).
    ///
    /// Override it only for a strategy that truly cannot exceed the value
    /// returned; reporting a ceiling higher than reality is harmless, but
    /// reporting one lower than reality silently shortens delays.
    fn max_delay(&self) -> Option<Duration> {
        None
    }

    /// Clamps the returned duration to at most `max`.
    #[must_use]
    fn cap(self, max: Duration) -> WaitCapped<Self>
    where
        Self: Sized,
    {
        WaitCapped { inner: self, max }
    }

    /// Uses this strategy for the first `after` attempts, then switches to `other`.
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
    /// use relentless::{Wait, wait};
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
    fn jitter(self, max_jitter: Duration) -> Jittered<Self>
    where
        Self: Sized,
    {
        Jittered::additive(self, max_jitter)
    }

    /// Replaces the computed delay with a random value in `[0, base]`.
    ///
    /// This is the "Full Jitter" strategy from the [AWS Architecture Blog](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/).
    ///
    /// The random range spans at most `u64::MAX` nanoseconds (~584 years); a
    /// base larger than that is jittered only within that range. This ceiling
    /// is irrelevant for realistic retry delays.
    #[must_use]
    fn full_jitter(self) -> Jittered<Self>
    where
        Self: Sized,
    {
        Jittered::full(self)
    }

    /// Keeps half the computed delay and jitters the other half.
    ///
    /// This is the "Equal Jitter" strategy from the [AWS Architecture Blog](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/).
    ///
    /// As with [`full_jitter`](Self::full_jitter), the random half spans at most
    /// `u64::MAX` nanoseconds (~584 years) — irrelevant for realistic delays.
    #[must_use]
    fn equal_jitter(self) -> Jittered<Self>
    where
        Self: Sized,
    {
        Jittered::equal(self)
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

    fn max_delay(&self) -> Option<Duration> {
        (**self).max_delay()
    }
}

/// A shared reference to a wait strategy is itself one, so a builder can borrow
/// a strategy stored in a reusable [`RetryPolicy`](crate::RetryPolicy).
impl<W: Wait + ?Sized> Wait for &W {
    fn next_wait(&self, state: &RetryState) -> Duration {
        (**self).next_wait(state)
    }

    fn max_delay(&self) -> Option<Duration> {
        (**self).max_delay()
    }
}
