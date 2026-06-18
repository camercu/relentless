use super::Stop;
use super::strategies::{StopAfterAttempts, StopAfterElapsed, StopNever};
use crate::state::RetryState;
use core::ops::{BitAnd, BitOr};

/// Composite strategy that stops when **either** constituent stops.
///
/// Created by combining two [`Stop`] strategies with the `|` operator
/// or the [`Stop::or`] named method.
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use relentless::Stop;
/// use relentless::stop;
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
    /// Prefer the `|` operator or [`Stop::or`] method over this constructor.
    #[must_use]
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAny<A, B> {
    fn should_stop(&self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left || right
    }
}

/// Composite strategy that stops only when **both** constituents stop.
///
/// Created by combining two [`Stop`] strategies with the `&` operator
/// or the [`Stop::and`] named method.
///
/// Both constituents are always evaluated (no short-circuit) so that
/// stateful strategies on either side receive every `should_stop` call.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use relentless::Stop;
/// use relentless::stop;
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
    /// Prefer the `&` operator or [`Stop::and`] method over this constructor.
    #[must_use]
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<A: Stop, B: Stop> Stop for StopAll<A, B> {
    fn should_stop(&self, state: &RetryState) -> bool {
        let left = self.left.should_stop(state);
        let right = self.right.should_stop(state);
        left && right
    }
}

/// Generates `BitOr` / `BitAnd` operator impls for a [`Stop`] type, producing
/// [`StopAny`] / [`StopAll`] composites respectively. Trailing `$param`s name
/// the type's own generic parameters so composites and leaves share one macro.
macro_rules! impl_stop_ops {
    ($ty:ty $(, $param:ident)*) => {
        impl<$($param,)* Rhs> BitOr<Rhs> for $ty {
            type Output = StopAny<Self, Rhs>;

            fn bitor(self, rhs: Rhs) -> Self::Output {
                StopAny::new(self, rhs)
            }
        }

        impl<$($param,)* Rhs> BitAnd<Rhs> for $ty {
            type Output = StopAll<Self, Rhs>;

            fn bitand(self, rhs: Rhs) -> Self::Output {
                StopAll::new(self, rhs)
            }
        }
    };
}

impl_stop_ops!(StopAfterAttempts);
impl_stop_ops!(StopAfterElapsed);
impl_stop_ops!(StopNever);
impl_stop_ops!(StopAny<A, B>, A, B);
impl_stop_ops!(StopAll<A, B>, A, B);
