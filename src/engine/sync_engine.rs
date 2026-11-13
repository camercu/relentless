//! The synchronous classifier engine.
//!
//! [`Retry`] pairs an operation with a classifier, stop and wait strategies and
//! a clock; `.call()` runs the shared decision core in a blocking
//! loop. The
//! async twin lives in `async_engine`.

use super::elapsed::Elapsed;
use super::op::{Blocking, Operation, ready_outcome};
use super::step::{Progress, Step, step};
use super::{RetryError, RetryStats};
use crate::clock::{SyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{ClosureClassifier, Decide, DefaultClassifier, IntoDecision, Until, When};
use crate::policy::RetryPolicy;
use crate::state::RetryState;
use crate::stop::{Stop, StopAfterAttempts};
use crate::wait::{Wait, WaitExponential};

/// Default builder returned by [`retry`].
type DefaultRetry<F> = Retry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock>;

/// A classifier-driven sync retry builder.
///
/// Configure the classifier (`.decide`/`.when`/`.until`), stop and wait
/// strategies, and clock, then call [`call`](Self::call). The classifier slot
/// `C` defaults to [`DefaultClassifier`].
pub struct Retry<F, C, S, W, Cl> {
    op: F,
    policy: RetryPolicy<S, W, C>,
    clock: Cl,
}

impl<F, C, S, W, Cl> core::fmt::Debug for Retry<F, C, S, W, Cl> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retry").finish_non_exhaustive()
    }
}

impl<F, C, S, W, Cl> core::fmt::Debug for RetryWithStats<F, C, S, W, Cl> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RetryWithStats").finish_non_exhaustive()
    }
}

/// Begins a classifier-driven retry from an operation.
///
/// The operation receives the [`RetryState`] (attempt number, elapsed time,
/// previous delay); ignore it with `|_|`. Defaults: `stop::attempts(3)`,
/// `wait::exponential(100ms)`, the default classifier, and
/// [`SystemClock`]. In non-`std` builds `.clock(...)` must be set before
/// `.call()`.
///
/// For an operation that returns a future, use
/// [`retry_async`](crate::retry_async). Passed here, the future itself becomes
/// the outcome, which the default classifier cannot classify.
pub fn retry<F, O>(op: F) -> DefaultRetry<F>
where
    F: FnMut(RetryState) -> O,
{
    Retry::from_op(op)
}

impl<F> DefaultRetry<F> {
    /// Builds a default-configured retry around an operation.
    fn from_op(op: F) -> Self {
        Retry::from_policy(op, RetryPolicy::new())
    }
}

impl<F, C, S, W> Retry<F, C, S, W, SystemClock> {
    /// Assembles a retry from an operation and a policy, with the default clock.
    /// Used by [`RetryPolicy::retry`](crate::RetryPolicy::retry)
    /// to borrow a reusable policy.
    pub(crate) fn from_policy(op: F, policy: RetryPolicy<S, W, C>) -> Self {
        Retry {
            op,
            policy,
            clock: SystemClock,
        }
    }
}

impl<F, C, S, W, Cl> Retry<F, C, S, W, Cl> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> Retry<F, C, NewStop, W, Cl> {
        Retry {
            op: self.op,
            policy: self.policy.stop(stop),
            clock: self.clock,
        }
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> Retry<F, C, S, NewWait, Cl> {
        Retry {
            op: self.op,
            policy: self.policy.wait(wait),
            clock: self.clock,
        }
    }

    /// Sets the clock that supplies elapsed time and performs waits.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> Retry<F, C, S, W, NewClock> {
        Retry {
            op: self.op,
            policy: self.policy,
            clock,
        }
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// The closure returns either [`Decision`](crate::decision::Decision) (no
    /// abort) or [`Verdict`](crate::decision::Verdict) (abort-capable);
    /// [`IntoDecision`] unifies both. When nothing is fatal, return `Decision`:
    /// a `Verdict` with no `Abort` arm leaves its abort type to annotate. The
    /// op-anchored `F: Operation<Blocking, Output = O>` bound here (not deferred to
    /// `.call()`) lets the closure's parameter infer from the operation's
    /// output, so inline classifiers need no annotations.
    #[must_use]
    pub fn decide<O, D, NewC>(self, classifier: NewC) -> Retry<F, ClosureClassifier<NewC>, S, W, Cl>
    where
        F: Operation<Blocking, Output = O>,
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
    pub fn when<T, E, P>(self, retryable: P) -> Retry<F, When<P>, S, W, Cl>
    where
        F: Operation<Blocking, Output = Result<T, E>>,
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
    pub fn until<T, E, P>(self, done: P) -> Retry<F, Until<P>, S, W, Cl>
    where
        F: Operation<Blocking, Output = Result<T, E>>,
        P: Fn(&T) -> bool,
    {
        self.with_classifier(Until(done))
    }

    /// Sets a wall-clock budget for the whole execution.
    ///
    /// A boundary check, not a preemptive timeout: it is evaluated between
    /// attempts. The next inter-attempt wait is clamped to the remaining budget,
    /// and the loop stops (as `Exhausted`) once elapsed time meets or exceeds
    /// `dur`. It cannot interrupt an operation or wait already in progress.
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

    /// Wraps this execution so [`call`](RetryWithStats::call) also returns
    /// [`RetryStats`].
    #[must_use]
    pub fn with_stats(self) -> RetryWithStats<F, C, S, W, Cl> {
        RetryWithStats { inner: self }
    }

    fn with_classifier<NewC>(self, classifier: NewC) -> Retry<F, NewC, S, W, Cl> {
        Retry {
            op: self.op,
            policy: self.policy.with_classifier(classifier),
            clock: self.clock,
        }
    }
}

impl<F, C, S, W, Cl, O> Retry<F, C, S, W, Cl>
where
    F: Operation<Blocking, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
{
    /// Drives the retry loop to completion.
    ///
    /// Returns `Ok(value)` on the classifier's first
    /// [`Verdict::Return`](crate::decision::Verdict::Return),
    /// `Err(RetryError::Aborted { .. })` on a
    /// [`Verdict::Abort`](crate::decision::Verdict::Abort), and
    /// `Err(RetryError::Exhausted { .. })` when the stop strategy fires (or the
    /// timeout is exceeded) while the classifier still wants to retry.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    ///
    /// # Panics
    ///
    /// Panics, in debug builds only, if the injected [`Clock`](crate::clock::Clock) reports an
    /// elapsed reading below one it already reported — a violation of
    /// [`Clock::now`](crate::clock::Clock::now)'s monotonicity precondition. Release builds clamp
    /// instead, so a broken clock cannot un-spend the timeout budget either
    /// way; the assertion exists to surface the broken clock in its
    /// implementor's own tests. No clock shipped with this crate can trigger
    /// it.
    pub fn call(self) -> Result<C::R, RetryError<C::A, O>> {
        self.run().0
    }

    /// Shared driver for [`call`](Self::call) and the stats wrapper: runs the
    /// loop and always produces [`RetryStats`] (they are `Copy`, so collecting
    /// them unconditionally is free and keeps the loop allocation-free).
    #[allow(clippy::type_complexity)]
    fn run(mut self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        let mut elapsed = Elapsed::start(&self.clock);

        let mut attempt: u32 = 1;
        let mut previous_delay: Option<Duration> = None;
        let mut total_wait = Duration::ZERO;

        loop {
            // One clock read for the before-attempt state and the operation.
            let before_state = RetryState::for_attempt(attempt)
                .with_elapsed(elapsed.read(&self.clock))
                .with_previous_delay(previous_delay);
            let outcome = ready_outcome(self.op.call_op(before_state));

            // Elapsed is re-read after the operation so stop and wait see the time the attempt actually took.
            let post_elapsed = elapsed.read(&self.clock);

            match step(
                Progress {
                    attempt,
                    elapsed: post_elapsed,
                    previous_delay,
                    total_wait,
                },
                outcome,
                &self.policy,
            ) {
                Step::Done { result, stats } => return (result, stats),
                Step::Continue {
                    delay,
                    total_wait: running,
                } => {
                    // The blocking sleep is the sync engine's half of the loop;
                    // the async engine polls a sleep future instead.
                    if !delay.is_zero() {
                        self.clock.wait(delay);
                    }
                    total_wait = running;
                    previous_delay = Some(delay);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

/// A [`Retry`] wrapper whose [`call`](RetryWithStats::call) also returns
/// [`RetryStats`]. Created by [`Retry::with_stats`].
pub struct RetryWithStats<F, C, S, W, Cl> {
    inner: Retry<F, C, S, W, Cl>,
}

impl<F, C, S, W, Cl, O> RetryWithStats<F, C, S, W, Cl>
where
    F: Operation<Blocking, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
{
    /// Drives the retry loop and returns both the result and the stats.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    ///
    /// # Panics
    ///
    /// Panics, in debug builds only, if the injected [`Clock`](crate::clock::Clock) reports an
    /// elapsed reading below one it already reported — see
    /// [`Retry::call`](Retry::call).
    #[allow(clippy::type_complexity)]
    pub fn call(self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        self.inner.run()
    }
}
