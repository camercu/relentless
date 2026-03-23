//! Stop trait and built-in stop strategies.
//!
//! Stop strategies determine when the retry loop should give up. They compose
//! with `|` ([`StopAny`]) and `&` ([`StopAll`]), or via named methods
//! `.or()` and `.and()` on the [`Stop`] trait.

#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::state::RetryState;

mod composition;
mod strategies;

pub use composition::{StopAll, StopAny};
pub use strategies::{StopAfterAttempts, StopAfterElapsed, StopNever, attempts, elapsed, never};

/// Determines when the retry loop should stop.
///
/// Implementations examine the current [`RetryState`] and return `true` when
/// no more attempts should be made. The state contains only timing and counting
/// fields — stop strategies never need to inspect the operation's outcome.
///
/// Composition methods are provided directly on the trait with
/// `where Self: Sized` bounds, following the `Iterator` pattern.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryState, Stop};
///
/// struct StopAfterThree;
///
/// impl Stop for StopAfterThree {
///     fn should_stop(&self, state: &RetryState) -> bool {
///         const MAX_ATTEMPTS: u32 = 3;
///         state.attempt >= MAX_ATTEMPTS
///     }
/// }
/// ```
pub trait Stop {
    /// Returns `true` if the retry loop should stop after examining the
    /// current retry state.
    fn should_stop(&self, state: &RetryState) -> bool;

    /// Returns a strategy that stops when either side stops.
    #[must_use]
    fn or<S: Stop>(self, other: S) -> StopAny<Self, S>
    where
        Self: Sized,
    {
        StopAny::new(self, other)
    }

    /// Returns a strategy that stops only when both sides stop.
    #[must_use]
    fn and<S: Stop>(self, other: S) -> StopAll<Self, S>
    where
        Self: Sized,
    {
        StopAll::new(self, other)
    }
}

#[cfg(feature = "alloc")]
impl<S> Stop for Box<S>
where
    S: Stop + ?Sized,
{
    fn should_stop(&self, state: &RetryState) -> bool {
        (**self).should_stop(state)
    }
}
