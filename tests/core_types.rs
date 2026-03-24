//! Acceptance tests for Core Types and Traits (Spec items 1.1–1.11)
//!
//! These tests verify:
//! - Stop trait definition and semantics (1.2)
//! - Wait trait definition and semantics (1.3)
//! - Predicate trait definition and semantics (1.4)
//! - Sleeper trait and blanket impl (1.5, 1.6)
//! - RetryState, AttemptState, and ExitState structs
//! - RetryError enum and Display/Error impls (1.9, 1.10)
//! - Duration is core::time::Duration (1.11)

use core::time::Duration;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tenacious::Predicate;
use tenacious::Sleeper;
use tenacious::Stop;
use tenacious::Wait;

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// The maximum attempts threshold used in stop-strategy tests.
const STOP_AFTER_MAX_ATTEMPTS: u32 = 3;

/// Values that are genuinely arbitrary — any valid value would work.
/// These signal "the specific value doesn't matter" to the reader.
const ARBITRARY_DURATION: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------------------
// 1.2: Stop trait — should_stop method (&self, no reset)
// ---------------------------------------------------------------------------

/// A trivial Stop implementation that stops after a fixed number of attempts.
struct StopAfter {
    max: u32,
}

impl Stop for StopAfter {
    fn should_stop(&self, state: &tenacious::RetryState) -> bool {
        state.attempt >= self.max
    }
}

#[test]
fn stop_trait_should_stop_returns_bool() {
    let stop = StopAfter {
        max: STOP_AFTER_MAX_ATTEMPTS,
    };

    let state = make_retry_state(1);
    assert!(
        !stop.should_stop(&state),
        "attempt 1 < max, should not stop"
    );

    let state = make_retry_state(STOP_AFTER_MAX_ATTEMPTS);
    assert!(stop.should_stop(&state), "attempt == max, should stop");
}

// ---------------------------------------------------------------------------
// 1.3: Wait trait — next_wait method (&self, no reset)
// ---------------------------------------------------------------------------

/// A trivial Wait implementation that returns a fixed duration.
struct FixedWait {
    dur: Duration,
}

impl Wait for FixedWait {
    fn next_wait(&self, _state: &tenacious::RetryState) -> Duration {
        self.dur
    }
}

#[test]
fn wait_trait_next_wait_returns_duration() {
    let wait = FixedWait {
        dur: ARBITRARY_DURATION,
    };
    let state = make_retry_state(1);
    assert_eq!(wait.next_wait(&state), ARBITRARY_DURATION);
}

// ---------------------------------------------------------------------------
// 1.2/1.3: Stop and Wait are non-generic (decoupled from T, E)
// ---------------------------------------------------------------------------

/// Stop and Wait accept RetryState (non-generic), so a single strategy
/// works across operations with different Result types.
#[test]
fn stop_and_wait_are_not_generic_over_result_type() {
    let stop = StopAfter {
        max: STOP_AFTER_MAX_ATTEMPTS,
    };
    let wait = FixedWait {
        dur: ARBITRARY_DURATION,
    };
    let state = make_retry_state(1);

    // The same stop and wait instances work regardless of operation type —
    // no <T, E> parameterization needed. This is a compile-time check.
    assert!(!stop.should_stop(&state));
    assert_eq!(wait.next_wait(&state), ARBITRARY_DURATION);
}

// ---------------------------------------------------------------------------
// 1.4: Predicate<T, E> trait — should_retry method (&self)
// ---------------------------------------------------------------------------

/// A predicate that retries on any error.
struct RetryOnAnyError;

impl Predicate<String, std::io::Error> for RetryOnAnyError {
    fn should_retry(&self, outcome: &Result<String, std::io::Error>) -> bool {
        outcome.is_err()
    }
}

#[test]
fn predicate_trait_should_retry_on_error() {
    let pred = RetryOnAnyError;
    let err_result: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    assert!(pred.should_retry(&err_result));
}

#[test]
fn predicate_trait_should_not_retry_on_ok() {
    let pred = RetryOnAnyError;
    let ok_result: Result<String, std::io::Error> = Ok("success".to_string());
    assert!(!pred.should_retry(&ok_result));
}

/// Verify that T and E are type parameters on the trait (not the method).
/// Two different Predicate impls for different (T, E) pairs on the same struct.
struct AlwaysRetry;

impl Predicate<u32, String> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<u32, String>) -> bool {
        true
    }
}

impl Predicate<bool, i32> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<bool, i32>) -> bool {
        true
    }
}

#[test]
fn predicate_trait_type_params_on_trait() {
    let pred = AlwaysRetry;
    let r1: Result<u32, String> = Ok(42);
    let r2: Result<bool, i32> = Err(-1);

    // Both should compile and work — T, E are on the trait, not the method.
    assert!(<AlwaysRetry as Predicate<u32, String>>::should_retry(
        &pred, &r1
    ));
    assert!(<AlwaysRetry as Predicate<bool, i32>>::should_retry(
        &pred, &r2
    ));
}

/// 4.8: Predicate is blanket-implemented for Fn(&Result<T, E>) -> bool.
#[test]
fn predicate_blanket_impl_for_closure() {
    let pred = |outcome: &Result<i32, &str>| outcome.is_err();

    let err: Result<i32, &str> = Err("fail");
    let ok: Result<i32, &str> = Ok(42);

    assert!(Predicate::should_retry(&pred, &err));
    assert!(!Predicate::should_retry(&pred, &ok));
}

/// Predicate::should_retry takes &self. Verify it can be called
/// multiple times through a shared reference.
#[test]
fn predicate_is_callable_multiple_times() {
    let pred = RetryOnAnyError;

    let err: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    let ok: Result<String, std::io::Error> = Ok("ok".to_string());

    // Multiple calls through a shared reference must work.
    assert!(pred.should_retry(&err));
    assert!(pred.should_retry(&Err(std::io::Error::other("boom2"))));
    assert!(!pred.should_retry(&ok));
}

// ---------------------------------------------------------------------------
// 1.5, 1.6: Sleeper trait and blanket impl for Fn(Duration) -> Future
// ---------------------------------------------------------------------------

/// A minimal future that resolves immediately.
struct Immediate;

impl Future for Immediate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
}

/// Verify Sleeper trait has the right shape: associated type Sleep and sleep method.
struct ImmediateSleeper;

impl Sleeper for ImmediateSleeper {
    type Sleep = Immediate;
    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        Immediate
    }
}

/// Creates a no-op waker for polling futures in unit tests.
fn noop_waker() -> std::task::Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    std::task::Waker::from(Arc::new(NoopWake))
}

#[test]
fn sleeper_trait_direct_impl() {
    let sleeper = ImmediateSleeper;
    let mut fut = sleeper.sleep(ARBITRARY_DURATION);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

/// 1.6: Blanket impl — a closure `Fn(Duration) -> Fut` satisfies Sleeper.
#[test]
fn sleeper_blanket_impl_for_closure() {
    let sleeper_fn = |_dur: Duration| Immediate;
    let mut fut = Sleeper::sleep(&sleeper_fn, ARBITRARY_DURATION);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

/// Verify the blanket impl works with a closure returning a different future type.
struct DelayedReady(bool);

impl Future for DelayedReady {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[test]
fn sleeper_blanket_impl_different_future_type() {
    let sleeper_fn = |_dur: Duration| DelayedReady(false);
    let mut fut = Sleeper::sleep(&sleeper_fn, Duration::ZERO);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Pending));
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

// ---------------------------------------------------------------------------
// 1.7, 1.8: RetryState, AttemptState, and ExitState structs
// ---------------------------------------------------------------------------

#[test]
fn retry_state_has_required_fields() {
    let elapsed = Duration::from_secs(5);

    let state = tenacious::RetryState::new(1, Some(elapsed));

    assert_eq!(state.attempt, 1);
    assert_eq!(state.elapsed, Some(elapsed));
}

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = make_retry_state(1);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
}

#[test]
fn retry_state_elapsed_can_be_none() {
    let state = tenacious::RetryState::new(1, None);
    assert_eq!(state.elapsed, None);
}

#[test]
fn attempt_state_has_flat_fields_and_outcome() {
    let outcome: Result<i32, String> = Ok(42);

    let state = tenacious::AttemptState::new(
        1,
        Some(Duration::ZERO),
        &outcome,
        Some(Duration::from_millis(100)),
    );

    assert_eq!(state.attempt, 1);
    assert_eq!(*state.outcome, Ok(42));
    assert_eq!(state.next_delay, Some(Duration::from_millis(100)));
}

#[test]
fn attempt_state_with_err_outcome() {
    let outcome: Result<(), String> = Err("network timeout".to_string());

    let state =
        tenacious::AttemptState::new(1, Some(Duration::ZERO), &outcome, Some(Duration::ZERO));

    assert!(state.outcome.is_err());
    assert_eq!(state.outcome.as_ref().unwrap_err(), "network timeout");
}

#[test]
fn exit_state_has_required_fields() {
    let outcome = Err::<i32, &str>("fatal");
    let state = tenacious::ExitState::new(2, None, &outcome, tenacious::StopReason::Exhausted);

    assert_eq!(state.attempt, 2);
    assert!(state.outcome.is_err());
    assert_eq!(state.elapsed, None);
    assert_eq!(state.stop_reason, tenacious::StopReason::Exhausted);
}

// ---------------------------------------------------------------------------
// 1.9, 1.10: RetryError enum
// ---------------------------------------------------------------------------

#[test]
fn retry_error_exhausted_variant() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("connection refused".to_string()),
    };

    match err {
        tenacious::RetryError::Exhausted { ref last } => {
            assert_eq!(last, &Err("connection refused".to_string()));
        }
        _ => panic!("expected Exhausted variant"),
    }
}

#[test]
fn retry_error_rejected_variant() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };

    match err {
        tenacious::RetryError::Rejected { ref last } => {
            assert_eq!(last, "fatal");
        }
        _ => panic!("expected Rejected variant"),
    }
}

#[test]
fn retry_error_exhausted_with_ok_last() {
    let err: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted { last: Ok(42) };

    if let tenacious::RetryError::Exhausted { last } = err {
        assert_eq!(last, Ok(42));
    }
}

/// 1.10: RetryError implements Display unconditionally.
#[test]
fn retry_error_display_includes_meaningful_content() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };

    let msg = format!("{}", err);
    assert!(
        msg.contains("timeout"),
        "Display should include the error message: {msg}"
    );

    let err2: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    let msg2 = format!("{}", err2);
    assert!(
        msg2.contains("fatal"),
        "Display should include the error message: {msg2}"
    );
}

/// 1.10: RetryError implements std::error::Error when std is active and E: Error + 'static.
#[test]
#[cfg(feature = "std")]
fn retry_error_implements_std_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Exhausted { last: Err(inner) };

    // Verify it can be used as a dyn Error.
    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Exhausted should chain to the inner error via source()"
    );
}

/// source() returns None for Exhausted with Ok.
#[test]
#[cfg(feature = "std")]
fn retry_error_exhausted_ok_source_is_none() {
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Exhausted { last: Ok(()) };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_none(),
        "Exhausted with Ok has no source error"
    );
}

/// source() returns Some(inner) for Rejected.
#[test]
#[cfg(feature = "std")]
fn retry_error_rejected_source_is_inner_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fatal");
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Rejected { last: inner };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Rejected should chain to the inner error via source()"
    );
}

/// RetryError derives Clone and PartialEq for ergonomic test assertions.
#[test]
fn retry_error_derives_clone_and_partial_eq() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("fail".to_string()),
    };

    let cloned = err.clone();
    assert_eq!(err, cloned);
}

#[test]
fn retry_error_accessors_expose_last_outcome_and_error() {
    let exhausted: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };
    let expected_error = "timeout".to_string();
    assert_eq!(exhausted.last(), Some(&Err(expected_error.clone())));
    assert_eq!(exhausted.last_error(), Some(&expected_error));

    let rejected: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    // Rejected has no full last() (only the error), so last() returns None
    assert_eq!(rejected.last(), None);
    assert_eq!(rejected.last_error(), Some(&"fatal".to_string()));
}

#[test]
fn retry_error_into_accessors_extract_owned_values() {
    let exhausted: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };
    assert_eq!(exhausted.into_last(), Some(Err("timeout".to_string())));

    let rejected: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    assert_eq!(rejected.into_last_error(), Some("fatal".to_string()));
}

#[test]
fn public_value_types_derive_common_traits() {
    fn assert_copy<T: Copy>() {}

    assert_copy::<tenacious::RetryState>();
    assert_copy::<tenacious::RetryStats>();
    assert_copy::<tenacious::StopReason>();
    assert_copy::<tenacious::wait::WaitFixed>();
    assert_copy::<tenacious::wait::WaitLinear>();
    assert_copy::<tenacious::wait::WaitExponential>();
    assert_copy::<tenacious::stop::StopAfterAttempts>();
    assert_copy::<tenacious::stop::StopAfterElapsed>();
    assert_copy::<tenacious::stop::StopNever>();
}

/// Verify RetryError can be used in a Result context (ergonomics).
#[test]
fn retry_error_in_result_context() {
    fn fallible() -> Result<i32, tenacious::RetryError<(), String>> {
        Err(tenacious::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    assert!(fallible().is_err());
}

/// Verify RetryResult aliases Result<T, RetryError<T, E>>.
#[test]
fn retry_result_alias_matches_retry_error_shape() {
    fn fallible() -> tenacious::RetryResult<i32, String> {
        Err(tenacious::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    let result: Result<i32, tenacious::RetryError<i32, String>> = fallible();
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// 1.11: Duration is core::time::Duration
// ---------------------------------------------------------------------------

#[test]
fn duration_is_core_time_duration() {
    // RetryState uses Duration — verify it's the standard core::time::Duration.
    let d: core::time::Duration = ARBITRARY_DURATION;
    let state = tenacious::RetryState::new(1, Some(d));
    assert_eq!(state.elapsed, Some(ARBITRARY_DURATION));
}

// ---------------------------------------------------------------------------
// Thread safety: default policy is Send + Sync
// ---------------------------------------------------------------------------

fn _assert_send_sync<T: Send + Sync>() {}

#[test]
fn default_retry_policy_is_send_and_sync() {
    _assert_send_sync::<tenacious::RetryPolicy>();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Helper to construct a RetryState with default zero/none fields.
///
/// NOTE: The spec says state types are "never constructed by user code" (1.8).
/// These helpers simulate what the execution engine would do.
fn make_retry_state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, None)
}
