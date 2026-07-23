//! Acceptance tests for the async execution path via `RetryPolicy::retry_async`.
//!
//! All tests run under a minimal in-process `block_on` executor so they are
//! deterministic and executor-agnostic: no Tokio runtime, no real timers. The
//! `RecordingClock` captures requested waits (advancing its own `now`, so the
//! elapsed seam stays coherent) without blocking, letting tests verify
//! wait-strategy output without wall-clock delays. Feature-gated tests check
//! that the runtime clock adapters satisfy the async engine's bound.

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use relentless::{RetryError, RetryPolicy};
use relentless::{predicate, stop, wait};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);
/// Deadline shorter than `ASYNC_CUSTOM_CLOCK_STEP_MILLIS` so the first attempt always exhausts it.
const ASYNC_CUSTOM_CLOCK_DEADLINE: Duration = Duration::from_millis(5);
const ASYNC_CUSTOM_CLOCK_STEP_MILLIS: u64 = 10;
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";

// Helpers

/// Creates a no-op waker for polling futures without an executor.
fn noop_waker() -> Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    Waker::from(Arc::new(NoopWake))
}

/// Polls a future to completion. Only correct for futures that never return `Pending`
/// permanently; yields the thread on each `Pending` to let cooperative tasks make progress.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Test-side [`relentless::AsyncClock`]: waits resolve immediately, advancing
/// the same `now` cell the elapsed seam reads (coherent by construction) and
/// recording each requested wait. Cloning shares the underlying cells.
#[derive(Clone, Default)]
struct RecordingClock {
    inner: Rc<RecordingClockCells>,
}

#[derive(Default)]
struct RecordingClockCells {
    now: Cell<Duration>,
    calls: RefCell<Vec<Duration>>,
}

impl RecordingClock {
    fn new() -> Self {
        Self::default()
    }

    /// Waits requested so far, in request order.
    fn calls(&self) -> Vec<Duration> {
        self.inner.calls.borrow().clone()
    }

    /// Advances virtual time without recording a wait (simulates op runtime).
    fn advance(&self, dur: Duration) {
        self.inner.now.set(self.inner.now.get().saturating_add(dur));
    }
}

impl relentless::Clock for RecordingClock {
    fn now(&self) -> Duration {
        self.inner.now.get()
    }
}

impl relentless::AsyncClock for RecordingClock {
    type Wait = RecordingWait;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        RecordingWait {
            inner: Rc::clone(&self.inner),
            dur,
            done: false,
        }
    }
}

/// Wait future of [`RecordingClock`]: advances and records on first poll, so
/// an unpolled (cancelled) wait leaves time untouched.
struct RecordingWait {
    inner: Rc<RecordingClockCells>,
    dur: Duration,
    done: bool,
}

impl Future for RecordingWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if !this.done {
            this.done = true;
            this.inner
                .now
                .set(this.inner.now.get().saturating_add(this.dur));
            this.inner.calls.borrow_mut().push(this.dur);
        }
        Poll::Ready(())
    }
}

/// End-to-end: the async loop feeds each attempt's post-clamp delay forward as
/// the next attempt's `RetryState::previous_delay`. The async wiring (a pinned
/// struct field threaded through `AsyncEngine::poll_step`) is separate code
/// from the sync path, so it needs its own coverage.
#[test]
fn engine_feeds_previous_delay_forward_async() {
    struct RecordingWait {
        seen: Rc<RefCell<Vec<Option<Duration>>>>,
        delay: Duration,
    }
    impl relentless::Wait for RecordingWait {
        fn next_wait(&self, state: &relentless::RetryState) -> Duration {
            self.seen.borrow_mut().push(state.previous_delay);
            self.delay
        }
    }

    const FEEDBACK_DELAY: Duration = Duration::from_millis(7);
    let seen: Rc<RefCell<Vec<Option<Duration>>>> = Rc::new(RefCell::new(Vec::new()));

    let _: relentless::RetryResult<i32, &str> = block_on(
        RetryPolicy::new()
            .stop(stop::attempts(4))
            .wait(RecordingWait {
                seen: Rc::clone(&seen),
                delay: FEEDBACK_DELAY,
            })
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );

    // The wait strategy is consulted once per retry, not on the terminal
    // attempt (which stops before any wait) — matching the sync engine.
    assert_eq!(
        *seen.borrow(),
        vec![None, Some(FEEDBACK_DELAY), Some(FEEDBACK_DELAY)]
    );
}

#[test]
fn retry_async_executes_when_clock_is_set() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let clock = RecordingClock::new();

    let call_count = Rc::new(Cell::new(0_u32));
    let future = policy.retry_async(|_| {
        let call_count = Rc::clone(&call_count);
        call_count.set(call_count.get().saturating_add(1));
        async move {
            if call_count.get() < MAX_ATTEMPTS {
                Err(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        }
    });

    let result: relentless::RetryResult<i32, &str> = block_on(future.clock(clock.clone()).call());
    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn async_retry_type_is_nameable_from_crate_root() {
    #[allow(clippy::type_complexity, clippy::needless_pass_by_value)]
    fn assert_nameable<F, C, S, W, Cl, BA, AA, OX>(
        retry: relentless::AsyncRetry<F, C, S, W, Cl, BA, AA, OX>,
    ) {
        let _ = retry;
    }

    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .clock(RecordingClock::new());
    assert_nameable(retry);
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_retry_call_returns_an_awaitable_future() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let clock = RecordingClock::new();
    let async_retry = policy
        .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .clock(clock)
        .call();

    let result: relentless::RetryResult<i32, &str> = block_on(async_retry);
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_retry_repoll_after_completion_panics() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let mut retry = Box::pin(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    let first_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
    assert_eq!(first_poll, Poll::Ready(Ok(SUCCESS_VALUE)));

    let second_poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = Future::poll(Pin::as_mut(&mut retry), &mut cx);
    }));
    assert!(second_poll.is_err());
}

#[test]
fn retry_async_borrows_policy_immutably() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Rc::new(Cell::new(0_u32));

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| {
                let call_count = Rc::clone(&call_count);
                call_count.set(call_count.get().saturating_add(1));
                async move { Err::<i32, &str>(ERROR_VALUE) }
            })
            .clock(RecordingClock::new())
            .call(),
    );

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

#[test]
fn async_retry_returns_exhausted_on_persistent_errors() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let clock = RecordingClock::new();

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .clock(clock)
            .call(),
    );

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err(ERROR_VALUE));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn async_retry_returns_exhausted_for_ok_exhaustion() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|value: &i32| *value < 0));
    let clock = RecordingClock::new();

    let result = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(-1) })
            .clock(clock)
            .call(),
    );

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn async_composed_polling_predicate_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::result(|o: &Result<i32, &str>| {
            matches!(o, Err(e) if *e == ERROR_VALUE) || matches!(o, Ok(v) if *v < SUCCESS_VALUE)
        }));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                let current = call_count.get().saturating_add(1);
                call_count.set(current);
                async move {
                    match current {
                        1 => Err::<i32, &str>(ERROR_VALUE),
                        2 => Ok(0),
                        _ => Ok(SUCCESS_VALUE),
                    }
                }
            })
            .clock(clock)
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn async_clock_receives_wait_strategy_delays() {
    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .clock(clock.clone())
            .call(),
    );

    let calls = clock.calls();
    let expected_sleep_calls = (MAX_ATTEMPTS - 1) as usize;
    assert_eq!(calls.len(), expected_sleep_calls);
    for duration in &calls {
        assert_eq!(*duration, WAIT_DURATION);
    }
}

#[test]
fn async_predicate_is_evaluated_before_stop() {
    // The predicate is consulted before the stop strategy. With stop::attempts(1),
    // the stop would fire on the first call if checked first — but because the Ok
    // value satisfies the predicate, the result is returned without stopping.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .when(predicate::ok(|value: &i32| *value < 0));
    let clock = RecordingClock::new();

    let result = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(clock)
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_default_predicate_behaves_like_any_error() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async { Err::<i32, &str>(ERROR_VALUE) }
            })
            .clock(clock)
            .call(),
    );

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// `.timeout()` stops the async loop when elapsed time meets or exceeds the budget,
/// even if the stop strategy would allow more attempts.
#[test]
fn async_timeout_stops_loop_when_budget_exceeded() {
    // Allow up to MAX_ATTEMPTS+10 attempts but set a tight timeout so the
    // loop exits after the first attempt advances the clock past the deadline.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(ASYNC_CUSTOM_CLOCK_STEP_MILLIS));
                    Err::<i32, &str>("fail")
                }
            })
            .timeout(ASYNC_CUSTOM_CLOCK_DEADLINE)
            .clock(clock.clone())
            .call(),
    );

    // The timeout is tighter than MAX_ATTEMPTS+10 would allow, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_custom_elapsed_clock_counts_operation_runtime() {
    let policy = RetryPolicy::new().stop(stop::elapsed(ASYNC_CUSTOM_CLOCK_DEADLINE));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(ASYNC_CUSTOM_CLOCK_STEP_MILLIS));
                    Err::<i32, &str>("slow failure")
                }
            })
            .clock(clock.clone())
            .call(),
    );

    assert_eq!(call_count.get(), 1);
    assert!(clock.calls().is_empty());
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 11.1.1 — the async elapsed baseline is captured at the first poll of the
/// returned future, not when the builder is configured or `.call()` is
/// invoked. Idle time before the future is awaited must not consume the
/// elapsed budget.
#[test]
fn async_elapsed_baseline_starts_at_first_poll() {
    const IDLE_BEFORE_AWAIT_MILLIS: u64 = 1_000;
    const PER_ATTEMPT_MILLIS: u64 = 20;
    // Three whole attempt steps: 20, 40, 60 — the loop stops on the third.
    const DEADLINE: Duration = Duration::from_millis(50);

    let policy = RetryPolicy::new()
        .stop(stop::elapsed(DEADLINE))
        .wait(wait::fixed(Duration::ZERO));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let future = policy
        .retry_async(|_| {
            call_count.set(call_count.get().saturating_add(1));
            async {
                clock.advance(Duration::from_millis(PER_ATTEMPT_MILLIS));
                Err::<i32, &str>("fail")
            }
        })
        .clock(clock.clone())
        .call();
    clock.advance(Duration::from_millis(IDLE_BEFORE_AWAIT_MILLIS));
    let result = block_on(future);

    assert_eq!(call_count.get(), 3);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_elapsed_stop_triggers_after_deadline() {
    let policy = RetryPolicy::new()
        .stop(stop::elapsed(Duration::from_millis(5)))
        .wait(wait::fixed(Duration::from_millis(1)));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(10));
                    Err::<i32, &str>("would exceed budget")
                }
            })
            .clock(clock.clone())
            .call(),
    );

    assert_eq!(call_count.get(), 1);
    assert!(clock.calls().is_empty());
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_hooks_fire_in_expected_places() {
    let before_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let after_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let exit_reason = Rc::new(Cell::new(None));
    let clock = RecordingClock::new();

    let before_attempt_ref = Rc::clone(&before_attempt_calls);
    let after_attempt_ref = Rc::clone(&after_attempt_calls);
    let exit_reason_ref = Rc::clone(&exit_reason);

    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .before_attempt(move |state| {
                before_attempt_ref.borrow_mut().push(state.attempt);
            })
            .after_attempt(
                move |state: &relentless::AttemptState<'_, Result<i32, &str>>| {
                    after_attempt_ref.borrow_mut().push(state.attempt);
                },
            )
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(clock)
            .call(),
    );

    let before_attempt = before_attempt_calls.borrow();
    let after_attempt = after_attempt_calls.borrow();

    assert_eq!(*before_attempt, vec![1, 2, 3]);
    assert_eq!(*after_attempt, vec![1, 2, 3]);
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Exhausted));
}

#[test]
fn async_on_exit_reports_success_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Returned));
}

#[test]
fn async_on_exit_reports_non_retryable_error_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|err: &&str| *err == "retryable"));

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>("fatal") })
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );

    assert!(matches!(result, Err(RetryError::Aborted { .. })));
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Aborted));
}

#[test]
fn async_hooks_are_per_call_and_do_not_persist() {
    let exit_calls = Rc::new(Cell::new(0_u32));
    let exit_calls_ref = Rc::clone(&exit_calls);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .on_exit(
                move |_e: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_calls_ref.set(exit_calls_ref.get().saturating_add(1));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );
    assert_eq!(exit_calls.get(), 1);

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );
    assert_eq!(exit_calls.get(), 1);
}

#[cfg(feature = "tokio-clock")]
#[test]
fn tokio_clock_adapter_is_available() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(relentless::clock::TokioClock::new())
            .call(),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(all(feature = "embassy-clock", target_os = "none"))]
#[test]
fn embassy_clock_adapter_is_available() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(relentless::clock::EmbassyClock::new())
            .call(),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(all(feature = "gloo-timers-clock", target_arch = "wasm32"))]
#[test]
fn gloo_clock_adapter_is_available() {
    fn zero_now() -> Duration {
        Duration::ZERO
    }
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(relentless::clock::GlooClock::with_now(zero_now))
            .call(),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(feature = "futures-timer-clock")]
#[test]
fn futures_timer_clock_adapter_is_available() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .clock(relentless::clock::FuturesTimerClock::new())
            .call(),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}
