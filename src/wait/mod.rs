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

    /// Returns a ceiling **imposed** on this strategy, if one has been.
    ///
    /// This reports a deliberate bound — what [`cap`](Self::cap) creates — and
    /// not merely the largest value the strategy happens to produce. The
    /// distinction is the whole contract: decorators that can inflate their
    /// inner strategy's output, [`jitter`](Self::jitter) above all, clamp
    /// themselves to whatever this returns, so a `.cap(max)` stays binding in
    /// a generic function, behind `Box<dyn Wait>`, or under UFCS, where the
    /// concrete type is not visible and no inherent method can intervene
    /// (SPEC 3.3.8).
    ///
    /// `None` means "no ceiling imposed" and is the correct answer for every
    /// undecorated strategy, including a deterministic one. Do **not** report
    /// a strategy's own output here: making [`fixed`]
    /// return `Some(d)` looks like free precision and instead clamps
    /// `wait::fixed(d).jitter(j)` to `d`, silently deleting the jitter. The
    /// suite fails loudly if you try it.
    ///
    /// A cap bounds the strategy it encloses, and nothing further. Composing
    /// with [`chain`](Self::chain) or [`add`](Self::add) builds a *new*
    /// strategy that no cap was applied to, so the composites report `None`
    /// even when a branch is capped — cap the composite itself if you want a
    /// bound over it. Reporting a ceiling above a genuinely imposed one is
    /// harmless; reporting one below it silently shortens delays.
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
