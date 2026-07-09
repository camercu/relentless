//! Tests for additive jitter on wait strategies.
//!
//! Verifies that jitter stays within [base, base+max], that `.cap()` order does not
//! affect the cap invariant, and that each policy invocation and each clone produces
//! a distinct sequence (decorrelation). Seeded tests confirm reproducibility.

use core::time::Duration;
use relentless::RetryPolicy;
use relentless::Wait;
use relentless::{stop, wait};
use std::cell::RefCell;

const BASE_WAIT: Duration = Duration::from_millis(20);
const MAX_JITTER: Duration = Duration::from_millis(10);
const WAIT_CAP: Duration = Duration::from_millis(25);
const SEEDED_NONCE_A: u64 = 7;
const SEEDED_NONCE_B: u64 = 8;
const SEEDED_ATTEMPT_COUNT: u32 = 8;
const SEEDED_JITTER_SEED: u64 = 0x11;

fn state(attempt: u32) -> relentless::RetryState {
    relentless::RetryState::new(attempt, None)
}

#[test]
fn jitter_additive_stays_within_base_plus_max() {
    let strategy = wait::fixed(BASE_WAIT).jitter(MAX_JITTER);
    let upper = BASE_WAIT.saturating_add(MAX_JITTER);

    for attempt in 1..=64 {
        let delay = strategy.next_wait(&state(attempt));
        assert!(delay >= BASE_WAIT);
        assert!(delay <= upper);
    }
}

#[test]
fn jitter_respects_cap_when_cap_called_before_jitter() {
    let capped_then_jittered = wait::fixed(BASE_WAIT).cap(WAIT_CAP).jitter(MAX_JITTER);

    for attempt in 1..=64 {
        let delay = capped_then_jittered.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[test]
fn jitter_respects_cap_when_cap_called_after_jitter() {
    let jittered_then_capped = wait::fixed(BASE_WAIT).jitter(MAX_JITTER).cap(WAIT_CAP);

    for attempt in 1..=64 {
        let delay = jittered_then_capped.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[test]
fn jitter_sequence_changes_between_policy_invocations() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::fixed(Duration::ZERO).jitter(MAX_JITTER));

    let first: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let second: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|_| Err::<(), _>("retry"))
        .sleep(|dur| first.borrow_mut().push(dur))
        .call();

    let _ = policy
        .retry(|_| Err::<(), _>("retry"))
        .sleep(|dur| second.borrow_mut().push(dur))
        .call();

    assert_eq!(first.borrow().len(), 3);
    assert_eq!(second.borrow().len(), 3);
    assert_ne!(
        *first.borrow(),
        *second.borrow(),
        "jitter should decorrelate retries across independent invocations"
    );
}

#[test]
fn jitter_seed_and_nonce_make_sequence_reproducible() {
    // Construct two identical instances (same seed + nonce) independently.
    let first = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_A);
    let second = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_A);

    for attempt in 1..=SEEDED_ATTEMPT_COUNT {
        assert_eq!(
            first.next_wait(&state(attempt)),
            second.next_wait(&state(attempt))
        );
    }
}

/// 3.3.7
#[test]
fn jitter_with_seed_alone_is_reproducible() {
    // Same seed, no explicit nonce: the nonce is derived from the seed, so the
    // sequences must match.
    let first = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED);
    let second = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED);

    for attempt in 1..=SEEDED_ATTEMPT_COUNT {
        assert_eq!(
            first.next_wait(&state(attempt)),
            second.next_wait(&state(attempt)),
            "same seed alone should produce identical sequences"
        );
    }
}

/// 3.3.7
#[test]
fn jitter_with_seed_resets_prior_nonce() {
    let explicit_nonce_then_seed = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_nonce(SEEDED_NONCE_A)
        .with_seed(SEEDED_JITTER_SEED);
    let seed_only = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED);

    for attempt in 1..=SEEDED_ATTEMPT_COUNT {
        assert_eq!(
            explicit_nonce_then_seed.next_wait(&state(attempt)),
            seed_only.next_wait(&state(attempt)),
            "with_seed should reset any prior nonce; call with_nonce last to pin a custom stream"
        );
    }
}

/// 3.3.4 — cloning re-nonces even when the original was seeded, so a clone
/// diverges from its seeded original.
#[test]
fn jitter_clone_of_seeded_strategy_diverges() {
    let original = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED);
    let cloned = original.clone();

    let orig_delays: Vec<Duration> = (1..=SEEDED_ATTEMPT_COUNT)
        .map(|a| original.next_wait(&state(a)))
        .collect();
    let clone_delays: Vec<Duration> = (1..=SEEDED_ATTEMPT_COUNT)
        .map(|a| cloned.next_wait(&state(a)))
        .collect();

    assert_ne!(
        orig_delays, clone_delays,
        "clone of a seeded strategy should still decorrelate"
    );
}

#[test]
fn jitter_nonce_changes_sequence_for_same_seed() {
    let first = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_A);
    let second = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_B);

    let first_delay = first.next_wait(&state(1));
    let second_delay = second.next_wait(&state(1));
    assert_ne!(first_delay, second_delay);
}

#[test]
fn clone_decorrelates_jitter_sequence() {
    let original = wait::fixed(Duration::ZERO).jitter(MAX_JITTER);
    let cloned = original.clone();

    let orig_delays: Vec<Duration> = (1..=8).map(|a| original.next_wait(&state(a))).collect();
    let clone_delays: Vec<Duration> = (1..=8).map(|a| cloned.next_wait(&state(a))).collect();

    assert_ne!(
        orig_delays, clone_delays,
        "cloned jitter strategy should produce a different sequence"
    );
}

/// 3.3.2
#[test]
fn full_jitter_stays_within_zero_to_base() {
    let strategy = wait::fixed(BASE_WAIT).full_jitter();
    for attempt in 1..=64 {
        let delay = strategy.next_wait(&state(attempt));
        assert!(delay >= Duration::ZERO, "full jitter should be >= 0");
        assert!(delay <= BASE_WAIT, "full jitter should be <= base");
    }
}

/// 3.3.3
#[test]
fn equal_jitter_stays_within_half_base_to_base() {
    let strategy = wait::fixed(BASE_WAIT).equal_jitter();
    let lower_bound = BASE_WAIT / 2;
    for attempt in 1..=64 {
        let delay = strategy.next_wait(&state(attempt));
        assert!(
            delay >= lower_bound,
            "equal jitter should be >= base/2, got {delay:?}"
        );
        assert!(
            delay <= BASE_WAIT,
            "equal jitter should be <= base, got {delay:?}"
        );
    }
}

/// 3.3.5
#[test]
fn decorrelated_jitter_first_attempt_range() {
    let base = Duration::from_millis(100);
    let upper = base.saturating_mul(3);

    for _ in 0..32 {
        // Fresh strategy each time; the first attempt has no previous delay.
        let strategy = wait::decorrelated_jitter(base);
        let delay = strategy.next_wait(&state(1));
        assert!(delay >= base, "decorrelated jitter should be >= base");
        assert!(
            delay <= upper,
            "decorrelated jitter first attempt should be <= base*3, got {delay:?}"
        );
    }
}

/// 3.3.5
#[test]
fn decorrelated_jitter_subsequent_attempts_bounded_by_prev_times_3() {
    let base = Duration::from_millis(50);
    let strategy = wait::decorrelated_jitter(base);

    // Feedback is supplied by the engine via RetryState::previous_delay, so the
    // upper bound is previous_delay * 3 (not the strategy's own prior output).
    let prev = Duration::from_millis(200);
    let upper = prev.saturating_mul(3);
    let st = relentless::RetryState::new(2, None).with_previous_delay(Some(prev));
    for _ in 0..32 {
        let delay = strategy.next_wait(&st);
        assert!(delay >= base, "should be >= base");
        assert!(delay <= upper, "should be <= previous_delay*3");
    }
}

/// 3.3.4
#[test]
fn full_jitter_clone_produces_decorrelated_sequence() {
    let original = wait::fixed(BASE_WAIT).full_jitter();
    let cloned = original.clone();

    let orig_delays: Vec<Duration> = (1..=16).map(|a| original.next_wait(&state(a))).collect();
    let clone_delays: Vec<Duration> = (1..=16).map(|a| cloned.next_wait(&state(a))).collect();

    assert_ne!(
        orig_delays, clone_delays,
        "cloned full jitter strategy should produce a different sequence"
    );
}

#[test]
fn equal_jitter_clone_produces_decorrelated_sequence() {
    let original = wait::fixed(BASE_WAIT).equal_jitter();
    let cloned = original.clone();

    let orig_delays: Vec<Duration> = (1..=16).map(|a| original.next_wait(&state(a))).collect();
    let clone_delays: Vec<Duration> = (1..=16).map(|a| cloned.next_wait(&state(a))).collect();

    assert_ne!(
        orig_delays, clone_delays,
        "cloned equal jitter strategy should produce a different sequence"
    );
}

/// 3.3.6
#[test]
fn decorrelated_jitter_clone_diverges() {
    let base = Duration::from_millis(100);
    let original = wait::decorrelated_jitter(base);

    let clone_a = original.clone();
    let clone_b = original.clone();

    let a_delays: Vec<Duration> = (1..=8).map(|i| clone_a.next_wait(&state(i))).collect();
    let b_delays: Vec<Duration> = (1..=8).map(|i| clone_b.next_wait(&state(i))).collect();

    assert_ne!(
        a_delays, b_delays,
        "two clones of decorrelated jitter should diverge (different PRNG streams)"
    );
}

/// 3.3.7
#[test]
fn decorrelated_jitter_with_seed_is_reproducible() {
    let base = Duration::from_millis(50);
    let seed = 0xDEAD_BEEF_u64;
    let nonce = 42_u64;

    let first = wait::decorrelated_jitter(base)
        .with_seed(seed)
        .with_nonce(nonce);
    let second = wait::decorrelated_jitter(base)
        .with_seed(seed)
        .with_nonce(nonce);

    for i in 1..=8_u32 {
        assert_eq!(
            first.next_wait(&state(i)),
            second.next_wait(&state(i)),
            "same seed+nonce should produce identical sequences"
        );
    }
}

/// §10 — a jittered policy shared by reference across threads: each loop
/// draws from the same atomic PRNG stream. The first attempt fails so every
/// thread actually reaches `next_wait` and draws concurrently.
#[test]
fn jittered_policy_is_shareable_across_threads() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(BASE_WAIT).jitter(MAX_JITTER));

    std::thread::scope(|scope| {
        for _ in 0..4 {
            let policy = &policy;
            scope.spawn(move || {
                let result = policy
                    .retry(|state| {
                        if state.attempt >= 2 {
                            Ok::<u32, &str>(1)
                        } else {
                            Err("first attempt fails to force a jitter draw")
                        }
                    })
                    .sleep(|_| {})
                    .call();
                assert_eq!(result.unwrap(), 1);
            });
        }
    });
}

/// 3.3.5
#[test]
fn decorrelated_jitter_with_cap_respects_max() {
    let base = Duration::from_millis(100);
    let cap = Duration::from_millis(150);
    let strategy = wait::decorrelated_jitter(base).cap(cap);

    for attempt in 1..=32 {
        let delay = strategy.next_wait(&state(attempt));
        assert!(
            delay <= cap,
            "decorrelated jitter with cap should not exceed cap, got {delay:?} at attempt {attempt}"
        );
    }
}

/// Pins the seeded jitter sequence to reference values computed with an
/// independent SplitMix64 implementation (state += golden gamma, then the
/// standard 30/27/31 xor-shift finalizer; instance nonce = one draw from a
/// generator seeded with the seed itself, XORed back into the seed).
///
/// Range/determinism checks alone cannot catch a degraded generator — a PRNG
/// mutated to emit constants or weakened mixing still stays in range and
/// still reproduces per seed. Known answers pin the full pipeline: nonce
/// derivation, state advance, finalizer, and the inclusive `[0, base]` draw.
#[test]
fn jitter_with_seed_matches_reference_splitmix64_vector() {
    const VECTOR_SEED: u64 = 0x0123_4567_89AB_CDEF;
    const VECTOR_BASE: Duration = Duration::from_nanos(1_000_000);
    const EXPECTED_NANOS: [u64; 4] = [531_209, 674_557, 751_447, 96_937];

    let strategy = wait::fixed(VECTOR_BASE)
        .full_jitter()
        .with_seed(VECTOR_SEED);

    let drawn: Vec<Duration> = (1..=EXPECTED_NANOS.len() as u32)
        .map(|attempt| strategy.next_wait(&state(attempt)))
        .collect();
    let expected: Vec<Duration> = EXPECTED_NANOS
        .into_iter()
        .map(Duration::from_nanos)
        .collect();
    assert_eq!(drawn, expected);
}
