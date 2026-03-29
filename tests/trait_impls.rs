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
