//! Acceptance tests for the async execution path via `RetryPolicy::retry_async`.
//!
//! All tests run under a minimal in-process `block_on` executor so they are
//! deterministic and executor-agnostic: no Tokio runtime, no real timers. The
//! `RecordingClock` captures requested waits (advancing its own `now`, so the
//! elapsed seam stays coherent) without blocking, letting tests verify
//! wait-strategy output without wall-clock delays.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_ERROR, ARBITRARY_VALUE};

use common::RecordingAsyncClock as RecordingClock;
use common::block_on;
use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use relentless::{RetryError, RetryPolicy};
use relentless::{predicate, stop, wait};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Helpers

#[test]
fn retry_async_executes_when_clock_is_set() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));
    let clock = RecordingClock::new();

    let call_count = Rc::new(Cell::new(0_u32));
    let future = policy.retry_async(|_| {
        let call_count = Rc::clone(&call_count);
        call_count.set(call_count.get().saturating_add(1));
        async move {
            if call_count.get() < ARBITRARY_ATTEMPTS {
                Err(ARBITRARY_ERROR)
            } else {
                Ok(ARBITRARY_VALUE)
            }
        }
    });

    let result: relentless::RetryResult<i32, &str> = block_on(future.clock(clock.clone()).call());
    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn async_retry_call_returns_an_awaitable_future() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let clock = RecordingClock::new();
    let async_retry = policy
        .retry_async(|_| async { Ok::<i32, &str>(ARBITRARY_VALUE) })
        .clock(clock)
        .call();

    let result: relentless::RetryResult<i32, &str> = block_on(async_retry);
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn async_retry_repoll_after_completion_panics() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let mut retry = Box::pin(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(ARBITRARY_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );
    let mut cx = Context::from_waker(Waker::noop());

    let first_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
    assert_eq!(first_poll, Poll::Ready(Ok(ARBITRARY_VALUE)));

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
                async move { Err::<i32, &str>(ARBITRARY_ERROR) }
            })
            .clock(RecordingClock::new())
            .call(),
    );

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

#[test]
fn async_retry_returns_exhausted_on_persistent_errors() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));
    let clock = RecordingClock::new();

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ARBITRARY_ERROR) })
            .clock(clock)
            .call(),
    );

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err(ARBITRARY_ERROR));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn async_retry_returns_exhausted_for_ok_exhaustion() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
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
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::result(|o: &Result<i32, &str>| {
            matches!(o, Err(e) if *e == ARBITRARY_ERROR)
                || matches!(o, Ok(v) if *v < ARBITRARY_VALUE)
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
                        1 => Err::<i32, &str>(ARBITRARY_ERROR),
                        2 => Ok(0),
                        _ => Ok(ARBITRARY_VALUE),
                    }
                }
            })
            .clock(clock)
            .call(),
    );

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn async_clock_receives_wait_strategy_delays() {
    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY));

    let _result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ARBITRARY_ERROR) })
            .clock(clock.clone())
            .call(),
    );

    let calls = clock.calls();
    let expected_sleep_calls = (ARBITRARY_ATTEMPTS - 1) as usize;
    assert_eq!(calls.len(), expected_sleep_calls);
    for duration in &calls {
        assert_eq!(*duration, ARBITRARY_DELAY);
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
            .retry_async(|_| async { Ok::<i32, &str>(ARBITRARY_VALUE) })
            .clock(clock)
            .call(),
    );

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn async_default_predicate_behaves_like_any_error() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async { Err::<i32, &str>(ARBITRARY_ERROR) }
            })
            .clock(clock)
            .call(),
    );

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A waker that counts wake-ups, so a cooperative yield can be checked for the
/// thing that makes it safe: the task is rescheduled, not stalled.
fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    struct CountingWake(Arc<AtomicUsize>);
    impl std::task::Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let count = Arc::new(AtomicUsize::new(0));
    (
        Waker::from(Arc::new(CountingWake(Arc::clone(&count)))),
        count,
    )
}

/// GIVEN an async retry whose strategy returns a zero delay
/// WHEN the future is polled
/// THEN it yields after each attempt — `Poll::Pending` plus a wake-up —
///      rather than running the whole loop inside one `poll`.
///
/// Skipping the sleep must not mean skipping the *yield*: a loop that never
/// returns `Pending` owns its executor thread until it finishes, so timers
/// cannot fire and `select!`, `timeout` and cancellation are all defeated —
/// on a current-thread runtime a co-tenant task never runs again. The wake-up
/// is what makes the yield cooperative rather than a stall.
#[test]
fn async_zero_delay_yields_between_attempts() {
    let (waker, wake_count) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let mut future = Box::pin(
        policy
            .retry_async(move |_| {
                attempts_ref.set(attempts_ref.get() + 1);
                async { Err::<i32, &str>(ARBITRARY_ERROR) }
            })
            .clock(RecordingClock::new())
            .call(),
    );

    assert!(
        future.as_mut().poll(&mut cx).is_pending(),
        "a zero delay must yield, not run the next attempt in the same poll"
    );
    assert_eq!(attempts.get(), 1, "exactly one attempt per poll");
    assert_eq!(
        wake_count.load(Ordering::Relaxed),
        1,
        "the yield must reschedule the task, or it stalls forever"
    );

    // Driven to completion, the outcome is unchanged: still one attempt per
    // poll, still exhausted after the budget.
    let result = loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            break result;
        }
    };
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}
