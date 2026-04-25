//! Tests for the Wait trait and built-in wait strategies.
//!
//! Verifies the formula contracts for fixed, linear, and exponential backoff (including
//! saturation on overflow), the boundary behavior of `.cap()` and `.chain()`, composition
//! via `+` (`WaitCombine`), and that provided methods work on user-defined Wait impls.

use core::time::Duration;
use relentless::Wait;
use relentless::wait;

const BASE: Duration = Duration::from_millis(100);
const INCREMENT: Duration = Duration::from_millis(50);
const CAP: Duration = Duration::from_millis(500);
const CHAIN_AFTER: u32 = 3;
const ARBITRARY_ADDEND: Duration = Duration::from_millis(50);
const ARBITRARY_SMALL_ADDEND: Duration = Duration::from_millis(5);
const ARBITRARY_JITTER_MAX: Duration = Duration::from_millis(20);
const ARBITRARY_FALLBACK: Duration = Duration::from_secs(1);

fn make_state(attempt: u32) -> relentless::RetryState {
    relentless::RetryState::new(attempt, None)
}

/// Minimal Wait implementation used to verify the trait contract.
struct FixedWait {
    dur: Duration,
}

impl Wait for FixedWait {
    fn next_wait(&self, _state: &relentless::RetryState) -> Duration {
        self.dur
    }
}

#[test]
fn wait_next_wait_takes_ref_self_and_retry_state() {
    let wait = FixedWait { dur: BASE };
    let state = make_state(1);
    assert_eq!(wait.next_wait(&state), BASE);
}

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

#[test]
fn linear_first_attempt_returns_initial() {
    let w = wait::linear(BASE, INCREMENT);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), BASE); // initial + (1-1)*increment = initial
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

#[test]
fn exponential_first_attempt_returns_initial() {
    let w = wait::exponential(BASE);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), BASE); // initial * 2^0 = initial
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
    let state = make_state(u32::MAX); // exponent overflows; must saturate rather than panic
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn exponential_with_base_3() {
    let base_multiplier = 3.0;
    let w = wait::exponential(BASE).base(base_multiplier);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), BASE); // 100ms * 3^0 = 100ms

    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::from_millis(300)); // 100ms * 3^1

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::from_millis(900)); // 100ms * 3^2
}

#[test]
fn exponential_base_below_1_clamped_to_1() {
    let w = wait::exponential(BASE).base(0.5);
    // base < 1 is clamped to 1.0, making the result constant at the initial value
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
    // NaN is not finite — falls into the same "clamp to 1.0" path as other invalid values
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn fixed_cap_has_no_effect_when_below() {
    let w = wait::fixed(BASE).cap(CAP);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), BASE);
}

#[test]
fn exponential_cap_limits_growth() {
    let w = wait::exponential(BASE).cap(CAP);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(100));

    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::from_millis(200));

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::from_millis(400));

    let state = make_state(4);
    assert_eq!(w.next_wait(&state), CAP); // 800ms uncapped > 500ms cap

    let state = make_state(10);
    assert_eq!(w.next_wait(&state), CAP);
}

#[test]
fn linear_cap_limits_growth() {
    let w = wait::linear(BASE, INCREMENT).cap(Duration::from_millis(200));

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(100));

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::from_millis(200)); // 100 + 2*50 == cap

    let state = make_state(4);
    assert_eq!(w.next_wait(&state), Duration::from_millis(200)); // 100 + 3*50 > cap, clamped
}

#[test]
fn cap_zero_always_returns_zero() {
    let w = wait::exponential(BASE).cap(Duration::ZERO);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::ZERO);
}

#[test]
fn combine_sums_two_fixed_strategies() {
    let second = Duration::from_millis(200);
    let w = wait::fixed(BASE) + wait::fixed(second);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(300));
}

#[test]
fn combine_sums_exponential_and_fixed() {
    let fixed_part = Duration::from_millis(50);
    let w = wait::exponential(BASE) + wait::fixed(fixed_part);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));

    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::from_millis(250));

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::from_millis(450));
}

#[test]
fn combine_three_way_addition() {
    let second = Duration::from_millis(20);
    let third = Duration::from_millis(30);
    let w = wait::fixed(BASE) + wait::fixed(second) + wait::fixed(third);
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(150));
}

#[test]
fn combine_saturates_on_overflow() {
    let w = wait::fixed(Duration::MAX) + wait::fixed(Duration::from_millis(1));
    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn chain_uses_first_strategy_for_early_attempts() {
    let fallback = Duration::from_secs(1);
    let w = wait::fixed(BASE).chain(wait::fixed(fallback), CHAIN_AFTER);

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
    assert_eq!(w.next_wait(&state), Duration::from_millis(10)); // 10ms * 2^0, first strategy

    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::from_millis(20)); // 10ms * 2^1, last first-strategy attempt

    let state = make_state(3);
    assert_eq!(w.next_wait(&state), fallback_fixed); // past switch_after threshold
}

#[test]
fn fixed_is_clone_and_debug() {
    let w = wait::fixed(BASE);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{w:?}");
    assert!(!debug.is_empty());
}

#[test]
fn linear_is_clone_and_debug() {
    let w = wait::linear(BASE, INCREMENT);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{w:?}");
    assert!(!debug.is_empty());
}

#[test]
fn exponential_is_clone_and_debug() {
    let w = wait::exponential(BASE);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{w:?}");
    assert!(!debug.is_empty());
}

#[test]
fn exponential_with_base_is_clone_and_debug() {
    let w = wait::exponential(BASE).base(3.0);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&w);
    let debug = format!("{w:?}");
    assert!(!debug.is_empty());
}

#[test]
fn capped_is_clone_and_debug() {
    let w = wait::exponential(BASE).cap(CAP);
    let w2 = w.clone();
    let debug = format!("{w2:?}");
    assert!(!debug.is_empty());
}

#[test]
fn combine_is_clone_and_debug() {
    let w = wait::fixed(BASE) + wait::fixed(Duration::from_millis(50));
    let w2 = w.clone();
    let debug = format!("{w2:?}");
    assert!(debug.contains("WaitCombine"));
}

#[test]
fn chain_is_clone_and_debug() {
    let w = wait::fixed(BASE).chain(wait::fixed(Duration::from_secs(1)), CHAIN_AFTER);
    let w2 = w.clone();
    let debug = format!("{w2:?}");
    assert!(debug.contains("WaitChain"));
}

#[test]
fn wait_strategy_returns_duration_not_sleep() {
    // The return type annotation enforces the compile-time contract.
    let w = wait::fixed(BASE);
    let state = make_state(1);
    let result: Duration = w.next_wait(&state);
    assert_eq!(result, BASE);
}

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
    let w = wait::linear(Duration::MAX, Duration::from_secs(1));
    let state = make_state(2);
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn linear_large_multiplier_saturates() {
    // (n-1)*increment overflows when increment itself is near Duration::MAX
    let large_increment = Duration::from_secs(u64::MAX);
    let w = wait::linear(BASE, large_increment);
    let state = make_state(3);
    assert_eq!(w.next_wait(&state), Duration::MAX);
}

#[test]
fn cap_on_combined_strategy() {
    let w = (wait::exponential(BASE) + wait::fixed(Duration::from_millis(50))).cap(CAP);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), Duration::from_millis(150)); // 100 + 50 < cap

    let state = make_state(4);
    assert_eq!(w.next_wait(&state), CAP); // 800 + 50 > cap
}

#[test]
fn chain_after_zero_always_uses_second() {
    let fallback = Duration::from_secs(1);
    let switch_after: u32 = 0;
    let w = wait::fixed(BASE).chain(wait::fixed(fallback), switch_after);

    let state = make_state(1);
    assert_eq!(w.next_wait(&state), fallback);
}

#[derive(Clone, Copy)]
struct StepWait {
    base: Duration,
    increment: Duration,
}

impl Wait for StepWait {
    fn next_wait(&self, state: &relentless::RetryState) -> Duration {
        let step = self
            .increment
            .checked_mul(state.attempt.saturating_sub(1))
            .unwrap_or(Duration::MAX);
        self.base.saturating_add(step)
    }
}

#[test]
fn custom_wait_supports_cap_and_chain_via_wait_ext() {
    let strategy = StepWait {
        base: Duration::from_millis(10),
        increment: Duration::from_millis(5),
    }
    .cap(Duration::from_millis(22))
    .chain(wait::fixed(Duration::from_millis(40)), 2);

    assert_eq!(
        strategy.next_wait(&make_state(1)),
        Duration::from_millis(10).min(Duration::from_millis(22))
    );
    assert_eq!(
        strategy.next_wait(&make_state(3)),
        Duration::from_millis(40)
    );
}

#[test]
fn custom_wait_supports_jitter_via_wait_ext() {
    let strategy = StepWait {
        base: Duration::from_millis(10),
        increment: Duration::from_millis(5),
    }
    .jitter(Duration::from_millis(7));

    let baseline = Duration::from_millis(10).saturating_add(Duration::from_millis(5));
    let upper = baseline.saturating_add(Duration::from_millis(7));
    let wait = strategy.next_wait(&make_state(2));

    assert!(wait >= baseline);
    assert!(wait <= upper);
}

#[test]
fn wait_named_add_matches_operator_and_supports_custom_wait() {
    let retry_state = make_state(1);
    let wait_a = Duration::from_millis(7);
    let wait_b = Duration::from_millis(11);

    let named = wait::fixed(wait_a).add(wait::fixed(wait_b));
    let op = wait::fixed(wait_a) + wait::fixed(wait_b);
    assert_eq!(named.next_wait(&retry_state), op.next_wait(&retry_state));

    #[derive(Clone, Copy)]
    struct CustomWait(Duration);
    impl Wait for CustomWait {
        fn next_wait(&self, _state: &relentless::RetryState) -> Duration {
            self.0
        }
    }

    let custom = CustomWait(wait_a).add(wait::fixed(wait_b));
    assert_eq!(
        custom.next_wait(&retry_state),
        wait_a.saturating_add(wait_b)
    );
}

/// 3.2.8
#[test]
fn zero_duration_sleep_is_skipped() {
    use relentless::{RetryPolicy, stop};
    use std::cell::Cell;

    let sleep_calls = Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .sleep(|_dur| {
            sleep_calls.set(sleep_calls.get().saturating_add(1));
        })
        .call();

    assert_eq!(
        sleep_calls.get(),
        0,
        "sleep should not be called when wait returns Duration::ZERO"
    );
}

#[test]
fn chain_plus_fixed_combines_via_add() {
    let chained = wait::exponential(BASE).chain(wait::fixed(ARBITRARY_FALLBACK), CHAIN_AFTER);
    let combined = chained + wait::fixed(ARBITRARY_ADDEND);

    let state = make_state(1);
    assert_eq!(
        combined.next_wait(&state),
        BASE.saturating_add(ARBITRARY_ADDEND)
    );

    let state = make_state(CHAIN_AFTER + 1);
    assert_eq!(
        combined.next_wait(&state),
        ARBITRARY_FALLBACK.saturating_add(ARBITRARY_ADDEND)
    );
}

#[test]
fn capped_plus_fixed_combines_via_add() {
    let capped = wait::exponential(BASE).cap(CAP);
    let combined = capped + wait::fixed(ARBITRARY_SMALL_ADDEND);

    let state = make_state(1);
    assert_eq!(
        combined.next_wait(&state),
        BASE.saturating_add(ARBITRARY_SMALL_ADDEND)
    );

    let state = make_state(10);
    assert_eq!(
        combined.next_wait(&state),
        CAP.saturating_add(ARBITRARY_SMALL_ADDEND)
    );
}

#[test]
fn jittered_plus_fixed_combines_via_add() {
    let jittered = wait::fixed(BASE).jitter(ARBITRARY_JITTER_MAX);
    let combined = jittered + wait::fixed(ARBITRARY_SMALL_ADDEND);

    let state = make_state(1);
    let result = combined.next_wait(&state);
    let lower = BASE.saturating_add(ARBITRARY_SMALL_ADDEND);
    let upper = lower.saturating_add(ARBITRARY_JITTER_MAX);
    assert!(result >= lower);
    assert!(result <= upper);
}

/// §14
#[test]
fn wait_exponential_is_partial_eq_not_eq() {
    // PartialEq is satisfied (compile-time check + runtime assertion).
    let a = wait::exponential(BASE);
    let b = wait::exponential(BASE);
    assert_eq!(a, b);

    // Eq is NOT implemented for WaitExponential because f64 fields are not Eq.
    // This is verified at compile time: WaitFixed, WaitLinear DO implement Eq.
    fn assert_eq_impl<T: Eq>(_: &T) {}
    assert_eq_impl(&wait::fixed(BASE));
    assert_eq_impl(&wait::linear(BASE, BASE));
    // WaitExponential intentionally does NOT implement Eq (f64 field).
    // The following would be a compile error:
    //   assert_eq_impl(&wait::exponential(BASE)); // compile error: WaitExponential: !Eq
}

/// §14
#[test]
fn wait_all_basic_types_implement_partial_eq() {
    let fixed1 = wait::fixed(BASE);
    let fixed2 = wait::fixed(BASE);
    assert_eq!(fixed1, fixed2);
    assert_ne!(fixed1, wait::fixed(BASE + Duration::from_millis(1)));

    let lin1 = wait::linear(BASE, INCREMENT);
    let lin2 = wait::linear(BASE, INCREMENT);
    assert_eq!(lin1, lin2);

    let exp1 = wait::exponential(BASE);
    let exp2 = wait::exponential(BASE);
    assert_eq!(exp1, exp2);
}
