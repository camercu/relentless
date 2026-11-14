//! Tests for elapsed-time tracking: the synchronous path at top level, the
//! asynchronous path in `async_tests`.
//!
//! Elapsed time is clock time since the execution's first attempt began: the
//! baseline starts at `.call()`, the operation and the wait strategy each see
//! their own snapshot, and a custom clock drives `stop::elapsed` without a
//! system clock.

mod common;

use common::{CUSTOM_CLOCK_DEADLINE, CUSTOM_CLOCK_STEP_MILLIS};

use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, RetryState, stop, wait};

use std::cell::RefCell;

const IDLE_BEFORE_CALL_MILLIS: u64 = 1_000;
// Three whole 20 ms attempt steps (20, 40, 60) against a 50 ms deadline, so
// the loop stops on the third attempt.
const PER_ATTEMPT_MILLIS: u64 = 20;
const BASELINE_DEADLINE: Duration = Duration::from_millis(50);

/// End-to-end: the engine feeds each attempt's post-clamp delay forward as the
/// next attempt's `RetryState::previous_delay`. The unit tests construct
/// `RetryState` by hand; this proves the sync loop actually wires it, without
/// which feedback strategies (decorrelated jitter) silently degrade.
#[test]
fn engine_feeds_previous_delay_forward_sync() {
    struct RecordingWait<'a> {
        seen: &'a RefCell<Vec<Option<Duration>>>,
        delay: Duration,
    }
    impl relentless::Wait for RecordingWait<'_> {
        fn next_wait(&self, state: &RetryState) -> Duration {
            self.seen.borrow_mut().push(state.previous_delay);
            self.delay
        }
    }

    const FEEDBACK_DELAY: Duration = Duration::from_millis(7);
    let seen: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());

    let _ = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(RecordingWait {
            seen: &seen,
            delay: FEEDBACK_DELAY,
        })
        .retry(|_| Err::<i32, &str>("fail"))
        .clock(VirtualClock::new())
        .call();

    // The wait strategy is consulted once per *retry* (not on the terminal
    // attempt, which stops before any wait). The first consultation sees no
    // previous delay; each later one sees the prior (constant) delay fed forward.
    assert_eq!(
        *seen.borrow(),
        vec![None, Some(FEEDBACK_DELAY), Some(FEEDBACK_DELAY)]
    );
}

/// The elapsed clock is read twice per attempt: once for the `RetryState` the
/// operation receives (attempt start) and again after the operation completes,
/// for the stop/wait evaluation. A clock that advances during the operation
/// must therefore show the operation an *earlier* elapsed than the wait
/// strategy sees for the same attempt. This guards the two-snapshot semantics
/// so engine refactors that thread `RetryState` as a unit cannot silently
/// collapse the pre-op and post-op snapshots into one.
#[cfg(feature = "alloc")]
#[test]
fn elapsed_is_snapshotted_separately_for_op_and_wait() {
    use std::rc::Rc;

    struct RecordingWait {
        seen: Rc<RefCell<Vec<Duration>>>,
    }
    impl relentless::Wait for RecordingWait {
        fn next_wait(&self, state: &relentless::RetryState) -> Duration {
            self.seen.borrow_mut().push(state.elapsed);
            Duration::ZERO
        }
    }

    let clock = VirtualClock::new();
    let op_elapsed: Cell<Option<Duration>> = Cell::new(None);
    let wait_elapsed: Rc<RefCell<Vec<Duration>>> = Rc::new(RefCell::new(Vec::new()));

    let _ = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(RecordingWait {
            seen: Rc::clone(&wait_elapsed),
        })
        .retry(|state: RetryState| {
            // Record the elapsed the op sees on the first attempt, then advance
            // the clock so the post-op read differs.
            if op_elapsed.get().is_none() {
                op_elapsed.set(Some(state.elapsed));
            }
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .call();

    let op_saw = op_elapsed.get().expect("op ran at least once");
    let wait_saw = wait_elapsed.borrow()[0];
    assert!(
        wait_saw > op_saw,
        "wait should see post-op elapsed ({wait_saw:?}), later than the op ({op_saw:?})"
    );
}

#[test]
fn custom_elapsed_clock_drives_elapsed_stop_without_std_clock() {
    let policy = RetryPolicy::new().stop(stop::elapsed(CUSTOM_CLOCK_DEADLINE));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("clocked failure")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 11.1.1 — the elapsed baseline is captured when execution starts (`.call()`),
/// not when the builder is configured. Idle time between configuring the
/// builder and calling it must not consume the elapsed budget.
#[test]
fn elapsed_baseline_starts_at_call_not_builder_construction() {
    let clock = VirtualClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::elapsed(BASELINE_DEADLINE))
        .wait(wait::fixed(Duration::ZERO));
    let execution = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(PER_ATTEMPT_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock);

    clock.advance(Duration::from_millis(IDLE_BEFORE_CALL_MILLIS));
    let (result, stats) = execution.call_with_stats();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 3);
}

mod async_tests {
    //! Tests for elapsed-time tracking on the asynchronous path.
    //!
    //! Mirrors the sync tests above: the baseline starts at the first poll, and the
    //! recording clock keeps its read and wait seams coherent.

    use super::common;

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
}
