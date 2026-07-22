//! The asynchronous classifier engine (ADR-6 S6).
//!
//! The async twin of the sync [`Retry`](super::Retry) builder. It carries the
//! same configuration surface and the same by-value [`Decide`] classifier; the
//! only differences are that the operation returns a future
//! (`FnMut(RetryState) -> Fut`, `Fut: Future<Output = O>`) and that `.call()`
//! yields a future driven by an [`AsyncClock`].
//!
//! [`AsyncRun`] owns the same transition order as the sync `run` loop; the
//! phases exist only because an async attempt or sleep can span multiple polls.

use super::hooks::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, HookChain};
use super::op::{AsyncRetryOp, StatelessOp};
use super::state::{AttemptState, Exit, StopReason};
use super::{RetryError, RetryStats};
use crate::clock::{AsyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{
    ClosureClassifier, Decide, DefaultClassifier, IntoDecision, Until, Verdict, When,
};
use crate::predicate::Predicate;
use crate::state::RetryState;
use crate::stop::{self, Stop, StopAfterAttempts};
use crate::wait::{self, Wait, WaitExponential};
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};
use pin_project_lite::pin_project;

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// A classifier-driven async retry builder — the async twin of
/// [`Retry`](super::Retry).
pub struct AsyncRetry<F, C, S, W, Cl, BA, AA, OX> {
    op: F,
    classifier: C,
    stop: S,
    wait: W,
    clock: Cl,
    hooks: ExecutionHooks<BA, AA, OX>,
    timeout: Option<Duration>,
}

impl<F, C, S, W, Cl, BA, AA, OX> core::fmt::Debug for AsyncRetry<F, C, S, W, Cl, BA, AA, OX> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncRetry").finish_non_exhaustive()
    }
}

impl<F, C, S, W, Cl, BA, AA, OX> core::fmt::Debug
    for AsyncRetryWithStats<F, C, S, W, Cl, BA, AA, OX>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncRetryWithStats")
            .finish_non_exhaustive()
    }
}

/// Begins a classifier-driven async retry from an operation.
///
/// Defaults mirror [`retry`](super::retry): `stop::attempts(3)`,
/// `wait::exponential(100ms)`, the default classifier, and no hooks. There is no
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
    AsyncRetry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock, (), (), ()>;

impl<F> DefaultAsyncRetry<F> {
    /// Builds a default-configured async retry around an operation.
    fn from_op(op: F) -> Self {
        AsyncRetry::from_parts(
            op,
            DefaultClassifier,
            stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait::exponential(DEFAULT_INITIAL_WAIT),
        )
    }
}

impl<F, C, S, W> AsyncRetry<F, C, S, W, SystemClock, (), (), ()> {
    /// Assembles an async retry from an operation and pre-chosen
    /// classifier/stop/wait, with no hooks or timeout. Used by
    /// [`RetryPolicy::retry_async`](crate::RetryPolicy::retry_async) to borrow a
    /// reusable policy's parts.
    pub(crate) fn from_parts(op: F, classifier: C, stop: S, wait: W) -> Self {
        AsyncRetry {
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

/// Starts an async retry directly from a no-argument closure or function.
///
/// The async twin of [`RetryExt`](super::RetryExt); use [`retry_async`] when you
/// need the [`RetryState`].
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

impl<F, C, S, W, Cl, BA, AA, OX> AsyncRetry<F, C, S, W, Cl, BA, AA, OX> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> AsyncRetry<F, C, NewStop, W, Cl, BA, AA, OX> {
        AsyncRetry {
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
    pub fn wait<NewWait>(self, wait: NewWait) -> AsyncRetry<F, C, S, NewWait, Cl, BA, AA, OX> {
        AsyncRetry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait,
            clock: self.clock,
            hooks: self.hooks,
            timeout: self.timeout,
        }
    }

    /// Sets the async clock that supplies elapsed time and performs waits.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> AsyncRetry<F, C, S, W, NewClock, BA, AA, OX> {
        AsyncRetry {
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
    /// Like [`Retry::decide`](super::Retry::decide), the op-anchored bound infers
    /// the closure parameter from the operation's output, reached here through
    /// `Fut::Output`.
    #[must_use]
    pub fn decide<O, D, NewC>(
        self,
        classifier: NewC,
    ) -> AsyncRetry<F, ClosureClassifier<NewC>, S, W, Cl, BA, AA, OX>
    where
        F: AsyncRetryOp<Output = O>,
        NewC: Fn(O) -> D,
        D: IntoDecision<O>,
    {
        self.with_classifier(ClosureClassifier(classifier))
    }

    /// Retries while `predicate` wants to; otherwise accepts. `Result`-only
    /// sugar over [`decide`](Self::decide).
    #[must_use]
    pub fn when<T, E, P>(self, predicate: P) -> AsyncRetry<F, When<P>, S, W, Cl, BA, AA, OX>
    where
        F: AsyncRetryOp<Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(When(predicate))
    }

    /// Retries *until* `predicate` is satisfied, then accepts.
    #[must_use]
    pub fn until<T, E, P>(self, predicate: P) -> AsyncRetry<F, Until<P>, S, W, Cl, BA, AA, OX>
    where
        F: AsyncRetryOp<Output = Result<T, E>>,
        P: Predicate<T, E>,
    {
        self.with_classifier(Until(predicate))
    }

    /// Sets a wall-clock budget for the whole execution (boundary check between
    /// attempts; see [`Retry::timeout`](super::Retry::timeout)).
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Wraps this execution so its future also yields [`RetryStats`].
    #[must_use]
    pub fn with_stats(self) -> AsyncRetryWithStats<F, C, S, W, Cl, BA, AA, OX> {
        AsyncRetryWithStats { inner: self }
    }

    fn with_classifier<NewC>(self, classifier: NewC) -> AsyncRetry<F, NewC, S, W, Cl, BA, AA, OX> {
        AsyncRetry {
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
    ) -> AsyncRetry<F, C, S, W, Cl, HookChain<BA, Hook>, AA, OX>
    where
        Hook: FnMut(&RetryState),
    {
        AsyncRetry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks.chain_before_attempt(hook),
            timeout: self.timeout,
        }
    }

    /// Registers a hook that runs after each attempt, before classification.
    #[must_use]
    pub fn after_attempt<O, Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<F, C, S, W, Cl, BA, HookChain<AA, Hook>, OX>
    where
        F: AsyncRetryOp<Output = O>,
        Hook: for<'a> FnMut(&AttemptState<'a, O>),
    {
        AsyncRetry {
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
    pub fn on_exit<O, Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<F, C, S, W, Cl, BA, AA, HookChain<OX, Hook>>
    where
        F: AsyncRetryOp<Output = O>,
        C: Decide<O>,
        Hook: for<'a> FnMut(&Exit<'a, C::R, C::A, O>),
    {
        AsyncRetry {
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

impl<F, Fut, C, S, W, Cl, BA, AA, OX, O> AsyncRetry<F, C, S, W, Cl, BA, AA, OX>
where
    F: AsyncRetryOp<Output = O, Future = Fut>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
    BA: BeforeAttemptHook,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    /// Drives the async retry loop to completion.
    ///
    /// The returned future is cancel-safe: dropping it stops the loop at the
    /// next `.await` (`on_exit` does not fire on drop).
    #[allow(clippy::type_complexity)]
    pub fn call(self) -> DropStats<AsyncRun<F, Fut, C, S, W, Cl, BA, AA, OX, O>> {
        DropStats {
            inner: self.into_run(),
        }
    }

    fn into_run(self) -> AsyncRun<F, Fut, C, S, W, Cl, BA, AA, OX, O> {
        AsyncRun {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
            hooks: self.hooks,
            timeout: self.timeout,
            phase: Phase::ReadyToStart,
            attempt: 1,
            previous_delay: None,
            total_wait: Duration::ZERO,
            origin: None,
            _marker: PhantomData,
        }
    }
}

/// A [`AsyncRetry`] wrapper whose future also yields [`RetryStats`]. Created by
/// [`AsyncRetry::with_stats`].
pub struct AsyncRetryWithStats<F, C, S, W, Cl, BA, AA, OX> {
    inner: AsyncRetry<F, C, S, W, Cl, BA, AA, OX>,
}

impl<F, Fut, C, S, W, Cl, BA, AA, OX, O> AsyncRetryWithStats<F, C, S, W, Cl, BA, AA, OX>
where
    F: AsyncRetryOp<Output = O, Future = Fut>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
    BA: BeforeAttemptHook,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    /// Drives the async retry loop, yielding both the result and the stats.
    pub fn call(self) -> AsyncRun<F, Fut, C, S, W, Cl, BA, AA, OX, O> {
        self.inner.into_run()
    }
}

pin_project! {
    #[project = PhaseProj]
    enum Phase<Fut, WaitFut> {
        ReadyToStart,
        Polling { #[pin] op_future: Fut },
        Sleeping { #[pin] sleep_future: WaitFut },
        Done,
    }
}

pin_project! {
    /// The async retry state machine. Yields `(result, stats)`; the public
    /// `.call()` drops the stats via [`DropStats`].
    pub struct AsyncRun<F, Fut, C, S, W, Cl, BA, AA, OX, O>
    where
        Cl: AsyncClock,
    {
        op: F,
        classifier: C,
        stop: S,
        wait: W,
        clock: Cl,
        hooks: ExecutionHooks<BA, AA, OX>,
        timeout: Option<Duration>,
        #[pin]
        phase: Phase<Fut, Cl::Wait>,
        attempt: u32,
        previous_delay: Option<Duration>,
        total_wait: Duration,
        origin: Option<Duration>,
        _marker: PhantomData<fn() -> O>,
    }
}

impl<F, Fut, C, S, W, Cl, BA, AA, OX, O> Future for AsyncRun<F, Fut, C, S, W, Cl, BA, AA, OX, O>
where
    F: AsyncRetryOp<Output = O, Future = Fut>,
    Fut: Future<Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: AsyncClock,
    BA: BeforeAttemptHook,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    type Output = (Result<C::R, RetryError<C::A, O>>, RetryStats);

    #[allow(clippy::type_complexity)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // Execution starts at the first poll: capture the elapsed baseline.
        let origin = *this.origin.get_or_insert_with(|| this.clock.now());
        let elapsed = |clock: &Cl| clock.now().saturating_sub(origin);

        loop {
            match this.phase.as_mut().project() {
                PhaseProj::ReadyToStart => {
                    let before_state = RetryState::for_attempt(*this.attempt)
                        .with_elapsed(elapsed(this.clock))
                        .with_previous_delay(*this.previous_delay);
                    this.hooks.before_attempt.call(&before_state);
                    let op_future = this.op.call_op(before_state);
                    this.phase.set(Phase::Polling { op_future });
                }
                PhaseProj::Polling { op_future } => {
                    let outcome = match op_future.poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(outcome) => outcome,
                    };
                    let post_elapsed = elapsed(this.clock);

                    {
                        let attempt_state =
                            AttemptState::new(*this.attempt, post_elapsed, &outcome);
                        this.hooks.after_attempt.call(&attempt_state);
                    }

                    let state = RetryState::for_attempt(*this.attempt)
                        .with_elapsed(post_elapsed)
                        .with_previous_delay(*this.previous_delay);

                    let stats_for = |reason| RetryStats {
                        attempts: *this.attempt,
                        total_elapsed: post_elapsed,
                        total_wait: *this.total_wait,
                        stop_reason: reason,
                    };

                    match this.classifier.decide(outcome) {
                        Verdict::Return(value) => {
                            this.hooks.on_exit.call(&Exit::Returned {
                                attempt: *this.attempt,
                                elapsed: post_elapsed,
                                value: &value,
                            });
                            this.phase.set(Phase::Done);
                            return Poll::Ready((Ok(value), stats_for(StopReason::Returned)));
                        }
                        Verdict::Abort(last) => {
                            this.hooks.on_exit.call(&Exit::Aborted {
                                attempt: *this.attempt,
                                elapsed: post_elapsed,
                                last: &last,
                            });
                            this.phase.set(Phase::Done);
                            return Poll::Ready((
                                Err(RetryError::Aborted { last }),
                                stats_for(StopReason::Aborted),
                            ));
                        }
                        Verdict::Retry(last) => {
                            let timeout_exceeded = this.timeout.is_some_and(|t| post_elapsed >= t);
                            if this.stop.should_stop(&state) || timeout_exceeded {
                                this.hooks.on_exit.call(&Exit::Exhausted {
                                    attempt: *this.attempt,
                                    elapsed: post_elapsed,
                                    last: &last,
                                });
                                this.phase.set(Phase::Done);
                                return Poll::Ready((
                                    Err(RetryError::Exhausted { last }),
                                    stats_for(StopReason::Exhausted),
                                ));
                            }

                            // Consult the wait strategy only now that a retry is
                            // certain (matching the sync engine).
                            let next_delay = this.wait.next_wait(&state);
                            let delay = match *this.timeout {
                                Some(t) => next_delay.min(t.saturating_sub(post_elapsed)),
                                None => next_delay,
                            };
                            *this.total_wait = this.total_wait.saturating_add(delay);
                            *this.previous_delay = Some(delay);

                            if delay.is_zero() {
                                // Skip spawning a zero-duration sleep future.
                                *this.attempt = this.attempt.saturating_add(1);
                                this.phase.set(Phase::ReadyToStart);
                            } else {
                                let sleep_future = this.clock.wait_async(delay);
                                this.phase.set(Phase::Sleeping { sleep_future });
                            }
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

// A heap-allocated waker is the only safe way to poll a future without an
// executor (the crate forbids the unsafe `RawWaker`), so these unit tests need
// `alloc`. The pure-no-alloc async path is exercised by integration tests.
#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use crate::clock::VirtualClock;
    use crate::decision::{Decision, Verdict};
    use crate::predicate;
    use alloc::sync::Arc;
    use alloc::task::Wake;
    use core::cell::Cell;

    /// Polls a future to completion under a no-op waker. The virtual clock's
    /// async waits resolve on poll, so no real executor is needed.
    fn block_on<Fut: Future>(future: Fut) -> Fut::Output {
        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }
        let mut future = core::pin::pin!(future);
        let waker = core::task::Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(output) = Future::poll(future.as_mut(), &mut cx) {
                return output;
            }
        }
    }

    type IntResult = Result<i32, &'static str>;
    const ARBITRARY_ATTEMPTS: u32 = 5;

    #[test]
    fn async_retry_ext_starts_from_a_no_arg_closure() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            (|| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") } }
            })
            .retry_async()
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(5));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn async_default_path_retries_then_returns_ok() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 2 { Ok::<i32, &str>(7) } else { Err("x") } }
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(7));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn async_decide_infers_through_fut_output() {
        // Inline classifier with no annotations: the parameter is typed through
        // `Fut::Output`.
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| async { Err::<i32, &str>("fatal") })
                .decide(|o| match o {
                    Ok(v) => Verdict::Return(v),
                    Err("fatal") => Verdict::Abort("boom"),
                    Err(_) => Verdict::Retry(o),
                })
                .stop(stop::attempts(ARBITRARY_ATTEMPTS))
                .wait(wait::fixed(Duration::ZERO))
                .clock(&clock)
                .call(),
        );

        assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
    }

    #[test]
    fn async_decide_returns_sought_error_through_ok() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 3 { Err("crash") } else { Ok(()) } }
            })
            .decide(|o| match o {
                Err(e) => Decision::Return(e),
                Ok(()) => Decision::Retry(o),
            })
            .stop(stop::attempts(10))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok("crash"));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn async_until_polls_a_result() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { Ok::<i32, &str>(n) }
            })
            .until(predicate::ok(|v: &i32| *v >= 3))
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(3));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn async_with_stats_and_on_exit_match_the_sync_engine() {
        let reason = Cell::new(None);
        let clock = VirtualClock::new();
        let (result, stats) = block_on(
            retry_async(|_| async { Err::<i32, &str>("boom") })
                .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
                .stop(stop::attempts(3))
                .wait(wait::fixed(Duration::from_millis(5)))
                .clock(&clock)
                .with_stats()
                .call(),
        );

        assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.total_wait, Duration::from_millis(10));
        assert_eq!(reason.get(), Some(StopReason::Exhausted));
    }
}
