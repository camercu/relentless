//! Derive coverage that spans the strategy and value types: `Copy`, `Debug`
//! and `PartialEq` on the public wait/stop/state types.
//!
//! Each type's own behavior tests live with its concept (`error.rs`,
//! `stats.rs`, `state.rs`, `when_until.rs`, `jitter.rs`, `policy.rs`).

#[test]
fn value_types_implement_copy() {
    fn assert_copy<T: Copy>() {}

    assert_copy::<relentless::RetryState>();
    assert_copy::<relentless::RetryStats>();
    assert_copy::<relentless::StopReason>();
    assert_copy::<relentless::wait::WaitFixed>();
    assert_copy::<relentless::wait::WaitLinear>();
    assert_copy::<relentless::wait::WaitExponential>();
    assert_copy::<relentless::stop::StopAfterAttempts>();
    assert_copy::<relentless::stop::StopAfterElapsed>();
    assert_copy::<relentless::stop::StopNever>();
}

/// §14
#[test]
fn all_strategy_types_implement_debug() {
    use core::time::Duration;
    use relentless::{stop, wait};

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
}

/// §14
#[test]
fn wait_exponential_has_partial_eq_not_eq() {
    use core::time::Duration;
    use relentless::wait;

    let a = wait::exponential(Duration::from_millis(100));
    let b = wait::exponential(Duration::from_millis(100));
    assert_eq!(a, b); // PartialEq works

    // Eq is absent on WaitExponential (f64 field). WaitFixed has Eq:
    fn assert_eq_bound<T: Eq>(_: T) {}
    assert_eq_bound(wait::fixed(Duration::from_millis(10)));
    assert_eq_bound(wait::linear(
        Duration::from_millis(10),
        Duration::from_millis(5),
    ));
    // assert_eq_bound(wait::exponential(Duration::from_millis(10))); // would not compile
}
