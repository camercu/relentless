use crate::clock::{AsyncClock, SyncClock};
use crate::compat::Duration;
use crate::error::RetryError;
use crate::policy::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, PolicyHandle, RetryPolicy,
};
use crate::predicate::Predicate;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::stats::{RetryStats, StopReason};
use crate::stop::Stop;
use crate::wait::Wait;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::Context;
use core::task::Poll;
use pin_project_lite::pin_project;

/// Unified callable interface for both the policy path (`FnMut(RetryState) -> Result<T, E>`)
/// and the ext-trait path (`StatelessOp<FnMut() -> Result<T, E>>`), so the sync
/// execution engine can drive both without a separate code path.
pub(crate) trait RetryOp<T, E> {
    fn call_op(&mut self, state: RetryState) -> Result<T, E>;
}

impl<T, E, F: FnMut(RetryState) -> Result<T, E>> RetryOp<T, E> for F {
    fn call_op(&mut self, state: RetryState) -> Result<T, E> {
        (self)(state)
    }
}

/// Async counterpart to [`RetryOp`]; same dual-path unification for the async
/// execution engine.
pub(crate) trait AsyncRetryOp<T, E, Fut: Future<Output = Result<T, E>>> {
    fn call_op(&mut self, state: RetryState) -> Fut;
}

impl<T, E, Fut: Future<Output = Result<T, E>>, F: FnMut(RetryState) -> Fut> AsyncRetryOp<T, E, Fut>
    for F
{
    fn call_op(&mut self, state: RetryState) -> Fut {
        (self)(state)
    }
}

fn attempt_state_from<'a, T, E>(
    retry_state: &RetryState,
    outcome: &'a Result<T, E>,
    next_delay: Option<Duration>,
) -> AttemptState<'a, T, E> {
    AttemptState::for_attempt(retry_state.attempt, outcome)
        .with_elapsed(retry_state.elapsed)
        .with_next_delay(next_delay)
}

fn exit_state_from<'a, T, E>(
    attempt_state: &AttemptState<'a, T, E>,
    stop_reason: StopReason,
) -> ExitState<'a, T, E> {
    ExitState::for_attempt(attempt_state.attempt, attempt_state.outcome, stop_reason)
        .with_elapsed(attempt_state.elapsed)
}

/// Clamps the next sleep delay to the remaining timeout budget, then fires the
/// `after_attempt` hook with the clamped value.
///
/// Shared by the sync and async loops so the hook always observes a truthful
/// delay (never an unclamped value) on every non-terminal path. Returns the
/// clamped delay for the caller to sleep on.
fn clamp_and_fire_after_attempt<BA, AA, OX, T, E>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    attempt: u32,
    elapsed: Option<Duration>,
    last_result: &Result<T, E>,
    next_delay: Duration,
    timeout: Option<Duration>,
) -> Duration
where
    AA: AttemptHook<T, E>,
{
    // Prevent sleeping past the deadline: cap the delay to the remaining
    // timeout budget so the loop terminates on time.
    let clamped = match (timeout, elapsed) {
        (Some(timeout_dur), Some(elapsed)) => next_delay.min(timeout_dur.saturating_sub(elapsed)),
        _ => next_delay,
    };

    let retry_state = RetryState::for_attempt(attempt).with_elapsed(elapsed);
    let attempt_state = attempt_state_from(&retry_state, last_result, Some(clamped));
    hooks.after_attempt.call(&attempt_state);

    clamped
}

#[derive(Clone, Copy)]
enum TerminalOutcomeKind {
    AcceptedOutcome,
    StopStrategyTriggered,
}

fn maybe_stats(
    collect_stats: bool,
    attempts: u32,
    total_elapsed: Option<Duration>,
    total_wait: Duration,
    stop_reason: StopReason,
) -> Option<RetryStats> {
    if collect_stats {
        Some(RetryStats {
            attempts,
            total_elapsed,
            total_wait,
            stop_reason,
        })
    } else {
        None
    }
}

fn finish_terminal_transition<BA, AA, OX, T, E>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    retry_state: &RetryState,
    outcome: Result<T, E>,
    total_wait: Duration,
    collect_stats: bool,
    outcome_kind: TerminalOutcomeKind,
) -> AttemptTransition<T, E>
where
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
{
    let reason = match outcome_kind {
        // An accepted outcome resolves to success on `Ok` and rejection on `Err`.
        TerminalOutcomeKind::AcceptedOutcome if outcome.is_ok() => StopReason::Succeeded,
        TerminalOutcomeKind::AcceptedOutcome => StopReason::Rejected,
        TerminalOutcomeKind::StopStrategyTriggered => StopReason::Exhausted,
    };

    let stats = maybe_stats(
        collect_stats,
        retry_state.attempt,
        retry_state.elapsed,
        total_wait,
        reason,
    );

    {
        let attempt_state = attempt_state_from(retry_state, &outcome, None);
        hooks.after_attempt.call(&attempt_state);
        let exit_state = exit_state_from(&attempt_state, reason);
        hooks.on_exit.call(&exit_state);
    }

    let result = match outcome_kind {
        TerminalOutcomeKind::AcceptedOutcome => match outcome {
            Ok(value) => Ok(value),
            Err(error) => Err(RetryError::Rejected { last: error }),
        },
        TerminalOutcomeKind::StopStrategyTriggered => Err(RetryError::Exhausted { last: outcome }),
    };

    AttemptTransition::Finished { result, stats }
}

enum AttemptTransition<T, E> {
    Finished {
        result: Result<T, RetryError<T, E>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
        last_result: Result<T, E>,
    },
}

pin_project! {
    #[project = AsyncPhaseProj]
    enum AsyncPhase<Fut, SleepFut> {
        ReadyToStartAttempt,
        PollingOperation {
            #[pin]
            op_future: Fut,
        },
        Sleeping {
            #[pin]
            sleep_future: SleepFut,
        },
        Finished,
    }
}

/// Fires the `before_attempt` hook and returns the state it observed, so the
/// caller passes the exact same snapshot (one clock read) to the operation.
fn fire_before_attempt<BA, AA, OX>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    attempt: u32,
    elapsed: Option<Duration>,
    previous_delay: Option<Duration>,
) -> RetryState
where
    BA: BeforeAttemptHook,
{
    let before_state = RetryState::for_attempt(attempt)
        .with_elapsed(elapsed)
        .with_previous_delay(previous_delay);
    hooks.before_attempt.call(&before_state);
    before_state
}

#[allow(clippy::too_many_arguments)]
fn transition_from_outcome<S, W, P, BA, AA, OX, T, E>(
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    outcome: Result<T, E>,
    attempt: u32,
    elapsed: Option<Duration>,
    previous_delay: Option<Duration>,
    total_wait: Duration,
    collect_stats: bool,
    timeout: Option<Duration>,
) -> AttemptTransition<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
{
    let retry_state = RetryState::for_attempt(attempt)
        .with_elapsed(elapsed)
        .with_previous_delay(previous_delay);

    if !policy.predicate.should_retry(&outcome) {
        return finish_terminal_transition(
            hooks,
            &retry_state,
            outcome,
            total_wait,
            collect_stats,
            TerminalOutcomeKind::AcceptedOutcome,
        );
    }

    let next_delay = policy.wait.next_wait(&retry_state);

    let timeout_exceeded = match (timeout, retry_state.elapsed) {
        (Some(t), Some(e)) => e >= t,
        _ => false,
    };

    if policy.stop.should_stop(&retry_state) || timeout_exceeded {
        return finish_terminal_transition(
            hooks,
            &retry_state,
            outcome,
            total_wait,
            collect_stats,
            TerminalOutcomeKind::StopStrategyTriggered,
        );
    }

    // after_attempt is fired by the caller (execute_sync_loop /
    // AsyncEngine::poll_step) after the timeout clamp, so the hook always
    // receives the actual sleep duration rather than an unclamped value.
    AttemptTransition::Sleep {
        next_delay,
        last_result: outcome,
    }
}

pub(crate) fn execute_sync_loop<S, W, P, BA, AA, OX, F, C, T, E, const COLLECT_STATS: bool>(
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    op: &mut F,
    clock: &C,
    timeout: Option<Duration>,
) -> (Result<T, RetryError<T, E>>, Option<RetryStats>)
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: RetryOp<T, E>,
    C: SyncClock,
{
    // Execution starts here: capture the elapsed baseline (SPEC 11.1.1).
    let origin = clock.now();
    // The clock is mandatory, so elapsed time is always available; the
    // `Option` shape survives only because the state types expose one.
    let elapsed = |clock: &C| Some(clock.now().saturating_sub(origin));

    let mut attempt: u32 = 1;
    let mut total_wait = Duration::ZERO;
    // The clamped delay applied before the current attempt, fed forward so
    // feedback wait strategies (e.g. decorrelated jitter) can read it.
    let mut previous_delay: Option<Duration> = None;

    loop {
        let state = fire_before_attempt(hooks, attempt, elapsed(clock), previous_delay);
        let outcome = op.call_op(state);
        match transition_from_outcome(
            policy,
            hooks,
            outcome,
            attempt,
            elapsed(clock),
            previous_delay,
            total_wait,
            COLLECT_STATS,
            timeout,
        ) {
            AttemptTransition::Finished { result, stats } => return (result, stats),
            AttemptTransition::Sleep {
                next_delay,
                last_result: attempt_last_result,
            } => {
                let next_delay = clamp_and_fire_after_attempt(
                    hooks,
                    attempt,
                    elapsed(clock),
                    &attempt_last_result,
                    next_delay,
                    timeout,
                );

                // Avoid a blocking syscall for zero-duration waits (e.g. when
                // the timeout budget is already exhausted).
                if !next_delay.is_zero() {
                    clock.wait(next_delay);
                }
                total_wait = total_wait.saturating_add(next_delay);
                previous_delay = Some(next_delay);

                attempt = attempt.saturating_add(1);
            }
        }
    }
}

pin_project! {
    /// The async retry state machine, constructed by `.call()` on the async
    /// builders once configuration is complete.
    ///
    /// Owns the same transition order as [`execute_sync_loop`]; the phases
    /// exist only because an async attempt or sleep can span multiple polls.
    pub(crate) struct AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut> {
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        clock: C,
        #[pin]
        phase: AsyncPhase<Fut, WaitFut>,
        attempt: u32,
        total_wait: Duration,
        previous_delay: Option<Duration>,
        // Clock reading captured at the first poll (SPEC 11.1.1); `None`
        // until execution starts.
        origin: Option<Duration>,
        timeout: Option<Duration>,
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

impl<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
    AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
{
    pub(crate) fn new(
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        clock: C,
        timeout: Option<Duration>,
    ) -> Self {
        Self {
            policy,
            hooks,
            op,
            clock,
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            previous_delay: None,
            origin: None,
            timeout,
            _marker: PhantomData,
        }
    }
}

impl<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
    AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
where
    Policy: PolicyHandle,
    Policy::Stop: Stop,
    Policy::Wait: Wait,
    Policy::Predicate: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    C: AsyncClock<Wait = WaitFut>,
    WaitFut: Future<Output = ()>,
{
    /// Advances the retry loop by one poll.
    ///
    /// Stats are always `Some` on completion when `COLLECT_STATS` is true,
    /// mirroring the sync engine's `execute::<COLLECT_STATS>`.
    ///
    /// # Panics
    ///
    /// Panics if called again after returning `Poll::Ready` (SPEC 15.2).
    #[allow(clippy::type_complexity)]
    pub(crate) fn poll_step<const COLLECT_STATS: bool>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<(Result<T, RetryError<T, E>>, Option<RetryStats>)> {
        let mut this = self.project();
        let policy = this.policy.policy_ref();

        // Execution starts at the first poll: capture the elapsed baseline
        // (SPEC 11.1.1). Later polls leave it unchanged.
        let origin = *this.origin.get_or_insert_with(|| this.clock.now());
        // The clock is mandatory, so elapsed time is always available; the
        // `Option` shape survives only because the state types expose one.
        let elapsed = |clock: &C| Some(clock.now().saturating_sub(origin));

        loop {
            match this.phase.as_mut().project() {
                AsyncPhaseProj::ReadyToStartAttempt => {
                    let state = fire_before_attempt(
                        this.hooks,
                        *this.attempt,
                        elapsed(this.clock),
                        *this.previous_delay,
                    );
                    this.phase.set(AsyncPhase::PollingOperation {
                        op_future: this.op.call_op(state),
                    });
                }
                AsyncPhaseProj::PollingOperation { op_future } => {
                    let outcome = match op_future.poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(outcome) => outcome,
                    };
                    match transition_from_outcome(
                        policy,
                        this.hooks,
                        outcome,
                        *this.attempt,
                        elapsed(this.clock),
                        *this.previous_delay,
                        *this.total_wait,
                        COLLECT_STATS,
                        *this.timeout,
                    ) {
                        AttemptTransition::Finished { result, stats } => {
                            this.phase.set(AsyncPhase::Finished);
                            return Poll::Ready((result, stats));
                        }
                        AttemptTransition::Sleep {
                            next_delay,
                            last_result: attempt_last_result,
                        } => {
                            let next_delay = clamp_and_fire_after_attempt(
                                this.hooks,
                                *this.attempt,
                                elapsed(this.clock),
                                &attempt_last_result,
                                next_delay,
                                *this.timeout,
                            );

                            *this.total_wait = this.total_wait.saturating_add(next_delay);
                            *this.previous_delay = Some(next_delay);

                            // Avoid spawning a zero-duration sleep future (e.g. when
                            // the timeout budget is already exhausted).
                            if next_delay.is_zero() {
                                *this.attempt = this.attempt.saturating_add(1);
                                this.phase.set(AsyncPhase::ReadyToStartAttempt);
                            } else {
                                this.phase.set(AsyncPhase::Sleeping {
                                    sleep_future: this.clock.wait_async(next_delay),
                                });
                            }
                        }
                    }
                }
                AsyncPhaseProj::Sleeping { sleep_future } => match sleep_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        *this.attempt = this.attempt.saturating_add(1);
                        this.phase.set(AsyncPhase::ReadyToStartAttempt);
                    }
                },
                AsyncPhaseProj::Finished => {
                    panic!("async retry future polled after completion");
                }
            }
        }
    }
}
