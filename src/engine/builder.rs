//! The retry builder shared by both modes.
//!
//! [`Retry`] pairs an operation with a classifier, stop and wait strategies and
//! a clock. Its mode `M` ([`Blocking`](super::Blocking) or
//! [`Async`](super::Async)) is fixed by the entry point and selects which
//! `.call()` exists: the blocking loop lives in `sync_engine`, the future in
//! `async_engine`. Every setter is written once, here, for both modes.

use super::op::Operation;
use crate::clock::SystemClock;
use crate::decision::{ClosureClassifier, IntoDecision, Until, When};
use crate::policy::RetryPolicy;
use core::marker::PhantomData;
use core::time::Duration;

/// A classifier-driven retry builder.
///
/// Configure the classifier (`.decide`/`.when`/`.until`), stop and wait
/// strategies, clock and timeout, then run it with `.call()` or
/// `.call_with_stats()`.
///
/// `M` is the mode: [`Blocking`](crate::Blocking) for [`retry`](crate::retry)
/// and [`RetryPolicy::retry`], [`Async`](crate::Async) for
/// [`retry_async`](crate::retry_async) and [`RetryPolicy::retry_async`]. A
/// blocking `.call()` needs a [`SyncClock`](crate::SyncClock) and runs to
/// completion; an async `.call()` needs an [`AsyncClock`](crate::AsyncClock)
/// and returns a future. The classifier slot `C` defaults to
/// [`DefaultClassifier`](crate::decision::DefaultClassifier).
pub struct Retry<M, F, C, S, W, Cl> {
    pub(super) op: F,
    pub(super) policy: RetryPolicy<S, W, C>,
    pub(super) clock: Cl,
    mode: PhantomData<M>,
}

impl<M, F, C, S, W, Cl> core::fmt::Debug for Retry<M, F, C, S, W, Cl> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retry").finish_non_exhaustive()
    }
}

impl<M, F, C, S, W> Retry<M, F, C, S, W, SystemClock> {
    /// Assembles a retry from an operation and a policy, with the default
    /// clock. Used by the entry points and by
    /// [`RetryPolicy::retry`]/[`RetryPolicy::retry_async`] to borrow a
    /// reusable policy.
    pub(crate) fn from_policy(op: F, policy: RetryPolicy<S, W, C>) -> Self {
        Self::from_parts(op, policy, SystemClock)
    }
}

impl<M, F, C, S, W, Cl> Retry<M, F, C, S, W, Cl> {
    fn from_parts(op: F, policy: RetryPolicy<S, W, C>, clock: Cl) -> Self {
        Retry {
            op,
            policy,
            clock,
            mode: PhantomData,
        }
    }

    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> Retry<M, F, C, NewStop, W, Cl> {
        Retry::from_parts(self.op, self.policy.stop(stop), self.clock)
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> Retry<M, F, C, S, NewWait, Cl> {
        Retry::from_parts(self.op, self.policy.wait(wait), self.clock)
    }

    /// Sets the clock that supplies elapsed time and performs waits.
    ///
    /// A blocking retry needs a [`SyncClock`](crate::SyncClock); an async one
    /// needs an [`AsyncClock`](crate::AsyncClock). The last call wins.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> Retry<M, F, C, S, W, NewClock> {
        Retry::from_parts(self.op, self.policy, clock)
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// The closure returns either [`Decision`](crate::decision::Decision) (no
    /// abort) or [`Verdict`](crate::decision::Verdict) (abort-capable);
    /// [`IntoDecision`] unifies both. When nothing is fatal, return `Decision`:
    /// a `Verdict` with no `Abort` arm leaves its abort type to annotate. The
    /// op-anchored `F: Operation<M, Output = O>` bound here (not deferred to
    /// `.call()`) lets the closure's parameter infer from the operation's
    /// output, so inline classifiers need no annotations.
    #[must_use]
    pub fn decide<O, D, NewC>(
        self,
        classifier: NewC,
    ) -> Retry<M, F, ClosureClassifier<NewC>, S, W, Cl>
    where
        F: Operation<M, Output = O>,
        NewC: Fn(O) -> D,
        D: IntoDecision<O>,
    {
        self.with_classifier(ClosureClassifier(classifier))
    }

    /// Retries an error while `retryable` returns `true` for it.
    ///
    /// An `Ok` returns. An `Err(e)` retries when `retryable(&e)` holds and
    /// otherwise aborts with the bare error. The closure's `&E` parameter
    /// infers from the operation's output, so it needs no annotation. For
    /// rules over the whole outcome, use [`decide`](Self::decide).
    #[must_use]
    pub fn when<T, E, P>(self, retryable: P) -> Retry<M, F, When<P>, S, W, Cl>
    where
        F: Operation<M, Output = Result<T, E>>,
        P: Fn(&E) -> bool,
    {
        self.with_classifier(When(retryable))
    }

    /// Retries until an `Ok` value satisfies `done`.
    ///
    /// An `Ok(v)` returns when `done(&v)` holds and retries otherwise; every
    /// `Err` retries too, so the loop never aborts. Natural for polling. To
    /// stop on a fatal error instead, use [`decide`](Self::decide).
    #[must_use]
    pub fn until<T, E, P>(self, done: P) -> Retry<M, F, Until<P>, S, W, Cl>
    where
        F: Operation<M, Output = Result<T, E>>,
        P: Fn(&T) -> bool,
    {
        self.with_classifier(Until(done))
    }

    /// Sets a wall-clock budget for the whole execution.
    ///
    /// A boundary check, not a preemptive timeout: it is evaluated between
    /// attempts. The next inter-attempt wait is clamped to the remaining budget,
    /// and the loop stops (as `Exhausted`) once elapsed time meets or exceeds
    /// `dur`. It cannot interrupt an operation or wait already in progress. A
    /// wait clamped to the last of the budget still ends with one final
    /// attempt at the deadline, so total wall-clock time can exceed `dur` by
    /// roughly that attempt's duration.
    ///
    /// `Duration::ZERO` therefore permits exactly one attempt: the boundary is
    /// met (`0 >= 0`) the instant that attempt completes. There is no "disabled"
    /// or "infinite" value — omit `.timeout` for an unbounded deadline.
    ///
    /// The budget is spent in whatever units the injected [`Clock`](crate::clock::Clock) reports, so
    /// it is only as good as that clock: a non-advancing clock pins elapsed at
    /// zero and this deadline never fires. Pair it with
    /// [`stop::attempts`](crate::stop::attempts) to stay bounded regardless —
    /// the same caveat [`stop::elapsed`](crate::stop::elapsed) carries. A clock
    /// that moves *backwards* is handled: the engine retains the highest
    /// elapsed reading, so a budget once spent cannot be refunded.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.policy = self.policy.timeout(dur);
        self
    }

    fn with_classifier<NewC>(self, classifier: NewC) -> Retry<M, F, NewC, S, W, Cl> {
        Retry::from_parts(self.op, self.policy.with_classifier(classifier), self.clock)
    }
}
