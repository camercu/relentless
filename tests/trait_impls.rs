//! Acceptance tests for standard trait implementations.
//!
//! These tests verify:
//! - Default RetryPolicy is Send + Sync
//! - Public value types implement Copy

fn _assert_send_sync<T: Send + Sync>() {}

#[test]
fn default_retry_policy_is_send_and_sync() {
    _assert_send_sync::<tenacious::RetryPolicy>();
}

#[test]
fn value_types_implement_copy() {
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

/// §14
#[test]
fn all_strategy_types_implement_debug() {
    use core::time::Duration;
    use tenacious::{predicate, stop, wait};

    let _ = format!("{:?}", stop::attempts(3));
    let _ = format!("{:?}", stop::elapsed(Duration::from_secs(10)));
    let _ = format!("{:?}", stop::never());
    let _ = format!("{:?}", stop::attempts(3) | stop::never());
    let _ = format!("{:?}", stop::attempts(3) & stop::never());

    let _ = format!("{:?}", wait::fixed(Duration::from_millis(10)));
    let _ = format!(
        "{:?}",
        wait::linear(Duration::from_millis(10), Duration::from_millis(5))
    );
    let _ = format!("{:?}", wait::exponential(Duration::from_millis(10)));

    // Predicate concrete types implement Debug. Closures inside don't derive Debug,
    // but the wrapper structs do. Verify the named container types are Debug:
    fn assert_debug<T: core::fmt::Debug>(_: &T) {}
    assert_debug(&predicate::any_error());
    assert_debug(&(predicate::any_error() | predicate::any_error()));
    assert_debug(&(predicate::any_error() & predicate::any_error()));
    let _ = format!("{:?}", predicate::any_error());
}

/// §14
#[test]
fn wait_exponential_has_partial_eq_not_eq() {
    use core::time::Duration;
    use tenacious::wait;

    let a = wait::exponential(Duration::from_millis(100));
    let b = wait::exponential(Duration::from_millis(100));
    assert_eq!(a, b); // PartialEq works

    // Eq is absent on WaitExponential (f64 field). WaitFixed has Eq:
    fn _assert_eq<T: Eq>(_: T) {}
    _assert_eq(wait::fixed(Duration::from_millis(10)));
    _assert_eq(wait::linear(
        Duration::from_millis(10),
        Duration::from_millis(5),
    ));
    // _assert_eq(wait::exponential(Duration::from_millis(10))); // would not compile
}

/// 4.2.3
#[test]
fn stop_reason_display_values() {
    use tenacious::StopReason;

    assert_eq!(format!("{}", StopReason::Accepted), "accepted");
    assert_eq!(format!("{}", StopReason::Exhausted), "retries exhausted");
}

/// 4.1.9
#[test]
fn retry_error_display_format() {
    use tenacious::RetryError;

    let e: RetryError<(), String> = RetryError::Exhausted {
        last: Err("network timeout".to_string()),
    };
    assert_eq!(format!("{}", e), "retries exhausted: network timeout");

    let r: RetryError<i32, String> = RetryError::Rejected {
        last: "fatal".to_string(),
    };
    assert_eq!(format!("{}", r), "rejected: fatal");
}

/// §14
#[test]
fn retry_stats_is_clone_and_copy() {
    use core::time::Duration;
    use tenacious::{RetryStats, StopReason};

    let stats = RetryStats {
        attempts: 2,
        total_elapsed: Some(Duration::from_secs(1)),
        total_wait: Duration::from_millis(100),
        stop_reason: StopReason::Exhausted,
    };

    let cloned = stats.clone();
    let copied = stats; // Copy — no move
    assert_eq!(stats.attempts, cloned.attempts);
    assert_eq!(stats.attempts, copied.attempts);
}

/// §14
#[test]
fn retry_state_is_clone_copy_partial_eq() {
    use core::time::Duration;
    use tenacious::RetryState;

    let s = RetryState::new(3, Some(Duration::from_secs(1)));
    let cloned = s.clone();
    let copied = s; // Copy
    assert_eq!(s, cloned);
    assert_eq!(s, copied);
    assert_ne!(s, RetryState::new(4, None));
}

/// 5.7
#[test]
fn retry_policy_is_clone_when_components_are_clone() {
    use core::time::Duration;
    use tenacious::{RetryPolicy, stop, wait};

    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(10)));

    let cloned = policy.clone();

    // Both clones produce the same results when used independently.
    let result1 = policy.retry(|_| Ok::<i32, &str>(1)).sleep(|_| {}).call();
    let result2 = cloned.retry(|_| Ok::<i32, &str>(1)).sleep(|_| {}).call();
    assert_eq!(result1, result2);
}

/// §14
#[test]
fn all_predicate_types_implement_clone() {
    use tenacious::predicate;

    // PredicateAnyError is Clone
    let a = predicate::any_error().clone();
    // PredicateError wrapping a Clone closure is Clone
    let _ = predicate::error(|_e: &&str| true).clone();
    // PredicateOk wrapping a Clone closure is Clone
    let _ = predicate::ok(|_v: &u32| true).clone();
    // PredicateResult wrapping a Clone closure is Clone
    let _ = predicate::result(|_r: &Result<u32, &str>| true).clone();
    // PredicateUntil is Clone
    let _ = predicate::until(predicate::any_error()).clone();
    // PredicateAny and PredicateAll are Clone
    let _ = (predicate::any_error() | predicate::any_error()).clone();
    let _ = (predicate::any_error() & predicate::any_error()).clone();
    drop(a);
}
