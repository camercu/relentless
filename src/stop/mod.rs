//! Stop trait and built-in stop strategies.
//!
//! Stop strategies determine when the retry loop should give up. They compose
//! with `|` ([`StopAny`]) and `&` ([`StopAll`]), or via named methods on
//! [`StopExt`].

#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::state::RetryState;

mod composition;
mod strategies;

pub use composition::{StopAll, StopAny};
pub use strategies::{
    NeedsStop, StopAfterAttempts, StopAfterElapsed, StopBeforeElapsed, StopConfigError, StopNever,
    attempts, attempts_checked, before_elapsed, elapsed, never,
};

/// Determines when the retry loop should stop.
///
/// Implementations examine the current [`RetryState`] and return `true` when
/// no more attempts should be made. The state contains only timing and counting
/// fields — stop strategies never need to inspect the operation's outcome.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryState, Stop};
///
/// struct StopAfterThree;
///
/// impl Stop for StopAfterThree {
///     fn should_stop(&mut self, state: &RetryState) -> bool {
///         const MAX_ATTEMPTS: u32 = 3;
///         state.attempt >= MAX_ATTEMPTS
///     }
/// }
/// ```
pub trait Stop {
    /// Returns `true` if the retry loop should stop after examining the
    /// current retry state.
    fn should_stop(&mut self, state: &RetryState) -> bool;

    /// Resets internal state so the strategy can be reused across independent
    /// retry loops. The default implementation is a no-op.
    fn reset(&mut self) {}
}

/// Ergonomic named combinators for [`Stop`] strategies.
///
/// These are equivalent to the operator forms:
/// - `.or(other)` is the same as `|`.
/// - `.and(other)` is the same as `&`.
pub trait StopExt: Stop + Sized {
    /// Returns a strategy that stops when either side stops.
    #[must_use]
    fn or<Rhs: Stop>(self, rhs: Rhs) -> StopAny<Self, Rhs> {
        StopAny::new(self, rhs)
    }

    /// Returns a strategy that stops only when both sides stop.
    #[must_use]
    fn and<Rhs: Stop>(self, rhs: Rhs) -> StopAll<Self, Rhs> {
        StopAll::new(self, rhs)
    }
}

impl<S> StopExt for S where S: Stop + Sized {}

#[cfg(feature = "alloc")]
impl<S> Stop for Box<S>
where
    S: Stop + ?Sized,
{
    fn should_stop(&mut self, state: &RetryState) -> bool {
        (**self).should_stop(state)
    }

    fn reset(&mut self) {
        (**self).reset();
    }
}
