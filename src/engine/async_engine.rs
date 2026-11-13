//! The asynchronous classifier engine.
//!
//! The async twin of the sync [`Retry`](super::Retry) builder. It carries the
//! same configuration surface and the same by-value [`Decide`] classifier; the
//! only differences are that the operation returns a future
//! (`FnMut(RetryState) -> Fut`, `Fut: Future<Output = O>`) and that `.call()`
//! yields a future driven by an [`AsyncClock`].
//!
//! [`AsyncRun`] owns the same transition order as the sync `run` loop; the
//! phases exist only because an async attempt or sleep can span multiple polls.

use super::elapsed::Elapsed;
use super::op::{Async, Attempt, Operation, StatelessOp};
use super::step::{Progress, Step, step};
use super::{RetryError, RetryStats};
use crate::clock::{AsyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{ClosureClassifier, Decide, DefaultClassifier, IntoDecision};
use crate::policy::RetryPolicy;
use crate::predicate::Predicate;
use crate::predicate::classifier::{Until, When};
use crate::state::RetryState;
use crate::stop::{Stop, StopAfterAttempts};
use crate::wait::{Wait, WaitExponential};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use pin_project_lite::pin_project;

/// A classifier-driven async retry builder — the async twin of
/// [`Retry`](super::Retry).
pub struct AsyncRetry<F, C, S, W, Cl> {
    op: F,
    policy: RetryPolicy<S, W, C>,
    clock: Cl,
}

impl<F, C, S, W, Cl> core::fmt::Debug for AsyncRetry<F, C, S, W, Cl> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncRetry").finish_non_exhaustive()
    }
}

impl<F, C, S, W, Cl> core::fmt::Debug for AsyncRetryWithStats<F, C, S, W, Cl> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncRetryWithStats")
            .finish_non_exhaustive()
    }
}

/// Begins a classifier-driven async retry from an operation.
///
/// Defaults mirror [`retry`](super::retry): `stop::attempts(3)`,
/// `wait::exponential(100ms)`, the default classifier. There is no
/// default async clock — set one with `.clock(...)` before `.call()`.
pub fn retry_async<F, Fut, O>(op: F) -> DefaultAsyncRetry<F>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = O>,
{
    AsyncRetry::from_op(op)
}

/// Default async builder returned by [`retry_async`] and [`AsyncRetryExt::retry_async`].
type DefaultAsyncRetry<F> =
    AsyncRetry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock>;

impl<F> DefaultAsyncRetry<F> {
    /// Builds a default-configured async retry around an operation.
    fn from_op(op: F) -> Self {
        AsyncRetry::from_policy(op, RetryPolicy::new())
    }
}

impl<F, C, S, W> AsyncRetry<F, C, S, W, SystemClock> {
    /// Assembles an async retry from an operation and a policy, with the
    /// default clock. Used by
    /// [`RetryPolicy::retry_async`](crate::RetryPolicy::retry_async) to borrow a
    /// reusable policy.
    pub(crate) fn from_policy(op: F, policy: RetryPolicy<S, W, C>) -> Self {
        AsyncRetry {
            op,
            policy,
            clock: SystemClock,
        }
    }
}

/// Starts an async retry directly from a no-argument closure or function.
///
/// The async twin of [`RetryExt`](super::RetryExt): `(|| op()).retry_async()`
/// behaves identically to `retry_async(|_| op())`, discarding the
/// [`RetryState`]. Use the free [`retry_async`] when the operation needs that
/// state; the switch changes only the closure's argument.
pub trait AsyncRetryExt<Fut, O>: FnMut() -> Fut + Sized
where
    Fut: Future<Output = O>,
{
    /// Begins an owned async retry builder from this closure.
    fn retry_async(self) -> DefaultAsyncRetry<StatelessOp<Self>>;
}

impl<Fut, O, F> AsyncRetryExt<Fut, O> for F
where
    F: FnMut() -> Fut,
    Fut: Future<Output = O>,
{
    fn retry_async(self) -> DefaultAsyncRetry<StatelessOp<Self>> {
        AsyncRetry::from_op(StatelessOp(self))
    }
}

impl<F, C, S, W, Cl> AsyncRetry<F, C, S, W, Cl> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> AsyncRetry<F, C, NewStop, W, Cl> {
        AsyncRetry {
            op: self.op,
            policy: self.policy.stop(stop),
            clock: self.clock,
        }
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> AsyncRetry<F, C, S, NewWait, Cl> {
        AsyncRetry {
            op: self.op,
            policy: self.policy.wait(wait),
            clock: self.clock,
        }
    }

    /// Sets the async clock that supplies elapsed time and performs waits.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> AsyncRetry<F, C, S, W, NewClock> {
        AsyncRetry {
            op: self.op,
            policy: self.policy,
            clock,
        }
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// Like [`Retry::decide`](super::Retry::decide), the op-anchored bound infers
    /// the closure parameter from the operation's output.
    #[must_use]
    pub fn decide<O, D, NewC>(
        self,
        classifier: NewC,
    ) -> AsyncRetry<F, ClosureClassifier<NewC>, S, W, Cl>
    where
        F: Operation<Async, Output = O>,
        NewC: Fn(O) -> D,
        D: IntoDecision<O>,
    {
        self.with_classifier(ClosureClassifier(classifier))
    }

    /// Retries while `predicate` wants to; otherwise accepts the outcome
    /// (`Ok` returns, a rejected `Err` aborts with the bare error).
    /// `Result`-only sugar over [`decide`](Self::decide).
    #[must_use]
    pub fn when<T, E, P>(self, predicate: P) -> AsyncRetry<F, When<P>, S, W, Cl>
    where
        F: Operation<Async, Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(When(predicate))
    }

    /// Retries *until* `predicate` is satisfied, then accepts the outcome
    /// (`Ok` returns, a matched `Err` aborts with the bare error). The inverse
    /// of [`when`](Self::when); see [`Retry::until`](super::Retry::until) for
    /// the fatal-error recipe.
    #[must_use]
    pub fn until<T, E, P>(self, predicate: P) -> AsyncRetry<F, Until<P>, S, W, Cl>
    where
        F: Operation<Async, Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(Until(predicate))
    }

    /// Sets a wall-clock budget for the whole execution (boundary check between
    /// attempts; see [`Retry::timeout`](super::Retry::timeout)).
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.policy = self.policy.timeout(dur);
        self
    }

    /// Wraps this execution so its future also yields [`RetryStats`].
    #[must_use]
    pub fn with_stats(self) -> AsyncRetryWithStats<F, C, S, W, Cl> {
        AsyncRetryWithStats { inner: self }
    }

    fn with_classifier<NewC>(self, classifier: NewC) -> AsyncRetry<F, NewC, S, W, Cl> {
        AsyncRetry {
            op: self.op,
            policy: self.policy.with_classifier(classifier),
            clock: self.clock,
        }
    }
}

impl<F, C, S, W, Cl, O> AsyncRetry<F, C, S, W, Cl>
where
    F: Operation<Async, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
{
    /// Drives the async retry loop to completion.
    ///
    /// The returned future is cancel-safe: dropping it stops the loop at the
    /// next `.await`.
    ///
    /// # Panics
    ///
    /// The returned future panics if it is polled again after it has returned
    /// [`Poll::Ready`]. The reachable case is
    /// `tokio::select!` over a `&mut` future in a loop, which re-polls
    /// whichever branch did not complete.
    ///
    /// Stop polling it once it completes — `break` out of the loop, or drop
    /// the future. Do **not** reach for `futures::FutureExt::fuse` here:
    /// `tokio::select!` never consults
    /// [`FusedFuture`](https://docs.rs/futures/latest/futures/future/trait.FusedFuture.html),
    /// and a fused future returns `Poll::Pending` forever once resolved, so
    /// fusing trades a panic that names the offending line for a task that
    /// hangs silently. (Under `futures::select!`, which does consult it,
    /// fusing is correct.)
    ///
    /// It also panics, in debug builds only, if the injected
    /// [`Clock`](crate::clock::Clock) reports an elapsed reading below one it
    /// already reported — see [`Retry::call`](super::Retry::call).
    #[allow(clippy::type_complexity)]
    pub fn call(self) -> DropStats<AsyncRun<F, C, S, W, Cl>> {
        DropStats {
            inner: self.into_run(),
        }
    }

    fn into_run(self) -> AsyncRun<F, C, S, W, Cl> {
        AsyncRun {
            op: self.op,
            policy: self.policy,
            clock: self.clock,
            phase: Phase::ReadyToStart,
            attempt: 1,
            previous_delay: None,
            total_wait: Duration::ZERO,
            elapsed: None,
        }
    }
}

/// A [`AsyncRetry`] wrapper whose future also yields [`RetryStats`]. Created by
/// [`AsyncRetry::with_stats`].
pub struct AsyncRetryWithStats<F, C, S, W, Cl> {
    inner: AsyncRetry<F, C, S, W, Cl>,
}

impl<F, C, S, W, Cl, O> AsyncRetryWithStats<F, C, S, W, Cl>
where
    F: Operation<Async, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
{
    /// Drives the async retry loop, yielding both the result and the stats.
    ///
    /// # Panics
    ///
    /// The returned future panics if it is polled again after it has returned
    /// [`Poll::Ready`], and — in debug builds only —
    /// if the injected [`Clock`](crate::clock::Clock) reports an elapsed
    /// reading below one it already reported. See
    /// [`AsyncRetry::call`](AsyncRetry::call).
    pub fn call(self) -> AsyncRun<F, C, S, W, Cl> {
        self.inner.into_run()
    }
}

pin_project! {
    #[project = PhaseProj]
    enum Phase<Fut, WaitFut> {
        ReadyToStart,
        Polling { #[pin] op_future: Fut },
        Sleeping { #[pin] sleep_future: WaitFut },
        /// Parked on a zero delay: the cooperative yield's counterpart to
        /// `Sleeping`. Both mean "between attempts, waiting to be polled
        /// again", and both advance the attempt counter when they end — so
        /// the counter cannot mean two different things at a poll boundary
        /// depending on whether the delay happened to be zero.
        Yielding,
        Done,
    }
}

pin_project! {
    /// The async retry state machine. Yields `(result, stats)`; the public
    /// `.call()` drops the stats via [`DropStats`].
    ///
    /// # Panics
    ///
    /// Polling this future after it has returned
    /// [`Poll::Ready`] panics.
    ///
    /// Polling also panics, in debug builds only, if the injected
    /// [`Clock`](crate::clock::Clock) reports an elapsed reading below one it
    /// already reported — a violation of
    /// [`Clock::now`](crate::clock::Clock::now)'s monotonicity precondition.
    /// Release builds clamp instead, so a broken clock cannot un-spend the
    /// timeout budget either way; the assertion exists to surface the broken
    /// clock in its implementor's own tests. No clock shipped with this crate
    /// can trigger it.
    pub struct AsyncRun<F, C, S, W, Cl>
    where
        F: Operation<Async>,
        Cl: AsyncClock,
    {
        op: F,
        policy: RetryPolicy<S, W, C>,
        clock: Cl,
        #[pin]
        phase: Phase<F::Call, Cl::Wait>,
        attempt: u32,
        previous_delay: Option<Duration>,
        total_wait: Duration,
        elapsed: Option<Elapsed>,
    }
}

impl<F, C, S, W, Cl, O> Future for AsyncRun<F, C, S, W, Cl>
where
    F: Operation<Async, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
{
    type Output = (Result<C::R, RetryError<C::A, O>>, RetryStats);

    #[allow(clippy::type_complexity)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // Execution starts at the first poll: capture the elapsed baseline.
        let elapsed = this
            .elapsed
            .get_or_insert_with(|| Elapsed::start(this.clock));

        loop {
            match this.phase.as_mut().project() {
                PhaseProj::ReadyToStart => {
                    let before_state = RetryState::for_attempt(*this.attempt)
                        .with_elapsed(elapsed.read(this.clock))
                        .with_previous_delay(*this.previous_delay);
                    let op_future = this.op.call_op(before_state);
                    this.phase.set(Phase::Polling { op_future });
                }
                PhaseProj::Polling { op_future } => {
                    // The mode is spelled out so the blanket `Attempt<Blocking>`
                    // impl, which every type satisfies, is never a candidate.
                    let polled = Attempt::<Async>::poll_attempt(op_future, cx, ());
                    let outcome = match polled {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(outcome) => outcome,
                    };
                    let post_elapsed = elapsed.read(this.clock);

                    match step(
                        Progress {
                            attempt: *this.attempt,
                            elapsed: post_elapsed,
                            previous_delay: *this.previous_delay,
                            total_wait: *this.total_wait,
                        },
                        outcome,
                        this.policy,
                    ) {
                        Step::Done { result, stats } => {
                            this.phase.set(Phase::Done);
                            return Poll::Ready((result, stats));
                        }
                        Step::Continue {
                            delay,
                            total_wait: running,
                        } => {
                            *this.total_wait = running;
                            *this.previous_delay = Some(delay);

                            // The poll-driven sleep is the async engine's half of
                            // the loop; the sync engine blocks instead.
                            if delay.is_zero() {
                                // No sleep future for a zero delay, but still a
                                // yield: a loop that never returns `Pending`
                                // owns its executor thread until it finishes,
                                // so timers cannot fire and `select!`,
                                // `timeout` and cancellation are all defeated.
                                // Waking first is what keeps this a yield
                                // rather than a stall.
                                this.phase.set(Phase::Yielding);
                                cx.waker().wake_by_ref();
                                return Poll::Pending;
                            }

                            let sleep_future = this.clock.wait_async(delay);
                            this.phase.set(Phase::Sleeping { sleep_future });
                        }
                    }
                }
                PhaseProj::Sleeping { sleep_future } => match sleep_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        *this.attempt = this.attempt.saturating_add(1);
                        this.phase.set(Phase::ReadyToStart);
                    }
                },
                PhaseProj::Yielding => {
                    *this.attempt = this.attempt.saturating_add(1);
                    this.phase.set(Phase::ReadyToStart);
                }
                PhaseProj::Done => panic!("async retry future polled after completion"),
            }
        }
    }
}

pin_project! {
    /// Adapts an [`AsyncRun`] future so it yields only the result, dropping the
    /// stats. Returned by [`AsyncRetry::call`].
    pub struct DropStats<Inner> {
        #[pin]
        inner: Inner,
    }
}

impl<Inner, R, E> Future for DropStats<Inner>
where
    Inner: Future<Output = (Result<R, E>, RetryStats)>,
{
    type Output = Result<R, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().inner.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready((result, _stats)) => Poll::Ready(result),
        }
    }
}
