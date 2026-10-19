//! Tests for elapsed-time tracking on the asynchronous path.
//!
//! Mirrors `elapsed.rs`: the baseline starts at the first poll, and the
//! recording clock keeps its read and wait seams coherent.

mod common;

use common::{ARBITRARY_ERROR, CUSTOM_CLOCK_DEADLINE, CUSTOM_CLOCK_STEP_MILLIS};

use common::RecordingAsyncClock as RecordingClock;
use common::block_on;
use core::cell::Cell;
use core::time::Duration;
use relentless::{RetryError, RetryPolicy, stop, wait};
use std::cell::RefCell;
use std::rc::Rc;

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
            .retry_async(|_| async { Err::<i32, &str>(ARBITRARY_ERROR) })
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
fn async_custom_elapsed_clock_counts_operation_runtime() {
    let policy = RetryPolicy::new().stop(stop::elapsed(CUSTOM_CLOCK_DEADLINE));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
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
