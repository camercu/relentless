//! Self-classifying outcomes: [`Outcome`] and its impls for the standard
//! shapes `Result`, `Option` and `ControlFlow`.

use super::Verdict;
use core::convert::Infallible;
use core::ops::ControlFlow;

/// Projects an owned outcome type into a [`Verdict`].
///
/// Implement this for a type you own to make it classify itself, so the default
/// engine path needs no `.decide(...)` closure at the call site. Blanket impls
/// cover the three standard outcome shapes: `Result<T, E>` (`Ok(v)` returns `v`,
/// any `Err` retries), `Option<T>` (`Some(v)` returns `v`, `None` retries), and
/// `ControlFlow<B, C>` (`Break(b)` returns `b`, `Continue(_)` retries).
///
/// # Orphan-rule note
///
/// The blanket impls for `Result<T, E>`, `Option<T>`, and `ControlFlow<B, C>`
/// already cover every one of those types, so you cannot write your own
/// `impl Outcome` for them. For custom classification, use `.decide(closure)`
/// or wrap the value in a newtype you own.
pub trait Outcome: Sized {
    /// The value delivered to the caller on `Return` (what `Ok` carries).
    type Return;
    /// The value delivered on `Abort` (what `RetryError::Aborted { last }` carries).
    type Abort;

    /// Classifies this outcome into a three-way [`Verdict`].
    fn classify(self) -> Verdict<Self::Return, Self::Abort, Self>;
}

/// Default: `Ok(v)` → `Return(v)`, any `Err` → `Retry`.
///
/// Every error is retried, so the loop terminates on an error only by
/// exhausting its stop strategy (`RetryError::Exhausted`), never by aborting on
/// the default path. `Abort` is typed as `E` (not `Infallible`) so the default and `.when`/`.until` paths
/// share one `RetryError<E, Result<T, E>>` shape.
impl<T, E> Outcome for Result<T, E> {
    type Return = T;
    type Abort = E;

    fn classify(self) -> Verdict<T, E, Result<T, E>> {
        match self {
            Ok(value) => Verdict::Return(value),
            Err(_) => Verdict::Retry(self),
        }
    }
}

/// The canonical two-state poll: `Some(v)` → `Return(v)`, `None` → `Retry`.
///
/// An `Option`-returning operation drives the default engine with no `.decide`
/// at the call site — `None` means "not ready, keep going", `Some` delivers the
/// value. There is no fatal outcome, so `Abort` is [`Infallible`]: the loop
/// terminates only by returning a `Some` or exhausting its stop strategy
/// (`RetryError::Exhausted { last: None }`).
///
/// # Type inference
///
/// Pin `T`: an op that only ever produces `None` (e.g. `retry(|_| None)`) leaves
/// `T` unconstrained (`E0282`). Give the op a concrete signature — as a named
/// function does — or annotate the `None`.
impl<T> Outcome for Option<T> {
    type Return = T;
    type Abort = Infallible;

    fn classify(self) -> Verdict<T, Infallible, Option<T>> {
        match self {
            Some(value) => Verdict::Return(value),
            None => Verdict::Retry(None),
        }
    }
}

/// Loop control as an outcome: `Break(b)` → `Return(b)`, `Continue(_)` → `Retry`.
///
/// [`ControlFlow`] is the standard "keep going vs stop with a value" signal, so
/// an operation returning it drives the default engine with no `.decide`:
/// `Continue` means "not done, retry" and `Break` carries the value out. As with
/// `Option` there is no fatal outcome, so `Abort` is [`Infallible`] and the loop
/// terminates only by breaking or exhausting its stop strategy
/// (`RetryError::Exhausted { last: ControlFlow::Continue(..) }`).
///
/// # Type inference
///
/// `ControlFlow` has two type parameters, so pin both. An op that only ever
/// produces one variant leaves the other unconstrained (`E0282`):
/// `retry(|_| ControlFlow::Continue(()))` cannot infer `B`, and
/// `retry(|_| ControlFlow::Break(1))` cannot infer `C`. Give the op a concrete
/// signature.
impl<B, C> Outcome for ControlFlow<B, C> {
    type Return = B;
    type Abort = Infallible;

    fn classify(self) -> Verdict<B, Infallible, ControlFlow<B, C>> {
        match self {
            ControlFlow::Break(value) => Verdict::Return(value),
            ControlFlow::Continue(_) => Verdict::Retry(self),
        }
    }
}
