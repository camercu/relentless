//! The classifier-driven retry engine (ADR-6).
//!
//! This is the parallel, not-yet-public engine that will replace the
//! predicate-driven [`crate::policy`] engine. It reuses the outcome-agnostic
//! infrastructure unchanged — [`Stop`], [`Wait`], [`SyncClock`], and
//! [`RetryState`] — and swaps the boolean predicate for a [`Decide`] classifier
//! that consumes each outcome by value.
//!
//! Both the sync ([`Retry`]) and async ([`AsyncRetry`]) paths carry the full
//! classifier surface — `.decide`/`.when`/`.until`, hooks, stats, and timeout.

mod async_engine;
mod error;
mod hooks;
mod op;
mod state;
mod stats;

pub use error::{RetryError, RetryResult};

use op::{RetryOp, StatelessOp};

/// Default builder returned by [`retry`], [`RetryExt::retry`], and friends.
type DefaultRetry<F> =
    Retry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock, (), (), ()>;

pub use async_engine::{
    AsyncRetry, AsyncRetryExt, AsyncRetryWithStats, AsyncRun, DropStats, retry_async,
};
pub use state::{AttemptState, Exit, StopReason};
pub use stats::RetryStats;

use crate::clock::{SyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{
    ClosureClassifier, Decide, DefaultClassifier, IntoDecision, Until, Verdict, When,
};
use crate::predicate::Predicate;
use crate::state::RetryState;
use crate::stop::{self, Stop, StopAfterAttempts};
use crate::wait::{self, Wait, WaitExponential};
use hooks::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, HookChain};

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// A classifier-driven sync retry builder.
///
/// Configure the classifier (`.decide`/`.when`/`.until`), stop and wait
/// strategies, clock, and hooks, then call [`call`](Self::call). The classifier
/// slot `C` defaults to [`DefaultClassifier`]; the hook slots `BA`/`AA`/`OX`
/// default to `()` (no-op).
pub struct Retry<F, C, S, W, Cl, BA, AA, OX> {
    op: F,
    classifier: C,
    stop: S,
    wait: W,
    clock: Cl,
    hooks: ExecutionHooks<BA, AA, OX>,
    timeout: Option<Duration>,
}

impl<F, C, S, W, Cl, BA, AA, OX> core::fmt::Debug for Retry<F, C, S, W, Cl, BA, AA, OX> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retry").finish_non_exhaustive()
    }
}

impl<F, C, S, W, Cl, BA, AA, OX> core::fmt::Debug for RetryWithStats<F, C, S, W, Cl, BA, AA, OX> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RetryWithStats").finish_non_exhaustive()
    }
}

/// Begins a classifier-driven retry from an operation.
///
/// Defaults: `stop::attempts(3)`, `wait::exponential(100ms)`, the default
/// classifier, no hooks, and [`SystemClock`]. In non-`std` builds `.clock(...)`
/// must be set before `.call()`.
pub fn retry<F, O>(op: F) -> DefaultRetry<F>
where
    F: FnMut(RetryState) -> O,
{
    Retry::from_op(op)
}

impl<F> DefaultRetry<F> {
    /// Builds a default-configured retry around an operation.
    fn from_op(op: F) -> Self {
        Retry::from_parts(
            op,
            DefaultClassifier,
            stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait::exponential(DEFAULT_INITIAL_WAIT),
        )
    }
}

impl<F, C, S, W> Retry<F, C, S, W, SystemClock, (), (), ()> {
    /// Assembles a retry from an operation and pre-chosen classifier/stop/wait,
    /// with the default clock, no hooks, and no timeout. Used by
    /// [`RetryPolicy::retry`](crate::RetryPolicy::retry) to borrow a reusable
    /// policy's parts.
    pub(crate) fn from_parts(op: F, classifier: C, stop: S, wait: W) -> Self {
        Retry {
            op,
            classifier,
            stop,
            wait,
            clock: SystemClock,
            hooks: ExecutionHooks::new(),
            timeout: None,
        }
    }
}

/// Starts a sync retry directly from a no-argument closure or function.
///
/// The operation takes no parameters; use [`retry`] when you need the
/// [`RetryState`]. Defaults match [`retry`]: `stop::attempts(3)`,
/// `wait::exponential(100ms)`, retry on any `Err` (via the default classifier).
pub trait RetryExt<O>: FnMut() -> O + Sized {
    /// Begins an owned retry builder from this closure.
    fn retry(self) -> DefaultRetry<StatelessOp<Self>>;
}

impl<O, F: FnMut() -> O> RetryExt<O> for F {
    fn retry(self) -> DefaultRetry<StatelessOp<Self>> {
        Retry::from_op(StatelessOp(self))
    }
}

impl<F, C, S, W, Cl, BA, AA, OX> Retry<F, C, S, W, Cl, BA, AA, OX> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> Retry<F, C, NewStop, W, Cl, BA, AA, OX> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks,
            timeout: self.timeout,
        }
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> Retry<F, C, S, NewWait, Cl, BA, AA, OX> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait,
            clock: self.clock,
            hooks: self.hooks,
            timeout: self.timeout,
        }
    }

    /// Sets the clock that supplies elapsed time and performs waits.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> Retry<F, C, S, W, NewClock, BA, AA, OX> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock,
            hooks: self.hooks,
            timeout: self.timeout,
        }
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// The closure returns either [`Decision`](crate::decision::Decision) (no
    /// abort) or [`Verdict`] (abort-capable); [`IntoDecision`] unifies both. The
    /// op-anchored `F: RetryOp<Output = O>` bound here (not deferred to
    /// `.call()`) lets the closure's parameter infer from the operation's
    /// output, so inline classifiers need no annotations.
    #[must_use]
    pub fn decide<O, D, NewC>(
        self,
        classifier: NewC,
    ) -> Retry<F, ClosureClassifier<NewC>, S, W, Cl, BA, AA, OX>
    where
        F: RetryOp<Output = O>,
        NewC: Fn(O) -> D,
        D: IntoDecision<O>,
    {
        self.with_classifier(ClosureClassifier(classifier))
    }

    /// Retries while `predicate` wants to; otherwise accepts the outcome
    /// (`Ok` returns, a rejected `Err` aborts with the bare error).
    ///
    /// `Result`-only sugar over [`decide`](Self::decide). The predicate's
    /// `&Result<T, E>` parameter infers from the operation's output.
    #[must_use]
    pub fn when<T, E, P>(self, predicate: P) -> Retry<F, When<P>, S, W, Cl, BA, AA, OX>
    where
        F: RetryOp<Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(When(predicate))
    }

    /// Retries *until* `predicate` is satisfied, then accepts. The inverse of
    /// [`when`](Self::when); natural for polling (`.until(ok(is_ready))`).
    #[must_use]
    pub fn until<T, E, P>(self, predicate: P) -> Retry<F, Until<P>, S, W, Cl, BA, AA, OX>
    where
        F: RetryOp<Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(Until(predicate))
    }

    /// Sets a wall-clock budget for the whole execution.
    ///
    /// A boundary check, not a preemptive timeout: it is evaluated between
    /// attempts. The next inter-attempt wait is clamped to the remaining budget,
    /// and the loop stops (as `Exhausted`) once elapsed time exceeds `dur`. It
    /// cannot interrupt an operation or wait already in progress.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Wraps this execution so [`call`](RetryWithStats::call) also returns
    /// [`RetryStats`].
    #[must_use]
    pub fn with_stats(self) -> RetryWithStats<F, C, S, W, Cl, BA, AA, OX> {
        RetryWithStats { inner: self }
    }

    fn with_classifier<NewC>(self, classifier: NewC) -> Retry<F, NewC, S, W, Cl, BA, AA, OX> {
        Retry {
            op: self.op,
            classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks,
            timeout: self.timeout,
        }
    }

    /// Registers a hook that runs before each attempt.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> Retry<F, C, S, W, Cl, HookChain<BA, Hook>, AA, OX>
    where
        Hook: FnMut(&RetryState),
    {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks.chain_before_attempt(hook),
            timeout: self.timeout,
        }
    }

    /// Registers a hook that runs after each attempt, before classification, so
    /// it observes every raw outcome — including the terminal one.
    #[must_use]
    pub fn after_attempt<O, Hook>(
        self,
        hook: Hook,
    ) -> Retry<F, C, S, W, Cl, BA, HookChain<AA, Hook>, OX>
    where
        F: RetryOp<Output = O>,
        Hook: for<'a> FnMut(&AttemptState<'a, O>),
    {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks.chain_after_attempt(hook),
            timeout: self.timeout,
        }
    }

    /// Registers a hook that runs once when the retry loop exits.
    #[must_use]
    pub fn on_exit<O, Hook>(self, hook: Hook) -> Retry<F, C, S, W, Cl, BA, AA, HookChain<OX, Hook>>
    where
        F: RetryOp<Output = O>,
        C: Decide<O>,
        Hook: for<'a> FnMut(&Exit<'a, C::R, C::A, O>),
    {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks.chain_on_exit(hook),
            timeout: self.timeout,
        }
    }
}

impl<F, C, S, W, Cl, BA, AA, OX, O> Retry<F, C, S, W, Cl, BA, AA, OX>
where
    F: RetryOp<Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
    BA: BeforeAttemptHook,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    /// Drives the retry loop to completion.
    ///
    /// Returns `Ok(value)` on the classifier's first [`Verdict::Return`],
    /// `Err(RetryError::Aborted { .. })` on a [`Verdict::Abort`], and
    /// `Err(RetryError::Exhausted { .. })` when the stop strategy fires (or the
    /// timeout is exceeded) while the classifier still wants to retry.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    pub fn call(self) -> Result<C::R, RetryError<C::A, O>> {
        self.run().0
    }

    /// Shared driver for [`call`](Self::call) and the stats wrapper: runs the
    /// loop and always produces [`RetryStats`] (they are `Copy`, so collecting
    /// them unconditionally is free and keeps the loop allocation-free).
    #[allow(clippy::type_complexity)]
    fn run(mut self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        let origin = self.clock.now();
        let elapsed = |clock: &Cl| clock.now().saturating_sub(origin);

        let mut attempt: u32 = 1;
        let mut previous_delay: Option<Duration> = None;
        let mut total_wait = Duration::ZERO;

        loop {
            // One clock read for the before-attempt state and the operation.
            let before_state = RetryState::for_attempt(attempt)
                .with_elapsed(elapsed(&self.clock))
                .with_previous_delay(previous_delay);
            self.hooks.before_attempt.call(&before_state);
            let outcome = self.op.call_op(before_state);

            // Elapsed is re-read after the operation so the post-attempt hooks
            // and stop/wait see the time the attempt actually took.
            let post_elapsed = elapsed(&self.clock);

            // `after_attempt` fires before the classifier consumes the outcome.
            {
                let attempt_state = AttemptState::new(attempt, post_elapsed, &outcome);
                self.hooks.after_attempt.call(&attempt_state);
            }

            let state = RetryState::for_attempt(attempt)
                .with_elapsed(post_elapsed)
                .with_previous_delay(previous_delay);

            let stats_for = |reason| RetryStats {
                attempts: attempt,
                total_elapsed: post_elapsed,
                total_wait,
                stop_reason: reason,
            };

            match self.classifier.decide(outcome) {
                Verdict::Return(value) => {
                    self.hooks.on_exit.call(&Exit::Returned {
                        attempt,
                        elapsed: post_elapsed,
                        value: &value,
                    });
                    return (Ok(value), stats_for(StopReason::Returned));
                }
                Verdict::Abort(last) => {
                    self.hooks.on_exit.call(&Exit::Aborted {
                        attempt,
                        elapsed: post_elapsed,
                        last: &last,
                    });
                    return (
                        Err(RetryError::Aborted { last }),
                        stats_for(StopReason::Aborted),
                    );
                }
                Verdict::Retry(last) => {
                    let timeout_exceeded = self.timeout.is_some_and(|t| post_elapsed >= t);
                    if self.stop.should_stop(&state) || timeout_exceeded {
                        self.hooks.on_exit.call(&Exit::Exhausted {
                            attempt,
                            elapsed: post_elapsed,
                            last: &last,
                        });
                        return (
                            Err(RetryError::Exhausted { last }),
                            stats_for(StopReason::Exhausted),
                        );
                    }

                    // The wait strategy is consulted only now that a retry is
                    // certain: no next attempt means no wait to compute. Clamp
                    // the sleep to the remaining timeout budget.
                    let next_delay = self.wait.next_wait(&state);
                    let delay = match self.timeout {
                        Some(t) => next_delay.min(t.saturating_sub(post_elapsed)),
                        None => next_delay,
                    };
                    if !delay.is_zero() {
                        self.clock.wait(delay);
                    }
                    total_wait = total_wait.saturating_add(delay);
                    previous_delay = Some(delay);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

/// A [`Retry`] wrapper whose [`call`](RetryWithStats::call) also returns
/// [`RetryStats`]. Created by [`Retry::with_stats`].
pub struct RetryWithStats<F, C, S, W, Cl, BA, AA, OX> {
    inner: Retry<F, C, S, W, Cl, BA, AA, OX>,
}

impl<F, C, S, W, Cl, BA, AA, OX, O> RetryWithStats<F, C, S, W, Cl, BA, AA, OX>
where
    F: RetryOp<Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
    BA: BeforeAttemptHook,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    /// Drives the retry loop and returns both the result and the stats.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    #[allow(clippy::type_complexity)]
    pub fn call(self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        self.inner.run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::VirtualClock;
    use crate::decision::Outcome;
    use crate::predicate;
    use core::cell::Cell;

    const ARBITRARY_ATTEMPTS: u32 = 5;

    /// A non-`Result` poll outcome that classifies itself, so the default path
    /// drives it with no `.decide` at the call site.
    #[derive(Debug, PartialEq)]
    enum Poll {
        Pending,
        Ready(i32),
        Failed(&'static str),
    }

    impl Outcome for Poll {
        type Return = i32;
        type Abort = &'static str;

        fn classify(self) -> Verdict<i32, &'static str, Poll> {
            match self {
                Poll::Ready(value) => Verdict::Return(value),
                Poll::Failed(error) => Verdict::Abort(error),
                Poll::Pending => Verdict::Retry(self),
            }
        }
    }

    #[test]
    fn retries_on_err_then_returns_the_ok_value() {
        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 2 {
                Ok::<i32, &str>(7)
            } else {
                Err("not yet")
            }
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(7));
        assert_eq!(counter.get(), 2, "should stop retrying once Ok is returned");
    }

    #[test]
    fn exhausts_with_the_final_outcome_when_stop_fires() {
        let counter = Cell::new(0);
        let result = retry(|_| {
            counter.set(counter.get() + 1);
            Err::<i32, &str>("boom")
        })
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
        assert_eq!(counter.get(), 3, "should attempt exactly the stop budget");
    }

    #[test]
    fn decide_can_abort_on_a_projected_payload() {
        use crate::decision::Verdict;

        // Fuzzing shape: keep retrying transient errors, but abort fatally on a
        // specific one — the abort arm pins the payload type inline.
        let result = retry(|_| Err::<i32, &str>("fatal"))
            .decide(|o| match o {
                Ok(v) => Verdict::Return(v),
                Err("fatal") => Verdict::Abort("boom"),
                Err(_) => Verdict::Retry(o),
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
    }

    #[test]
    fn decide_returns_the_sought_error_through_ok() {
        use crate::decision::Decision;

        // The inverted-polling wart, gone: probe until a failure appears and
        // deliver that failure as the success value via `Ok` — no `RetryError`.
        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 3 { Err("crash") } else { Ok(()) }
        })
        .decide(|o| match o {
            Err(e) => Decision::Return(e),
            Ok(()) => Decision::Retry(o),
        })
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok("crash"));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn until_polls_a_result_until_ready() {
        #[derive(Debug, PartialEq)]
        enum Status {
            Pending,
            Done,
        }

        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            Ok::<Status, &str>(if n >= 2 {
                Status::Done
            } else {
                Status::Pending
            })
        })
        .until(predicate::ok(|s: &Status| *s == Status::Done))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(Status::Done));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn when_aborts_on_a_rejected_error() {
        // Retry only "transient"; any other error is non-retryable and aborts
        // with the bare error payload.
        let result = retry(|_| Err::<i32, &str>("fatal"))
            .when(predicate::error(|e: &&str| *e == "transient"))
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(result, Err(RetryError::Aborted { last: "fatal" }));
    }

    #[test]
    fn owned_outcome_type_returns_via_the_default_path() {
        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 3 {
                Poll::Ready(9)
            } else {
                Poll::Pending
            }
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(9));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn owned_outcome_type_aborts_via_the_default_path() {
        let result = retry(|_| Poll::Failed("io"))
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(result, Err(RetryError::Aborted { last: "io" }));
    }

    type IntResult = Result<i32, &'static str>;

    #[test]
    fn after_attempt_fires_before_classification_on_every_attempt() {
        // `after_attempt` observes the raw outcome for every attempt, including
        // the terminal `Ok` — proving it fires before the classifier consumes it.
        let before = Cell::new(0u32);
        let after = Cell::new(0u32);
        let last_after_ok = Cell::new(false);

        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 2 { Ok::<i32, &str>(7) } else { Err("x") }
        })
        .before_attempt(|_| before.set(before.get() + 1))
        .after_attempt(|s: &AttemptState<IntResult>| {
            after.set(after.get() + 1);
            last_after_ok.set(s.outcome.is_ok());
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(7));
        assert_eq!(before.get(), 2);
        assert_eq!(after.get(), 2);
        assert!(last_after_ok.get(), "after_attempt saw the terminal Ok");
    }

    #[test]
    fn on_exit_reports_returned_with_the_final_attempt_count() {
        let seen = Cell::new(None);
        let _ = retry(|_| Ok::<i32, &str>(1))
            .on_exit(|e: &Exit<i32, &str, IntResult>| {
                seen.set(Some((e.stop_reason(), e.attempt())));
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(seen.get(), Some((StopReason::Returned, 1)));
    }

    #[test]
    fn on_exit_reports_aborted_from_a_rejected_error() {
        let reason = Cell::new(None);
        let _ = retry(|_| Err::<i32, &str>("fatal"))
            .when(predicate::error(|e: &&str| *e == "transient"))
            .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
            .stop(stop::attempts(3))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(reason.get(), Some(StopReason::Aborted));
    }

    #[test]
    fn on_exit_reports_exhausted_after_the_full_stop_budget() {
        let reason = Cell::new(None);
        let after = Cell::new(0u32);
        let _ = retry(|_| Err::<i32, &str>("boom"))
            .after_attempt(|_: &AttemptState<IntResult>| after.set(after.get() + 1))
            .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
            .stop(stop::attempts(3))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(reason.get(), Some(StopReason::Exhausted));
        assert_eq!(
            after.get(),
            3,
            "after_attempt fires on the terminal attempt too"
        );
    }

    #[test]
    fn retry_ext_starts_from_a_no_arg_closure() {
        let counter = Cell::new(0);
        let result = (|| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") }
        })
        .retry()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(5));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn retry_ext_supports_the_classifier_surface() {
        // The stateless-ext builder still reaches `.until`, because the op slot
        // is unified behind `RetryOp` rather than a bare `FnMut`.
        let counter = Cell::new(0);
        let result = (|| {
            counter.set(counter.get() + 1);
            Ok::<i32, &str>(counter.get())
        })
        .retry()
        .until(predicate::ok(|v: &i32| *v >= 3))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(3));
    }

    #[test]
    fn with_stats_reports_attempts_wait_elapsed_and_reason() {
        let (result, stats) = retry(|_| Err::<i32, &str>("boom"))
            .stop(stop::attempts(3))
            .wait(wait::fixed(Duration::from_millis(5)))
            .clock(VirtualClock::new())
            .with_stats()
            .call();

        assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
        assert_eq!(stats.attempts, 3);
        // Two inter-attempt waits of 5ms; the terminal attempt does not wait.
        assert_eq!(stats.total_wait, Duration::from_millis(10));
        assert_eq!(stats.total_elapsed, Duration::from_millis(10));
        assert_eq!(stats.stop_reason, StopReason::Exhausted);
    }

    #[test]
    fn timeout_stops_the_loop_once_the_budget_is_exceeded() {
        // 10ms waits under a 25ms budget: attempts run at t=0,10,20,25; the last
        // wait is clamped to 5ms to land exactly on the deadline, where the next
        // boundary check exhausts the loop.
        let (result, stats) = retry(|_| Err::<i32, &str>("x"))
            .stop(stop::attempts(100))
            .wait(wait::fixed(Duration::from_millis(10)))
            .timeout(Duration::from_millis(25))
            .clock(VirtualClock::new())
            .with_stats()
            .call();

        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
        assert_eq!(stats.attempts, 4);
        assert_eq!(stats.total_elapsed, Duration::from_millis(25));
        assert_eq!(stats.total_wait, Duration::from_millis(25));
        assert_eq!(stats.stop_reason, StopReason::Exhausted);
    }
}
