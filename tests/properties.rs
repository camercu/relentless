//! Property-based tests over the public wait-strategy surface.
//!
//! These sweep arbitrary inputs for panics and invariant violations that
//! example-based tests miss — both arithmetic bugs fixed in the mutation-
//! testing pass (zero-initial exponential jumping to `Duration::MAX`, the
//! Embassy conversion overflow) were extreme-input cases of exactly this
//! shape.
//!
//! On failure, proptest prints the failing case and persists its seed under
//! `proptest-regressions/` so the case replays deterministically. Case count
//! is tunable via `PROPTEST_CASES`.

use core::time::Duration;
use proptest::prelude::*;
use relentless::{RetryState, Wait, wait};

const NANOS_PER_SEC: u32 = 1_000_000_000;

/// Bounds keeping `initial_nanos * 2^(attempt - 1)` under 2^52 — inside
/// f64's exact-integer range — where the spec formula must hold exactly.
const EXACT_NANOS_LIMIT: u64 = 1 << 20;
const EXACT_ATTEMPT_LIMIT: u32 = 33;

fn state(attempt: u32) -> RetryState {
    RetryState::new(attempt, None)
}

/// The full `Duration` range, including the extremes where saturation and
/// f64-rounding bugs live.
fn arb_duration() -> impl Strategy<Value = Duration> {
    (any::<u64>(), 0..NANOS_PER_SEC).prop_map(|(secs, nanos)| Duration::new(secs, nanos))
}

proptest! {
    /// 3.2.4 — exponential is total: any initial, any f64 base (the builder
    /// clamps), any attempt. Saturates instead of panicking or overflowing.
    #[test]
    fn exponential_never_panics(
        initial in arb_duration(),
        base in any::<f64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::exponential(initial).base(base);
        let _ = strategy.next_wait(&state(attempt));
    }

    /// 3.2.4 — within f64's exact-integer window the result equals the spec
    /// formula `initial * 2^(attempt - 1)` computed independently in u64.
    #[test]
    fn exponential_matches_integer_reference_in_exact_window(
        nanos in 0..EXACT_NANOS_LIMIT,
        attempt in 1..=EXACT_ATTEMPT_LIMIT,
    ) {
        let strategy = wait::exponential(Duration::from_nanos(nanos));
        let expected = Duration::from_nanos(nanos << (attempt - 1));
        prop_assert_eq!(strategy.next_wait(&state(attempt)), expected);
    }

    /// 3.2.4 — zero initial stays zero at every attempt and base:
    /// `0 * base^n = 0`, even where the multiplier overflows f64.
    #[test]
    fn exponential_zero_initial_is_always_zero(
        base in any::<f64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::exponential(Duration::ZERO).base(base);
        prop_assert_eq!(strategy.next_wait(&state(attempt)), Duration::ZERO);
    }

    /// 3.2.3 — linear equals the spec formula `initial + (attempt - 1) *
    /// increment` computed independently in u128, saturating at
    /// `Duration::MAX`, over the full input range.
    #[test]
    fn linear_matches_u128_reference(
        initial in arb_duration(),
        increment in arb_duration(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::linear(initial, increment);
        let reference = initial.as_nanos()
            + u128::from(attempt.saturating_sub(1)) * increment.as_nanos();
        let expected = if reference > Duration::MAX.as_nanos() {
            Duration::MAX
        } else {
            duration_from_nanos_u128(reference)
        };
        prop_assert_eq!(strategy.next_wait(&state(attempt)), expected);
    }

    /// Full jitter stays in `[0, base]` for any base, seed, and attempt.
    #[test]
    fn full_jitter_stays_within_base(
        base in arb_duration(),
        seed in any::<u64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::fixed(base).full_jitter().with_seed(seed);
        prop_assert!(strategy.next_wait(&state(attempt)) <= base);
    }

    /// Additive jitter stays in `[base, base + max]` (saturating) for any
    /// base, jitter bound, seed, and attempt.
    #[test]
    fn additive_jitter_stays_within_base_plus_max(
        base in arb_duration(),
        max_jitter in arb_duration(),
        seed in any::<u64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::fixed(base).jitter(max_jitter).with_seed(seed);
        let delay = strategy.next_wait(&state(attempt));
        prop_assert!(delay >= base);
        prop_assert!(delay <= base.saturating_add(max_jitter));
    }

    /// Equal jitter stays in `[base/2, base]` for any base, seed, and attempt.
    #[test]
    fn equal_jitter_stays_within_half_to_base(
        base in arb_duration(),
        seed in any::<u64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::fixed(base).equal_jitter().with_seed(seed);
        let delay = strategy.next_wait(&state(attempt));
        prop_assert!(delay >= base / 2);
        prop_assert!(delay <= base);
    }

    /// Decorrelated jitter stays in `[base, max(base, 3 * previous_delay)]`
    /// for any base, previous delay, seed, and attempt.
    #[test]
    fn decorrelated_jitter_stays_within_feedback_bounds(
        base in arb_duration(),
        previous in arb_duration(),
        seed in any::<u64>(),
        attempt in any::<u32>(),
    ) {
        let strategy = wait::decorrelated_jitter(base).with_seed(seed);
        let state = state(attempt).with_previous_delay(Some(previous));
        let delay = strategy.next_wait(&state);
        prop_assert!(delay >= base);
        prop_assert!(delay <= base.max(previous.saturating_mul(3)));
    }
}

/// Converts nanoseconds to a `Duration`; caller guarantees the value fits.
fn duration_from_nanos_u128(nanos: u128) -> Duration {
    Duration::new(
        (nanos / u128::from(NANOS_PER_SEC)) as u64,
        (nanos % u128::from(NANOS_PER_SEC)) as u32,
    )
}
