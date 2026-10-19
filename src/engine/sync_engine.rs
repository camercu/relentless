//! The synchronous classifier engine.
//!
//! [`Retry`] pairs an operation with a classifier, stop and wait strategies, a
//! clock and hooks; `.call()` runs the shared decision core in a blocking
//! loop. [`RetryExt`] starts the same builder from a no-argument closure. The
//! async twin lives in `async_engine`.

use super::elapsed::Elapsed;
use super::hooks::{
    AttemptHook, AttemptState, BeforeAttemptHook, ExecutionHooks, Exit, ExitHook, HookChain,
};
use super::op::{RetryOp, StatelessOp};
use super::step::{Progress, Step, step};
use super::{RetryError, RetryStats};
use crate::clock::{SyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{ClosureClassifier, Decide, DefaultClassifier, IntoDecision};
use crate::defaults;
use crate::predicate::Predicate;
use crate::predicate::classifier::{Until, When};
use crate::state::RetryState;
use crate::stop::{Stop, StopAfterAttempts};
use crate::wait::{Wait, WaitExponential};

/// Default builder returned by [`retry`], [`RetryExt::retry`], and friends.
type DefaultRetry<F> =
    Retry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock, (), (), ()>;

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
/// The operation receives the [`RetryState`]; when it ignores that state, the
/// [`RetryExt::retry`] extension form (`(|| op()).retry()`) is equivalent and
/// drops the unused argument. Defaults: `stop::attempts(3)`,
/// `wait::exponential(100ms)`, the default classifier, no hooks, and
/// [`SystemClock`]. In non-`std` builds `.clock(...)` must be set before
/// `.call()`.
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
            defaults::stop(),
            defaults::wait(),
            None,
        )
    }
}

impl<F, C, S, W> Retry<F, C, S, W, SystemClock, (), (), ()> {
    /// Assembles a retry from an operation and pre-chosen classifier/stop/wait,
    /// with the default clock and no hooks. Used by
    /// [`RetryPolicy::retry`](crate::RetryPolicy::retry) to borrow a reusable
    /// policy's parts and seed its timeout.
    pub(crate) fn from_parts(
        op: F,
        classifier: C,
        stop: S,
        wait: W,
        timeout: Option<Duration>,
    ) -> Self {
        Retry {
            op,
            classifier,
            stop,
            wait,
            clock: SystemClock,
            hooks: ExecutionHooks::new(),
            timeout,
        }
    }
}

/// Starts a sync retry directly from a no-argument closure or function.
///
/// This is the stateless twin of [`retry`]: `(|| op()).retry()` behaves
/// identically to `retry(|_| op())` — the extension form simply discards the
/// [`RetryState`]. Reach for the free [`retry`] when the operation needs that
/// state (e.g. the attempt number); the switch is mechanical, changing only the
/// closure's argument. Defaults match [`retry`]: `stop::attempts(3)`,
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
    /// abort) or [`Verdict`](crate::decision::Verdict) (abort-capable);
    /// [`IntoDecision`] unifies both. The
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

    /// Retries *until* `predicate` is satisfied, then accepts the outcome
    /// (`Ok` returns, a matched `Err` aborts with the bare error). The inverse
    /// of [`when`](Self::when); natural for polling (`.until(ok(is_ready))`).
    ///
    /// With `.until(ok(f))` errors are retried, because `ok(f)` is `false` for
    /// any `Err` and `until` inverts that. To make a fatal error terminate the
    /// loop instead, match on the whole outcome with
    /// [`result`](crate::predicate::result):
    ///
    /// ```
    /// use relentless::clock::VirtualClock;
    /// use relentless::{predicate, retry, stop, RetryError};
    ///
    /// // Poll until ready, but give up immediately on a fatal error.
    /// let outcome = retry(|_| Err::<&str, &str>("fatal"))
    ///     .until(predicate::result(|o: &Result<&str, &str>| {
    ///         matches!(o, Ok("ready") | Err("fatal"))
    ///     }))
    ///     .stop(stop::attempts(5))
    ///     .clock(VirtualClock::new())
    ///     .call();
    ///
    /// // The predicate matched an `Err`, so the loop aborts with the bare error
    /// // rather than returning it as `Ok`.
    /// assert_eq!(outcome, Err(RetryError::Aborted { last: "fatal" }));
    /// ```
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

    /// Registers a hook that runs once when the retry loop exits normally.
    ///
    /// It does not run if the operation or a hook panics: once a panic starts
    /// unwinding, the remaining hooks for that execution are skipped. Cleanup
    /// that must happen belongs in a `Drop` impl on your own type; keep
    /// `on_exit` for reporting the outcome.
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
            self.hooks.before_attempt.call(&before_state);
            let outcome = self.op.call_op(before_state);

            // Elapsed is re-read after the operation so the post-attempt hooks
            // and stop/wait see the time the attempt actually took.
            let post_elapsed = elapsed.read(&self.clock);

            match step(
                Progress {
                    attempt,
                    elapsed: post_elapsed,
                    previous_delay,
                    total_wait,
                },
                outcome,
                &self.classifier,
                &self.stop,
                &self.wait,
                self.timeout,
                &mut self.hooks,
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
