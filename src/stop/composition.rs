use super::Stop;
use super::strategies::{StopAfterAttempts, StopAfterElapsed, StopNever};
use crate::state::RetryState;
use core::ops::{BitAnd, BitOr};

/// Composite strategy that stops when **either** constituent stops.
///
/// Created by combining two [`Stop`] strategies with the `|` operator,
/// the [`Stop::or`] named method, or [`StopAny::new`].
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// // Stop after 5 attempts OR after 30 seconds, whichever comes first.
/// let s = stop::attempts(5) | stop::elapsed(Duration::from_secs(30));
///
/// // Equivalent using the named method:
/// let s = stop::attempts(5).or(stop::elapsed(Duration::from_secs(30)));
/// ```
#[derive(Debug, Clone)]
pub struct StopAny<A, B> {
    left: A,
    right: B,
}

impl<A, B> StopAny<A, B> {
    /// Creates a composite that stops when either `left` or `right` stops.
    ///
    /// Prefer the `|` operator or [`Stop::or`] method instead of calling
    /// this constructor directly.
    #[must_use]
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAny<A, B> {
    /// Returns `true` if **either** constituent says to stop.
    ///
    /// Both constituents are always evaluated (no short-circuit) so that
    /// stateful strategies on either side receive every call.
    fn should_stop(&self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left || right
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
/// Created by combining two [`Stop`] strategies with the `&` operator,
/// the [`Stop::and`] named method, or [`StopAll::new`].
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// // Stop only when BOTH conditions are true.
/// let s = stop::attempts(5) & stop::elapsed(Duration::from_secs(30));
///
/// // Equivalent using the named method:
/// let s = stop::attempts(5).and(stop::elapsed(Duration::from_secs(30)));
/// ```
#[derive(Debug, Clone)]
pub struct StopAll<A, B> {
    left: A,
    right: B,
}

impl<A, B> StopAll<A, B> {
    /// Creates a composite that stops only when both `left` and `right` stop.
    ///
    /// Prefer the `&` operator or [`Stop::and`] method instead of calling
    /// this constructor directly.
    #[must_use]
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAll<A, B> {
    /// Returns `true` only when **both** constituents say to stop.
    ///
    /// Both constituents are always evaluated (no short-circuit) so that
    /// stateful strategies on either side receive every call.
    fn should_stop(&self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left && right
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

/// Generates `BitOr` and `BitAnd` impls for a concrete (non-generic) [`Stop`] type,
/// producing [`StopAny`] and [`StopAll`] composites respectively.
macro_rules! impl_stop_ops {
    ($($ty:ty),+ $(,)?) => {
        $(
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
        )+
    };
}

impl_stop_ops!(StopAfterAttempts, StopAfterElapsed, StopNever);
