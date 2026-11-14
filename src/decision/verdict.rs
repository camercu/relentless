//! The verdict types a classifier returns: two-way [`Decision`], three-way
//! [`Verdict`], and [`IntoDecision`], which unifies them.

use super::sealed;
use core::convert::Infallible;

/// A two-way decision about a completed outcome: accept it or try again.
///
/// The common-case classifier currency — polling, searching, inverted probes —
/// where no outcome is ever fatal. It has no abort type parameter, so a closure
/// returning `Decision` can never leave an abort type unconstrained. Reach for
/// [`Verdict`] when you need an `Abort` arm; the `Return`/`Retry` variants are
/// spelled the same, so upgrading is a one-word change plus the new arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<R, O> {
    /// Accept this outcome as success. `call()` yields `Ok(value)`.
    Return(R),
    /// Try again; the whole outcome is handed back to the engine.
    Retry(O),
}

/// A three-way decision about a completed outcome: accept it, retry it, or abort.
///
/// `O` is the whole outcome type the operation produces; `R` is what the caller
/// receives on success (`Ok(R)`); `A` is the payload projected on a fatal abort.
///
/// A classifier that never aborts leaves `A` with nothing to infer it from,
/// so the call needs a turbofish such as `Verdict::<_, Infallible, _>`. Return
/// a [`Decision`] instead: it has no abort type to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<R, A, O> {
    /// Accept this outcome as success. `call()` yields `Ok(value)`.
    Return(R),
    /// Try again; the whole outcome is handed back to the engine.
    Retry(O),
    /// Reject as fatal. `call()` yields `Err(RetryError::Aborted { last })`.
    Abort(A),
}

/// Unifies [`Decision`] and [`Verdict`] under one `.decide` method.
///
/// A `.decide` closure returns either enum; this trait converts it into the
/// engine's canonical [`Verdict`]. It is sealed — users return one of the two
/// enums, never implement this themselves:
///
/// ```compile_fail
/// use relentless::{IntoDecision, Verdict};
///
/// struct Always;
///
/// impl IntoDecision<u32> for Always {
///     type R = u32;
///     type A = u32;
///     fn into_verdict(self) -> Verdict<u32, u32, u32> {
///         Verdict::Return(0)
///     }
/// }
/// ```
pub trait IntoDecision<O>: sealed::Sealed {
    /// The value produced on `Return`.
    type R;
    /// The value produced on `Abort`.
    type A;
    /// Converts into the canonical three-way form.
    fn into_verdict(self) -> Verdict<Self::R, Self::A, O>;
}

impl<R, O> sealed::Sealed for Decision<R, O> {}
impl<R, A, O> sealed::Sealed for Verdict<R, A, O> {}

impl<R, O> IntoDecision<O> for Decision<R, O> {
    type R = R;
    // A no-abort decision has no abort payload; `Infallible` makes the abort
    // arm uninhabited.
    type A = Infallible;

    fn into_verdict(self) -> Verdict<R, Infallible, O> {
        match self {
            Decision::Return(r) => Verdict::Return(r),
            Decision::Retry(o) => Verdict::Retry(o),
        }
    }
}

impl<R, A, O> IntoDecision<O> for Verdict<R, A, O> {
    type R = R;
    type A = A;

    fn into_verdict(self) -> Verdict<R, A, O> {
        self
    }
}
