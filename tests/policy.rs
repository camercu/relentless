//! Acceptance tests for the `RetryPolicy` builder: synchronous execution at top
//! level, `RetryPolicy::retry_async` in `async_tests`.
//!
//! Each test exercises a single observable contract: builder type-state transitions,
//! execution loop invariants through a policy (attempt counting, sleep timing),
//! reset across `.retry()` invocations, and the `RetryPolicy::default()`
//! safe-defaults guarantee. Classifier semantics live in `when_until.rs`, the
//! default clock in `defaults.rs`. The test harness avoids real
//! sleeps by injecting a `VirtualClock`, whose waits advance virtual time
//! instead of blocking.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_VALUE};

use common::RecordingClock;
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, Verdict};
use relentless::{stop, wait};

const DEFAULT_POLICY_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_POLICY_INITIAL_WAIT: Duration = Duration::from_millis(100);
const STORAGE_POLICY_WAIT: Duration = Duration::from_millis(1);

#[test]
fn new_policy_retries_on_any_error_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn new_policy_accepts_ok_immediately() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let result = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn new_policy_has_exponential_wait_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let recorded = clock.waits();
    assert_eq!(recorded.len(), (ARBITRARY_ATTEMPTS - 1) as usize);
    assert_eq!(recorded[0], Duration::from_millis(100));
    assert_eq!(recorded[1], Duration::from_millis(200));
}

#[test]
fn stop_builder_configures_stop_strategy() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn wait_builder_configures_wait_strategy() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(ARBITRARY_DELAY));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    assert_eq!(clock.waits(), vec![ARBITRARY_DELAY]);
}

#[test]
fn when_builder_configures_classifier() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(|e: &&str| *e == "retryable");

    // Retryable error: should retry until exhausted.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("retryable")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // Non-retryable error: should NOT retry, returns immediately.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Aborted { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Aborted with last=\"fatal\", got {other:?}"),
    }
}

#[test]
fn policy_is_easy_to_store_via_three_type_params() {
    type DbPolicy =
        RetryPolicy<stop::StopAfterAttempts, wait::WaitFixed, relentless::DefaultClassifier>;

    struct Service {
        retry: DbPolicy,
    }

    let service = Service {
        retry: RetryPolicy::new()
            .stop(stop::attempts(2))
            .wait(wait::fixed(STORAGE_POLICY_WAIT)),
    };

    let result = service
        .retry
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn retry_borrows_policy_without_mut() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(STORAGE_POLICY_WAIT));
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

#[test]
fn retry_succeeds_after_transient_failures() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < ARBITRARY_ATTEMPTS {
                Err("transient")
            } else {
                Ok(ARBITRARY_VALUE)
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn retry_returns_exhausted_when_all_attempts_fail() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let result = policy
        .retry(|_| Err::<i32, _>("always fails"))
        .clock(VirtualClock::new())
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err("always fails"));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn retry_with_never_stop_still_returns_on_ok() {
    // stop::never() will never fire, so the only exit is a successful outcome
    // returned by the classifier. Verifies the loop doesn't require a stop condition
    // to terminate when Ok is returned.
    let policy = RetryPolicy::new().stop(stop::never());

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < 3 {
                Err("transient")
            } else {
                Ok(ARBITRARY_VALUE)
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn default_policy_uses_exponential_backoff() {
    let policy = RetryPolicy::default();
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let durations = clock.waits();
    assert_eq!(durations.len(), (DEFAULT_POLICY_MAX_ATTEMPTS - 1) as usize);
    assert_eq!(durations[0], DEFAULT_POLICY_INITIAL_WAIT);
    assert_eq!(durations[1], DEFAULT_POLICY_INITIAL_WAIT.saturating_mul(2),);
}

#[test]
fn exponential_wait_increases_sleep_durations() {
    let clock = RecordingClock::new();
    let initial = Duration::from_millis(10);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(initial));

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let durations = clock.waits();
    assert_eq!(durations.len(), 3);
    assert_eq!(durations[0], Duration::from_millis(10));
    assert_eq!(durations[1], Duration::from_millis(20));
    assert_eq!(durations[2], Duration::from_millis(40));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_local_erases_policy_types() {
    // `.boxed_local()` erases stop+wait only; the classifier keeps its concrete
    // type (here the default `DefaultClassifier`, via the third defaulted param).
    let policy: RetryPolicy<
        Box<dyn relentless::stop::Stop + 'static>,
        Box<dyn relentless::wait::Wait + 'static>,
    > = RetryPolicy::new().boxed_local();
    let result = policy
        .retry(|_| Err::<(), _>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(result.is_err());
}

/// A boxed default-classifier policy is reusable across operations with
/// different `Ok` types, because only stop and wait are boxed: the
/// classifier stays generic, not pinned to one outcome type.
#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_reuses_across_different_ok_types() {
    let policy: RetryPolicy<Box<dyn relentless::Stop + Send>, Box<dyn relentless::Wait + Send>> =
        RetryPolicy::new()
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .boxed();

    let s = policy
        .retry(|_| Ok::<String, &str>("v".to_string()))
        .clock(VirtualClock::new())
        .call();
    let u = policy
        .retry(|_| Ok::<(), &str>(()))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(s.unwrap(), "v");
    assert_eq!(u.unwrap(), ());
}

/// `boxed_local` erases types but preserves retry behavior: the loop still
/// runs the configured number of attempts and returns `Exhausted` on failure.
#[cfg(feature = "alloc")]
#[test]
fn boxed_local_runs_full_retry_cycle() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        call_count.get(),
        ARBITRARY_ATTEMPTS,
        "should run ARBITRARY_ATTEMPTS times"
    );
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// `boxed_local` policy is not `Send` — the trait objects it contains lack the
/// `Send` bound. Verify this at compile time via a `compile_fail` doc test on
/// the method itself; here we verify the positive case: it IS usable on a
/// single thread without any `Send` requirement.
#[cfg(feature = "alloc")]
#[test]
fn boxed_local_is_usable_without_send_bound() {
    // Rc is !Send — if boxed_local required Send, this would fail to compile.
    let counter = std::rc::Rc::new(Cell::new(0_u32));
    let counter_clone = counter.clone();

    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local();

    let result = policy
        .retry(move |_| {
            counter_clone.set(counter_clone.get().saturating_add(1));
            Err::<(), _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(counter.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_erases_strategy_types() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn policy_resets_between_retry_invocations() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|_| Err::<i32, _>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // A second `.retry()` call on the same policy must start from attempt 1.
    // If internal state leaked across calls, the counter would already be at
    // ARBITRARY_ATTEMPTS and op would never be invoked.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail again")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn retry_with_single_attempt_calls_op_once() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), 1);
}

#[test]
fn retry_succeeds_on_first_attempt() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Ok::<_, &str>(ARBITRARY_VALUE)
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 1);
}

#[test]
fn composed_polling_classifier_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .decide(|o: Result<i32, &'static str>| match o {
            Err("transient") => Verdict::Retry(Err("transient")),
            Ok(v) if v < ARBITRARY_VALUE => Verdict::Retry(Ok(v)),
            Ok(v) => Verdict::Return(v),
            Err(e) => Verdict::Abort(e),
        });

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let next_call = call_count.get().saturating_add(1);
            call_count.set(next_call);
            match next_call {
                1 => Err("transient"),
                2 => Ok(0),
                _ => Ok(ARBITRARY_VALUE),
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 3);
}

/// §6
#[test]
fn policy_retry_borrows_self_immutably() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::ZERO));

    let r1 = policy
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    let r2 = policy
        .retry(|_| Ok::<i32, &str>(2))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(r1, Ok(1));
    assert_eq!(r2, Ok(2));
}

/// §6
#[test]
fn shared_policy_reference_across_multiple_threads() {
    use std::sync::Arc;
    use std::thread;

    let policy = Arc::new(
        RetryPolicy::new()
            .stop(stop::attempts(2))
            .wait(wait::fixed(Duration::ZERO)),
    );

    let p1 = Arc::clone(&policy);
    let p2 = Arc::clone(&policy);

    let t1 = thread::spawn(move || {
        p1.retry(|_| Ok::<i32, &str>(1))
            .clock(VirtualClock::new())
            .call()
    });
    let t2 = thread::spawn(move || {
        p2.retry(|_| Ok::<i32, &str>(2))
            .clock(VirtualClock::new())
            .call()
    });

    assert_eq!(t1.join().unwrap(), Ok(1));
    assert_eq!(t2.join().unwrap(), Ok(2));
}

/// 6.4.2
#[test]
fn call_returns_retry_result_type() {
    let result: relentless::RetryResult<i32, &str> = RetryPolicy::new()
        .stop(stop::attempts(1))
        .retry(|_| Err::<i32, &str>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 5.1, 5.2
#[test]
fn new_and_default_produce_same_policy() {
    // Both should retry 3 times on persistent errors. The unparameterized
    // `RetryPolicy` annotation also pins the default type parameters.
    let call_count_new = Cell::new(0_u32);
    let r_new = RetryPolicy::new()
        .retry(|_| {
            call_count_new.set(call_count_new.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    let call_count_def = Cell::new(0_u32);
    let default_policy: RetryPolicy = RetryPolicy::default();
    let r_def = default_policy
        .retry(|_| {
            call_count_def.set(call_count_def.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(r_new, Err(RetryError::Exhausted { .. })));
    assert!(matches!(r_def, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count_new.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
    assert_eq!(call_count_def.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
}

/// 5.3
#[test]
fn builder_methods_return_typed_policy() {
    // If builder methods consumed/mutated rather than returning new types, the
    // chained call would fail to compile; executing it pins that the fully
    // chained policy actually runs.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .when(|_: &&str| true)
        .until(|_: &i32| true);

    let result = policy
        .retry(|_| Ok::<i32, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn default_retry_policy_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<relentless::RetryPolicy>();
}

/// 5.7
#[test]
fn retry_policy_is_clone_when_components_are_clone() {
    use core::time::Duration;
    use relentless::{RetryPolicy, stop, wait};

    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(10)));

    let cloned = policy.clone();

    // Both clones produce the same results when used independently.
    let result1 = policy
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    let result2 = cloned
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(result1, result2);
}

mod async_tests {
    //! Acceptance tests for the async execution path via `RetryPolicy::retry_async`.
    //!
    //! All tests run under a minimal in-process `block_on` executor so they are
    //! deterministic and executor-agnostic: no Tokio runtime, no real timers. The
    //! `RecordingClock` captures requested waits (advancing its own `now`, so the
    //! elapsed seam stays coherent) without blocking, letting tests verify
    //! wait-strategy output without wall-clock delays.

    use super::common;

    use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_ERROR, ARBITRARY_VALUE};

    use common::RecordingAsyncClock as RecordingClock;
    use common::block_on;
    use core::cell::Cell;
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use core::time::Duration;
    use relentless::{RetryError, RetryPolicy};
    use relentless::{Verdict, stop, wait};
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

        let result: relentless::RetryResult<i32, &str> =
            block_on(future.clock(clock.clone()).call());
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
            .until(|value: &i32| *value >= 0);
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
    fn async_composed_polling_classifier_handles_transient_errors_and_not_ready_values() {
        let policy = RetryPolicy::new()
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .decide(|o: Result<i32, &'static str>| match o {
                Err(e) if e == ARBITRARY_ERROR => Verdict::Retry(Err(e)),
                Ok(v) if v < ARBITRARY_VALUE => Verdict::Retry(Ok(v)),
                Ok(v) => Verdict::Return(v),
                Err(e) => Verdict::Abort(e),
            });
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
    fn async_classifier_is_evaluated_before_stop() {
        // The classifier is consulted before the stop strategy. With stop::attempts(1),
        // the stop would fire on the first call if checked first — but because the Ok
        // value satisfies `.until`, the result is returned without stopping.
        let policy = RetryPolicy::new()
            .stop(stop::attempts(1))
            .until(|value: &i32| *value >= 0);
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
    fn async_default_classifier_behaves_like_any_error() {
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
}
