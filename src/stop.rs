//! Stop trait and built-in stop strategies.
//!
//! Stop strategies determine when the retry loop should give up. They compose
//! with `|` ([`StopAny`]) and `&` ([`StopAll`]).

use crate::compat::Duration;
use crate::state::RetryState;
#[cfg(feature = "alloc")]
use alloc::boxed::Box;
use core::ops::{BitAnd, BitOr};

/// Determines when the retry loop should stop.
///
/// Implementations examine the current [`RetryState`] and return `true` when
/// no more attempts should be made. The state contains only timing and counting
/// fields — stop strategies never need to inspect the operation's outcome.
///
/// # Examples
///
/// ```
/// use tenacious::{Stop, RetryState};
/// use core::time::Duration;
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

// ---------------------------------------------------------------------------
// Built-in strategies
// ---------------------------------------------------------------------------

/// Stops after a fixed number of completed attempts.
///
/// Created by [`attempts`]. Fires when `state.attempt >= max`.
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
///
/// let mut s = stop::attempts(3);
/// # let state = tenacious::RetryState {
/// #     attempt: 3, elapsed: None,
/// #     next_delay: core::time::Duration::ZERO,
/// #     total_wait: core::time::Duration::ZERO,
/// # };
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone)]
pub struct StopAfterAttempts {
    max: u32,
}

/// Produces a strategy that stops after `max` completed attempts.
///
/// The stop fires when `state.attempt >= max`.
pub fn attempts(max: u32) -> StopAfterAttempts {
    StopAfterAttempts { max }
}

impl Stop for StopAfterAttempts {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state.attempt >= self.max
    }
}

/// Stops when wall-clock elapsed time meets or exceeds a deadline.
///
/// Created by [`elapsed`]. When `state.elapsed` is `None` (no clock
/// available), this strategy never fires.
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
/// use core::time::Duration;
///
/// let mut s = stop::elapsed(Duration::from_secs(30));
/// # let state = tenacious::RetryState {
/// #     attempt: 1, elapsed: Some(Duration::from_secs(31)),
/// #     next_delay: Duration::ZERO, total_wait: Duration::ZERO,
/// # };
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone)]
pub struct StopAfterElapsed {
    deadline: Duration,
}

/// Produces a strategy that stops when `state.elapsed >= Some(deadline)`.
///
/// When no clock is available (`elapsed` is `None`), this strategy never fires.
pub fn elapsed(deadline: Duration) -> StopAfterElapsed {
    StopAfterElapsed { deadline }
}

impl Stop for StopAfterElapsed {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state
            .elapsed
            .is_some_and(|elapsed| elapsed >= self.deadline)
    }
}

/// Conservative stop strategy that fires when the next attempt would likely
/// exceed a deadline.
///
/// Created by [`before_elapsed`]. Fires when
/// `state.elapsed + state.next_delay >= deadline`. This prevents starting an
/// attempt that cannot complete within the time budget.
///
/// When `state.elapsed` is `None` (no clock), this strategy never fires.
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
/// use core::time::Duration;
///
/// let mut s = stop::before_elapsed(Duration::from_secs(10));
/// # let state = tenacious::RetryState {
/// #     attempt: 1, elapsed: Some(Duration::from_secs(9)),
/// #     next_delay: Duration::from_secs(2), total_wait: Duration::ZERO,
/// # };
/// assert!(s.should_stop(&state)); // 9s + 2s >= 10s
/// ```
#[derive(Debug, Clone)]
pub struct StopBeforeElapsed {
    deadline: Duration,
}

/// Produces a conservative strategy that stops when elapsed time plus the
/// next delay would meet or exceed `deadline`.
///
/// When no clock is available (`elapsed` is `None`), this strategy never fires.
pub fn before_elapsed(deadline: Duration) -> StopBeforeElapsed {
    StopBeforeElapsed { deadline }
}

impl Stop for StopBeforeElapsed {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state
            .elapsed
            .is_some_and(|elapsed| elapsed.saturating_add(state.next_delay) >= self.deadline)
    }
}

/// A strategy that never stops — the retry loop continues indefinitely.
///
/// Created by [`never`]. This is the correct explicit spelling of
/// "retry indefinitely."
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
///
/// let mut s = stop::never();
/// # let state = tenacious::RetryState {
/// #     attempt: u32::MAX, elapsed: None,
/// #     next_delay: core::time::Duration::ZERO,
/// #     total_wait: core::time::Duration::ZERO,
/// # };
/// assert!(!s.should_stop(&state));
/// ```
#[derive(Debug, Clone)]
pub struct StopNever;

/// Produces a strategy that always returns `false` — never stops.
pub fn never() -> StopNever {
    StopNever
}

impl Stop for StopNever {
    fn should_stop(&mut self, _state: &RetryState) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Composition: StopAny (BitOr) and StopAll (BitAnd)
// ---------------------------------------------------------------------------

/// Composite strategy that stops when **either** constituent stops.
///
/// Created by combining two [`Stop`] strategies with `|`, or via
/// [`StopAny::new`].
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
/// use core::time::Duration;
///
/// // Stop after 5 attempts OR after 30 seconds, whichever comes first.
/// let mut s = stop::attempts(5) | stop::elapsed(Duration::from_secs(30));
/// ```
#[derive(Debug, Clone)]
pub struct StopAny<A, B> {
    left: A,
    right: B,
}

impl<A, B> StopAny<A, B> {
    /// Creates a composite that stops when either `left` or `right` stops.
    ///
    /// This constructor is useful for composing custom [`Stop`] implementations
    /// that don't have `BitOr` operator overloads.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAny<A, B> {
    /// Returns `true` if **either** constituent says to stop.
    ///
    /// Both constituents are always evaluated (no short-circuit) so that
    /// stateful strategies on either side receive every call.
    fn should_stop(&mut self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left || right
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}

impl<A: Stop, B: Stop, Rhs: Stop> BitOr<Rhs> for StopAny<A, B> {
    type Output = StopAny<Self, Rhs>;

    fn bitor(self, rhs: Rhs) -> Self::Output {
        StopAny::new(self, rhs)
    }
}

impl<A: Stop, B: Stop, Rhs: Stop> BitAnd<Rhs> for StopAny<A, B> {
    type Output = StopAll<Self, Rhs>;

    fn bitand(self, rhs: Rhs) -> Self::Output {
        StopAll::new(self, rhs)
    }
}

/// Composite strategy that stops only when **both** constituents stop.
///
/// Created by combining two [`Stop`] strategies with `&`, or via
/// [`StopAll::new`].
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use tenacious::stop;
/// use tenacious::Stop;
/// use core::time::Duration;
///
/// // Stop only when BOTH conditions are true.
/// let mut s = stop::attempts(5) & stop::elapsed(Duration::from_secs(30));
/// ```
#[derive(Debug, Clone)]
pub struct StopAll<A, B> {
    left: A,
    right: B,
}

impl<A, B> StopAll<A, B> {
    /// Creates a composite that stops only when both `left` and `right` stop.
    ///
    /// This constructor is useful for composing custom [`Stop`] implementations
    /// that don't have `BitAnd` operator overloads.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAll<A, B> {
    /// Returns `true` only when **both** constituents say to stop.
    ///
    /// Both constituents are always evaluated (no short-circuit) so that
    /// stateful strategies on either side receive every call.
    fn should_stop(&mut self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left && right
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}

impl<A: Stop, B: Stop, Rhs: Stop> BitAnd<Rhs> for StopAll<A, B> {
    type Output = StopAll<Self, Rhs>;

    fn bitand(self, rhs: Rhs) -> Self::Output {
        StopAll::new(self, rhs)
    }
}

impl<A: Stop, B: Stop, Rhs: Stop> BitOr<Rhs> for StopAll<A, B> {
    type Output = StopAny<Self, Rhs>;

    fn bitor(self, rhs: Rhs) -> Self::Output {
        StopAny::new(self, rhs)
    }
}

// ---------------------------------------------------------------------------
// BitOr / BitAnd for built-in concrete types and composite types
// ---------------------------------------------------------------------------

/// Generates `BitOr` and `BitAnd` impls for a concrete (non-generic) [`Stop`] type,
/// producing [`StopAny`] and [`StopAll`] composites respectively.
macro_rules! impl_stop_ops {
    ($($ty:ty),+ $(,)?) => {$(
        impl<Rhs: Stop> BitOr<Rhs> for $ty {
            type Output = StopAny<Self, Rhs>;

            fn bitor(self, rhs: Rhs) -> Self::Output {
                StopAny::new(self, rhs)
            }
        }

        impl<Rhs: Stop> BitAnd<Rhs> for $ty {
            type Output = StopAll<Self, Rhs>;

            fn bitand(self, rhs: Rhs) -> Self::Output {
                StopAll::new(self, rhs)
            }
        }
    )+};
}

impl_stop_ops!(
    StopAfterAttempts,
    StopAfterElapsed,
    StopBeforeElapsed,
    StopNever
);
