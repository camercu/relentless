//! Acceptance tests for Wait Strategies (Spec items 3.1–3.11)
//!
//! These tests verify:
//! - wait::fixed(dur) always returns dur (3.1)
//! - wait::linear(initial, increment) computes initial + (n-1)*increment (3.2)
//! - wait::exponential(initial) computes initial * 2^(n-1) (3.3)
//! - exponential .base(f) changes the multiplier (3.4)
//! - .cap(max) clamps computed wait (3.5)
//! - Add composition produces WaitCombine (3.7)
//! - .chain(other, after) produces WaitChain (3.8)
//! - Reset propagation on composites (3.9)
//! - Clone and Debug on all types (3.10)
//! - Wait strategies return Duration, don't interact with sleep (3.11)

use core::time::Duration;
use tenacious::Wait;
use tenacious::wait;

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Base duration used across most wait strategy tests.
const BASE: Duration = Duration::from_millis(100);

/// Increment for linear backoff tests.
const INCREMENT: Duration = Duration::from_millis(50);

/// Cap duration for testing .cap() builder.
const CAP: Duration = Duration::from_millis(500);

/// Number of attempts after which a WaitChain switches strategies.
const CHAIN_AFTER: u32 = 3;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, None)
}

// ---------------------------------------------------------------------------
// 3.1: wait::fixed(dur)
// ---------------------------------------------------------------------------

#[test]
fn fixed_always_returns_same_duration() {
    let w = wait::fixed(BASE);
    for attempt in 1..=10 {
        let state = make_state(attempt);
        assert_eq!(w.next_wait(&state), BASE, "attempt {attempt}");
    }
}

#[test]
fn fixed_returns_zero_for_zero_duration() {
    let w = wait::fixed(Duration::ZERO);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::ZERO);
}

// ---------------------------------------------------------------------------
// 3.2: wait::linear(initial, increment)
// ---------------------------------------------------------------------------

#[test]
fn linear_first_attempt_returns_initial() {
    let w = wait::linear(BASE, INCREMENT);
    let state = make_state(1);
    // initial + (1-1)*increment = initial
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn linear_subsequent_attempts_increase() {
    let w = wait::linear(BASE, INCREMENT);

    let state = make_state(2);
    // 100ms + (2-1)*50ms = 150ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));

    let state = make_state(3);
    // 100ms + (3-1)*50ms = 200ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(200));

    let state = make_state(4);
    // 100ms + (4-1)*50ms = 250ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(250));
}

#[test]
fn linear_saturates_on_overflow() {
    let w = wait::linear(Duration::MAX, INCREMENT);
    let state = make_state(2);
    // Duration::MAX + 50ms should saturate at Duration::MAX
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn linear_with_zero_increment_is_fixed() {
    let w = wait::linear(BASE, Duration::ZERO);
    for attempt in 1..=5 {
        let state = make_state(attempt);
        assert_eq!(w.next_wait(&state), BASE, "attempt {attempt}");
    }
}

// ---------------------------------------------------------------------------
// 3.3: wait::exponential(initial)
// ---------------------------------------------------------------------------

#[test]
fn exponential_first_attempt_returns_initial() {
    let w = wait::exponential(BASE);
    let state = make_state(1);
    // initial * 2^0 = initial
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn exponential_doubles_each_attempt() {
    let w = wait::exponential(BASE);

    let state = make_state(2);
    // 100ms * 2^1 = 200ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(200));

    let state = make_state(3);
    // 100ms * 2^2 = 400ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(400));

    let state = make_state(4);
    // 100ms * 2^3 = 800ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(800));
}

#[test]
fn exponential_saturates_on_overflow() {
    let w = wait::exponential(BASE);
    // 2^(u32::MAX-1) will overflow; should saturate.
    let state = make_state(u32::MAX);
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

// ---------------------------------------------------------------------------
// 3.4: exponential .base(f64)
// ---------------------------------------------------------------------------

#[test]
fn exponential_with_base_3() {
    let base_multiplier = 3.0;
    let w = wait::exponential(BASE).base(base_multiplier);

    let state = make_state(1);
    // 100ms * 3^0 = 100ms
    assert_eq!(w.next_wait(&state), BASE);

    let state = make_state(2);
    // 100ms * 3^1 = 300ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(300));

    let state = make_state(3);
    // 100ms * 3^2 = 900ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(900));
}

#[test]
fn exponential_base_below_1_clamped_to_1() {
    let w = wait::exponential(BASE).base(0.5);
    // base clamped to 1.0, so every attempt returns initial.
    for attempt in 1..=5 {
        let state = make_state(attempt);
        assert_eq!(
            w.next_wait(&state),
            BASE,
            "with base clamped to 1.0, attempt {attempt} should return initial"
        );
    }
}

#[test]
fn exponential_base_exactly_1_returns_initial_always() {
    let w = wait::exponential(BASE).base(1.0);
    for attempt in 1..=5 {
        let state = make_state(attempt);
        assert_eq!(w.next_wait(&state), BASE, "attempt {attempt}");
    }
}

#[test]
fn exponential_base_negative_clamped_to_1() {
    let w = wait::exponential(BASE).base(-2.0);
    let state = make_state(3);
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn exponential_base_infinity_clamped_to_1() {
    let w = wait::exponential(BASE).base(f64::INFINITY);
    let state = make_state(3);
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn exponential_base_nan_clamped_to_1() {
    let w = wait::exponential(BASE).base(f64::NAN);
    let state = make_state(3);
    // NAN is not finite, so it should be clamped to base 1.0 (constant initial delay).
    assert_eq!(w.next_wait(&state), BASE);
}

// ---------------------------------------------------------------------------
// 3.5: .cap(max)
// ---------------------------------------------------------------------------

#[test]
fn fixed_cap_has_no_effect_when_below() {
    let w = wait::fixed(BASE).cap(CAP);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), BASE); // 100ms < 500ms cap
}

#[test]
fn exponential_cap_limits_growth() {
    let w = wait::exponential(BASE).cap(CAP);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(100)); // 100ms

    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::from_millis(200)); // 200ms

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::from_millis(400)); // 400ms

    let state = make_state(4);
    // 800ms would exceed 500ms cap
    assert_eq!(w.next_wait(&state), CAP);

    let state = make_state(10);
    // Way above cap
    assert_eq!(w.next_wait(&state), CAP);
}

#[test]
fn linear_cap_limits_growth() {
    let w = wait::linear(BASE, INCREMENT).cap(Duration::from_millis(200));

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(100)); // 100ms

    let state = make_state(3);
    // 100ms + 2*50ms = 200ms == cap
    assert_eq!(w.next_wait(&state), Duration::from_millis(200));

    let state = make_state(4);
    // 100ms + 3*50ms = 250ms > 200ms cap
    assert_eq!(w.next_wait(&state), Duration::from_millis(200));
}

#[test]
fn cap_zero_always_returns_zero() {
    let w = wait::exponential(BASE).cap(Duration::ZERO);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::ZERO);
}

// ---------------------------------------------------------------------------
// 3.7: WaitCombine via Add
// ---------------------------------------------------------------------------

#[test]
fn combine_sums_two_fixed_strategies() {
    let second = Duration::from_millis(200);
    let w = wait::fixed(BASE) + wait::fixed(second);
    let state = make_state(1);
    // 100ms + 200ms = 300ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(300));
}

#[test]
fn combine_sums_exponential_and_fixed() {
    let fixed_part = Duration::from_millis(50);
    let w = wait::exponential(BASE) + wait::fixed(fixed_part);

    let state = make_state(1);
    // 100ms + 50ms = 150ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));

    let state = make_state(2);
    // 200ms + 50ms = 250ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(250));

    let state = make_state(3);
    // 400ms + 50ms = 450ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(450));
}

#[test]
fn combine_three_way_addition() {
    let second = Duration::from_millis(20);
    let third = Duration::from_millis(30);
    let w = wait::fixed(BASE) + wait::fixed(second) + wait::fixed(third);
    let state = make_state(1);
    // 100ms + 20ms + 30ms = 150ms
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));
}

#[test]
fn combine_saturates_on_overflow() {
    let w = wait::fixed(Duration::MAX) + wait::fixed(Duration::from_millis(1));
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

// ---------------------------------------------------------------------------
// 3.8: WaitChain via .chain(other, after)
// ---------------------------------------------------------------------------

#[test]
fn chain_uses_first_strategy_for_early_attempts() {
    let fallback = Duration::from_secs(1);
    let w = wait::fixed(BASE).chain(wait::fixed(fallback), CHAIN_AFTER);

    // First CHAIN_AFTER (3) attempts use the first strategy.
    for attempt in 1..=CHAIN_AFTER {
        let state = make_state(attempt);
        assert_eq!(
            w.next_wait(&state),
            BASE,
            "attempt {attempt} should use first strategy"
        );
    }
}

#[test]
fn chain_switches_to_second_strategy_after_threshold() {
    let fallback = Duration::from_secs(1);
    let w = wait::fixed(BASE).chain(wait::fixed(fallback), CHAIN_AFTER);

    // After CHAIN_AFTER (3) attempts, switch to fallback.
    let state = make_state(CHAIN_AFTER + 1);
    assert_eq!(w.next_wait(&state), fallback);

    let state = make_state(CHAIN_AFTER + 10);
    assert_eq!(w.next_wait(&state), fallback);
}

#[test]
fn chain_with_exponential_strategies() {
    let initial_backoff = Duration::from_millis(10);
    let fallback_fixed = Duration::from_secs(5);
    let switch_after: u32 = 2;
    let w = wait::exponential(initial_backoff).chain(wait::fixed(fallback_fixed), switch_after);

    let state = make_state(1);
    // 10ms * 2^0 = 10ms (first strategy)
    assert_eq!(w.next_wait(&state), Duration::from_millis(10));

    let state = make_state(2);
    // 10ms * 2^1 = 20ms (first strategy, at boundary)
    assert_eq!(w.next_wait(&state), Duration::from_millis(20));

    let state = make_state(3);
    // Past boundary — uses fallback
    assert_eq!(w.next_wait(&state), fallback_fixed);
}

// 3.9: Reset removed — traits use &self and are stateless.

// ---------------------------------------------------------------------------
// 3.10: Clone and Debug
// ---------------------------------------------------------------------------

#[test]
fn fixed_is_clone_and_debug() {
    let w = wait::fixed(BASE);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{:?}", w);
    assert!(!debug.is_empty());
}

#[test]
fn linear_is_clone_and_debug() {
    let w = wait::linear(BASE, INCREMENT);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{:?}", w);
    assert!(!debug.is_empty());
}

#[test]
fn exponential_is_clone_and_debug() {
    let w = wait::exponential(BASE);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{:?}", w);
    assert!(!debug.is_empty());
}

#[test]
fn exponential_with_base_is_clone_and_debug() {
    let w = wait::exponential(BASE).base(3.0);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{:?}", w);
    assert!(!debug.is_empty());
}

#[test]
fn capped_is_clone_and_debug() {
    let w = wait::exponential(BASE).cap(CAP);
    let w2 = w.clone();
    let debug = format!("{:?}", w2);
    assert!(!debug.is_empty());
}

#[test]
fn combine_is_clone_and_debug() {
    let w = wait::fixed(BASE) + wait::fixed(Duration::from_millis(50));
    let w2 = w.clone();
    let debug = format!("{:?}", w2);
    assert!(debug.contains("WaitCombine"));
}

#[test]
fn chain_is_clone_and_debug() {
    let w = wait::fixed(BASE).chain(wait::fixed(Duration::from_secs(1)), CHAIN_AFTER);
    let w2 = w.clone();
    let debug = format!("{:?}", w2);
    assert!(debug.contains("WaitChain"));
}

// ---------------------------------------------------------------------------
// 3.11: Wait strategies return Duration (compile-time check)
// ---------------------------------------------------------------------------

#[test]
fn wait_strategy_returns_duration_not_sleep() {
    // This is a compile-time property test. The Wait trait returns Duration.
    // The test just confirms behavior — we call next_wait and get a Duration.
    let w = wait::fixed(BASE);
    let state = make_state(1);
    let result: Duration = w.next_wait(&state);
    assert_eq!(result, BASE);
}

// ---------------------------------------------------------------------------
// Additional edge cases
// ---------------------------------------------------------------------------

#[test]
fn exponential_with_zero_initial_always_returns_zero() {
    let w = wait::exponential(Duration::ZERO);
    for attempt in 1..=5 {
        let state = make_state(attempt);
        assert_eq!(w.next_wait(&state), Duration::ZERO, "attempt {attempt}");
    }
}

#[test]
fn linear_large_attempt_number_saturates() {
    // With very large initial + large increment, the sum should saturate.
    let w = wait::linear(Duration::MAX, Duration::from_secs(1));
    let state = make_state(2);
    // Duration::MAX + 1s saturates at Duration::MAX.
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn linear_large_multiplier_saturates() {
    // When (n-1)*increment alone overflows Duration, it should saturate.
    let large_increment = Duration::from_secs(u64::MAX);
    let w = wait::linear(BASE, large_increment);
    let state = make_state(3);
    // 2 * Duration::from_secs(u64::MAX) overflows; should saturate.
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn cap_on_combined_strategy() {
    // Verify that .cap() works when combined with +
    let w = (wait::exponential(BASE) + wait::fixed(Duration::from_millis(50))).cap(CAP);

    let state = make_state(1);
    // 100ms + 50ms = 150ms < 500ms cap
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));

    let state = make_state(4);
    // 800ms + 50ms = 850ms > 500ms cap
    assert_eq!(w.next_wait(&state), CAP);
}

#[test]
fn chain_after_zero_always_uses_second() {
    let fallback = Duration::from_secs(1);
    let switch_after: u32 = 0;
    let w = wait::fixed(BASE).chain(wait::fixed(fallback), switch_after);

    // With after=0, always use the second strategy.
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), fallback);
}
