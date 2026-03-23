use super::sync_exec::SyncSleep;
use crate::cancel::{AsyncCanceler, Canceler};
use crate::compat::Duration;
use crate::error::RetryError;
use crate::policy::time::ElapsedTracker;
use crate::policy::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::predicate::Predicate;
use crate::sleep::Sleeper;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::stats::{RetryStats, StopReason};
use crate::stop::Stop;
use crate::wait::Wait;
use core::future::Future;
use core::pin::Pin;
use core::task::Context;
use core::task::Poll;
use pin_project_lite::pin_project;

fn attempt_state_from<'a, T, E>(
    retry_state: &RetryState,
    outcome: &'a Result<T, E>,
    next_delay: Option<Duration>,
) -> AttemptState<'a, T, E> {
    AttemptState::new(
        retry_state.attempt,
        retry_state.elapsed,
        outcome,
        next_delay,
    )
}

fn exit_state_from<'a, T, E>(
    attempt_state: &AttemptState<'a, T, E>,
    stop_reason: StopReason,
) -> ExitState<'a, T, E> {
    ExitState::new(
        attempt_state.attempt,
        attempt_state.elapsed,
        Some(attempt_state.outcome),
        stop_reason,
    )
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

fn finish_cancelled<T, E, BA, AA, OX>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    last_result: Option<Result<T, E>>,
    attempts: u32,
    total_wait: Duration,
    collect_stats: bool,
    elapsed_tracker: &ElapsedTracker,
) -> (Result<T, RetryError<T, E>>, Option<RetryStats>)
where
    OX: ExitHook<T, E>,
{
    let elapsed = elapsed_tracker.elapsed();
    let stats = maybe_stats(
        collect_stats,
        attempts,
        elapsed,
        total_wait,
        StopReason::Cancelled,
    );

    let exit_state = ExitState::new(
        attempts,
        elapsed,
        last_result.as_ref(),
        StopReason::Cancelled,
    );
    hooks.on_exit.call(&exit_state);

    (Err(RetryError::Cancelled { last: last_result }), stats)
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
        TerminalOutcomeKind::AcceptedOutcome => StopReason::Accepted,
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

fn finish_async_poll<T, E, Fut, SleepFut, CancelFut>(
    mut phase: Pin<&mut AsyncPhase<Fut, SleepFut, CancelFut>>,
    final_stats: &mut Option<RetryStats>,
    result: Result<T, RetryError<T, E>>,
    stats: Option<RetryStats>,
) -> Poll<Result<T, RetryError<T, E>>> {
    *final_stats = stats;
    phase.set(AsyncPhase::Finished);
    Poll::Ready(result)
}

#[allow(clippy::too_many_arguments)]
fn finish_cancelled_async_poll<BA, AA, OX, T, E, Fut, SleepFut, CancelFut>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    last_result: &mut Option<Result<T, E>>,
    attempts: u32,
    total_wait: Duration,
    collect_stats: bool,
    elapsed_tracker: &ElapsedTracker,
    phase: Pin<&mut AsyncPhase<Fut, SleepFut, CancelFut>>,
    final_stats: &mut Option<RetryStats>,
) -> Poll<Result<T, RetryError<T, E>>>
where
    OX: ExitHook<T, E>,
{
    let (result, stats) = finish_cancelled(
        hooks,
        last_result.take(),
        attempts,
        total_wait,
        collect_stats,
        elapsed_tracker,
    );
    finish_async_poll(phase, final_stats, result, stats)
}

pub(super) enum AttemptTransition<T, E> {
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
pub(crate) enum AsyncPhase<Fut, SleepFut, CancelFut> {
        ReadyToStartAttempt,
        PollingOperation {
            #[pin]
            op_future: Fut,
        },
        Sleeping {
            #[pin]
            sleep_future: SleepFut,
            #[pin]
            cancel_future: CancelFut,
        },
        Finished,
    }
}

pub(crate) enum AsyncOperationPoll<T, E> {
    Pending,
    Finished {
        result: Result<T, RetryError<T, E>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
        last_result: Result<T, E>,
    },
}

pub(crate) fn remap_no_sleep_phase<Fut, OldSleepFut, OldCancelFut, NewSleepFut, NewCancelFut>(
    phase: AsyncPhase<Fut, OldSleepFut, OldCancelFut>,
    unreachable_message: &'static str,
) -> AsyncPhase<Fut, NewSleepFut, NewCancelFut> {
    match phase {
        AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
        AsyncPhase::PollingOperation { op_future } => AsyncPhase::PollingOperation { op_future },
        AsyncPhase::Sleeping { .. } => unreachable!("{unreachable_message}"),
        AsyncPhase::Finished => AsyncPhase::Finished,
    }
}

pub(crate) fn fire_before_attempt<BA, AA, OX>(
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    attempt: u32,
    elapsed: Option<Duration>,
) where
    BA: BeforeAttemptHook,
{
    let before_state = RetryState::new(attempt, elapsed);
    hooks.before_attempt.call(&before_state);
}

// Intentional: this helper wires all state-machine inputs in one place to keep
// async retry transition logic shared between policy and extension builders.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_operation_future<S, W, P, BA, AA, OX, Fut, T, E>(
    op_future: Pin<&mut Fut>,
    cx: &mut Context<'_>,
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    attempt: u32,
    elapsed_tracker: &ElapsedTracker,
    total_wait: Duration,
    collect_stats: bool,
    timeout: Option<Duration>,
) -> AsyncOperationPoll<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    Fut: Future<Output = Result<T, E>>,
{
    match op_future.poll(cx) {
        Poll::Pending => AsyncOperationPoll::Pending,
        Poll::Ready(outcome) => match transition_from_outcome(
            policy,
            hooks,
            outcome,
            attempt,
            elapsed_tracker.elapsed(),
            total_wait,
            collect_stats,
            timeout,
        ) {
            AttemptTransition::Finished { result, stats } => {
                AsyncOperationPoll::Finished { result, stats }
            }
            AttemptTransition::Sleep {
                next_delay,
                last_result,
            } => AsyncOperationPoll::Sleep {
                next_delay,
                last_result,
            },
        },
    }
}

pub(super) fn process_attempt_transition<S, W, P, BA, AA, OX, T, E>(
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    outcome: Result<T, E>,
    retry_state: RetryState,
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
    let should_retry = policy.predicate.should_retry(&outcome);

    if !should_retry {
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

    // Check both the policy's stop strategy and the timeout deadline.
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

    // after_attempt is fired by the caller after the step-10 cancellation
    // check so that the hook receives a truthful next_delay on all
    // termination paths detected before sleep.
    AttemptTransition::Sleep {
        next_delay,
        last_result: outcome,
    }
}

pub(super) fn transition_from_outcome<S, W, P, BA, AA, OX, T, E>(
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    outcome: Result<T, E>,
    attempt: u32,
    elapsed: Option<Duration>,
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
    let retry_state = RetryState::new(attempt, elapsed);

    process_attempt_transition(
        policy,
        hooks,
        outcome,
        retry_state,
        total_wait,
        collect_stats,
        timeout,
    )
}

pub(crate) fn execute_sync_loop<
    S,
    W,
    P,
    BA,
    AA,
    OX,
    F,
    SleepFn,
    T,
    E,
    C,
    const COLLECT_STATS: bool,
>(
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    op: &mut F,
    sleeper: &mut SleepFn,
    canceler: &C,
    elapsed_tracker: &ElapsedTracker,
    timeout: Option<Duration>,
) -> (Result<T, RetryError<T, E>>, Option<RetryStats>)
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut(RetryState) -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    let mut attempt: u32 = 1;
    let mut total_wait = Duration::ZERO;
    let mut last_result: Option<Result<T, E>> = None;

    loop {
        if canceler.is_cancelled() {
            let completed_attempts = attempt.saturating_sub(1);
            return finish_cancelled(
                hooks,
                last_result.take(),
                completed_attempts,
                total_wait,
                COLLECT_STATS,
                elapsed_tracker,
            );
        }

        fire_before_attempt(hooks, attempt, elapsed_tracker.elapsed());

        let state = RetryState::new(attempt, elapsed_tracker.elapsed());
        let outcome = (op)(state);
        match transition_from_outcome(
            policy,
            hooks,
            outcome,
            attempt,
            elapsed_tracker.elapsed(),
            total_wait,
            COLLECT_STATS,
            timeout,
        ) {
            AttemptTransition::Finished { result, stats } => return (result, stats),
            AttemptTransition::Sleep {
                mut next_delay,
                last_result: attempt_last_result,
            } => {
                // Step 7: Clamp delay to remaining timeout budget.
                if let (Some(timeout_dur), Some(elapsed)) = (timeout, elapsed_tracker.elapsed()) {
                    next_delay = next_delay.min(timeout_dur.saturating_sub(elapsed));
                }

                // Step 10: Check cancellation before committing to sleep.
                if canceler.is_cancelled() {
                    // Fire after_attempt with next_delay = None (step 10a).
                    let attempt_state = AttemptState::new(
                        attempt,
                        elapsed_tracker.elapsed(),
                        &attempt_last_result,
                        None,
                    );
                    hooks.after_attempt.call(&attempt_state);
                    return finish_cancelled(
                        hooks,
                        Some(attempt_last_result),
                        attempt,
                        total_wait,
                        COLLECT_STATS,
                        elapsed_tracker,
                    );
                }

                // Step 11: Fire after_attempt with next_delay = Some(delay).
                {
                    let retry_state = RetryState::new(attempt, elapsed_tracker.elapsed());
                    let attempt_state =
                        attempt_state_from(&retry_state, &attempt_last_result, Some(next_delay));
                    hooks.after_attempt.call(&attempt_state);
                }

                // Step 12: Zero-duration sleep rule: skip sleep when delay is zero.
                if !next_delay.is_zero() {
                    sleeper.sleep(next_delay);
                }
                total_wait = total_wait.saturating_add(next_delay);

                // Post-sleep cancellation check (step 1 of next iteration,
                // short-circuited here for efficiency).
                if canceler.is_cancelled() {
                    return finish_cancelled(
                        hooks,
                        Some(attempt_last_result),
                        attempt,
                        total_wait,
                        COLLECT_STATS,
                        elapsed_tracker,
                    );
                }

                // Step 13: increment attempt, continue.
                last_result = Some(attempt_last_result);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

pub(crate) fn poll_after_completion<T>(type_name: &str) -> Poll<T> {
    panic!("{type_name} polled after completion");
}

// Intentional: this is the shared async state-machine engine used by both
// policy-based and extension-trait async retry futures.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_async_loop<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>(
    cx: &mut Context<'_>,
    policy: &RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
    op: &mut F,
    sleeper: &SleepImpl,
    canceler: &C,
    last_result: &mut Option<Result<T, E>>,
    mut phase: Pin<&mut AsyncPhase<Fut, SleepFut, C::Cancel>>,
    attempt: &mut u32,
    total_wait: &mut Duration,
    collect_stats: bool,
    final_stats: &mut Option<RetryStats>,
    elapsed_tracker: &ElapsedTracker,
    timeout: Option<Duration>,
    completed_type_name: &'static str,
) -> Poll<Result<T, RetryError<T, E>>>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
    C: AsyncCanceler,
{
    loop {
        if canceler.is_cancelled() {
            let completed_attempts = match phase.as_ref().get_ref() {
                AsyncPhase::ReadyToStartAttempt => Some(attempt.saturating_sub(1)),
                AsyncPhase::Sleeping { .. } => Some(*attempt),
                AsyncPhase::PollingOperation { .. } | AsyncPhase::Finished => None,
            };
            if let Some(attempts) = completed_attempts {
                return finish_cancelled_async_poll(
                    hooks,
                    last_result,
                    attempts,
                    *total_wait,
                    collect_stats,
                    elapsed_tracker,
                    phase,
                    final_stats,
                );
            }
        }

        match phase.as_mut().project() {
            AsyncPhaseProj::ReadyToStartAttempt => {
                fire_before_attempt(hooks, *attempt, elapsed_tracker.elapsed());

                let state = RetryState::new(*attempt, elapsed_tracker.elapsed());
                phase.set(AsyncPhase::PollingOperation {
                    op_future: (op)(state),
                });
            }
            AsyncPhaseProj::PollingOperation { op_future } => match poll_operation_future(
                op_future,
                cx,
                policy,
                hooks,
                *attempt,
                elapsed_tracker,
                *total_wait,
                collect_stats,
                timeout,
            ) {
                AsyncOperationPoll::Pending => return Poll::Pending,
                AsyncOperationPoll::Finished { result, stats } => {
                    return finish_async_poll(phase, final_stats, result, stats);
                }
                AsyncOperationPoll::Sleep {
                    mut next_delay,
                    last_result: attempt_last_result,
                } => {
                    // Step 7: Clamp delay to remaining timeout budget.
                    if let (Some(timeout_dur), Some(elapsed)) = (timeout, elapsed_tracker.elapsed())
                    {
                        next_delay = next_delay.min(timeout_dur.saturating_sub(elapsed));
                    }

                    // Step 10: Check cancellation before committing to sleep.
                    if canceler.is_cancelled() {
                        // Fire after_attempt with next_delay = None (step 10a).
                        let attempt_state = AttemptState::new(
                            *attempt,
                            elapsed_tracker.elapsed(),
                            &attempt_last_result,
                            None,
                        );
                        hooks.after_attempt.call(&attempt_state);
                        *last_result = Some(attempt_last_result);
                        return finish_cancelled_async_poll(
                            hooks,
                            last_result,
                            *attempt,
                            *total_wait,
                            collect_stats,
                            elapsed_tracker,
                            phase,
                            final_stats,
                        );
                    }

                    // Step 11: Fire after_attempt with next_delay = Some(delay).
                    {
                        let retry_state = RetryState::new(*attempt, elapsed_tracker.elapsed());
                        let attempt_state = attempt_state_from(
                            &retry_state,
                            &attempt_last_result,
                            Some(next_delay),
                        );
                        hooks.after_attempt.call(&attempt_state);
                    }

                    *last_result = Some(attempt_last_result);
                    *total_wait = total_wait.saturating_add(next_delay);

                    // Step 12: Zero-duration sleep rule: skip sleep when delay is zero.
                    if next_delay.is_zero() {
                        *attempt = attempt.saturating_add(1);
                        phase.set(AsyncPhase::ReadyToStartAttempt);
                    } else {
                        phase.set(AsyncPhase::Sleeping {
                            sleep_future: sleeper.sleep(next_delay),
                            cancel_future: canceler.cancel(),
                        });
                    }
                }
            },
            AsyncPhaseProj::Sleeping {
                sleep_future,
                cancel_future,
            } => {
                if let Poll::Ready(()) = cancel_future.poll(cx) {
                    return finish_cancelled_async_poll(
                        hooks,
                        last_result,
                        *attempt,
                        *total_wait,
                        collect_stats,
                        elapsed_tracker,
                        phase,
                        final_stats,
                    );
                }

                let sleep_poll = sleep_future.poll(cx);
                if canceler.is_cancelled() {
                    return finish_cancelled_async_poll(
                        hooks,
                        last_result,
                        *attempt,
                        *total_wait,
                        collect_stats,
                        elapsed_tracker,
                        phase,
                        final_stats,
                    );
                }

                match sleep_poll {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        *attempt = attempt.saturating_add(1);
                        phase.set(AsyncPhase::ReadyToStartAttempt);
                    }
                }
            }
            AsyncPhaseProj::Finished => {
                return poll_after_completion(completed_type_name);
            }
        }
    }
}
