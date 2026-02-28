//! Seeded property-style tests for composition invariants.

use core::time::Duration;
use std::env;
use tenacious::{Predicate, Stop, Wait, WaitExt, on, stop, wait};

#[path = "support/property_seed.rs"]
mod property_seed;
use property_seed::{PROPTEST_SEED_ENV, derive_stream_seed, parse_seed, run_seed};

const SAMPLE_COUNT: u32 = 1_024;
const MAX_ATTEMPT: u32 = 32;
const MAX_ELAPSED_MILLIS: u64 = 500;
const MAX_DELAY_MILLIS: u64 = 200;
const MAX_DEADLINE_MILLIS: u64 = 400;
const MAX_INITIAL_WAIT_MILLIS: u64 = 50;
const MAX_INCREMENT_MILLIS: u64 = 20;
const WAIT_CAP_MILLIS: u64 = 60;
const CHAIN_SWITCH_ATTEMPT: u32 = 5;
const OK_THRESHOLD: u32 = 40;
const SAMPLE_INDEX_OFFSET: u32 = 1;
const PREDICATE_PAYLOAD_MAX: u32 = 100;
const REPRO_SEQUENCE_LENGTH: u32 = 128;
const DIGEST_ROTATE_LEFT_BITS: u32 = 7;
const DIGEST_MIX_MULTIPLIER: u64 = 0x0000_0001_0000_01B3;

const LCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const LCG_INCREMENT: u64 = 1_442_695_040_888_963_407;
const STOP_STREAM_DISCRIMINANT: u64 = 1;
const WAIT_STREAM_DISCRIMINANT: u64 = 2;
const PREDICATE_STREAM_DISCRIMINANT: u64 = 3;

fn stream_signature(run_seed: u64, stream_discriminant: u64) -> u64 {
    let mut seed = derive_stream_seed(run_seed, stream_discriminant);
    let mut signature = 0_u64;

    for sample_index in 0..REPRO_SEQUENCE_LENGTH {
        let random_word = next_u64(&mut seed);
        let sample = u64::from(sample_index + SAMPLE_INDEX_OFFSET);
        signature = signature
            .rotate_left(DIGEST_ROTATE_LEFT_BITS)
            .wrapping_mul(DIGEST_MIX_MULTIPLIER)
            ^ random_word
            ^ sample;
    }

    signature
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(LCG_MULTIPLIER)
        .wrapping_add(LCG_INCREMENT);
    *state
}

fn bounded_u32(state: &mut u64, max_inclusive: u32) -> u32 {
    ((next_u64(state) % u64::from(max_inclusive)) as u32).saturating_add(1)
}

fn bounded_u64(state: &mut u64, max_inclusive: u64) -> u64 {
    (next_u64(state) % max_inclusive).saturating_add(1)
}

fn make_state(state: &mut u64) -> tenacious::RetryState {
    let attempt = bounded_u32(state, MAX_ATTEMPT);
    let elapsed = if next_u64(state) & 1 == 0 {
        Some(Duration::from_millis(bounded_u64(
            state,
            MAX_ELAPSED_MILLIS,
        )))
    } else {
        None
    };
    let next_delay = Duration::from_millis(bounded_u64(state, MAX_DELAY_MILLIS));

    tenacious::RetryState {
        attempt,
        elapsed,
        next_delay,
        total_wait: Duration::ZERO,
    }
}

#[test]
fn stop_composition_matches_boolean_algebra() {
    let run_seed = run_seed();
    let mut seed = derive_stream_seed(run_seed, STOP_STREAM_DISCRIMINANT);

    for sample_index in 0..SAMPLE_COUNT {
        let state = make_state(&mut seed);
        let max_attempts = bounded_u32(&mut seed, MAX_ATTEMPT);
        let deadline = Duration::from_millis(bounded_u64(&mut seed, MAX_DEADLINE_MILLIS));
        let sample = sample_index + SAMPLE_INDEX_OFFSET;

        let mut left = stop::attempts(max_attempts);
        let mut right = stop::before_elapsed(deadline);
        let left_value = left.should_stop(&state);
        let right_value = right.should_stop(&state);

        let mut either = stop::attempts(max_attempts) | stop::before_elapsed(deadline);
        let mut both = stop::attempts(max_attempts) & stop::before_elapsed(deadline);

        assert_eq!(
            either.should_stop(&state),
            left_value || right_value,
            "repro: {}={:#018x}; sample={}; invariant=stop-or",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );
        assert_eq!(
            both.should_stop(&state),
            left_value && right_value,
            "repro: {}={:#018x}; sample={}; invariant=stop-and",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );
    }
}

#[test]
fn wait_composition_matches_saturating_arithmetic_and_builders() {
    let run_seed = run_seed();
    let mut seed = derive_stream_seed(run_seed, WAIT_STREAM_DISCRIMINANT);
    let cap = Duration::from_millis(WAIT_CAP_MILLIS);

    for sample_index in 0..SAMPLE_COUNT {
        let state = make_state(&mut seed);
        let initial = Duration::from_millis(bounded_u64(&mut seed, MAX_INITIAL_WAIT_MILLIS));
        let increment = Duration::from_millis(bounded_u64(&mut seed, MAX_INCREMENT_MILLIS));
        let fallback = Duration::from_millis(bounded_u64(&mut seed, MAX_INITIAL_WAIT_MILLIS));
        let sample = sample_index + SAMPLE_INDEX_OFFSET;

        let mut left = wait::linear(initial, increment);
        let mut right = wait::fixed(fallback);
        let left_value = left.next_wait(&state);
        let right_value = right.next_wait(&state);

        let mut combined = wait::linear(initial, increment) + wait::fixed(fallback);
        assert_eq!(
            combined.next_wait(&state),
            left_value.saturating_add(right_value),
            "repro: {}={:#018x}; sample={}; invariant=wait-add",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );

        let mut capped = wait::linear(initial, increment).cap(cap);
        assert!(
            capped.next_wait(&state) <= cap,
            "repro: {}={:#018x}; sample={}; invariant=wait-cap",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );

        let mut chained =
            wait::linear(initial, increment).chain(wait::fixed(fallback), CHAIN_SWITCH_ATTEMPT);
        let expected = if state.attempt <= CHAIN_SWITCH_ATTEMPT {
            wait::linear(initial, increment).next_wait(&state)
        } else {
            fallback
        };
        assert_eq!(
            chained.next_wait(&state),
            expected,
            "repro: {}={:#018x}; sample={}; invariant=wait-chain",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );
    }
}

#[test]
fn predicate_composition_matches_boolean_algebra() {
    let run_seed = run_seed();
    let mut seed = derive_stream_seed(run_seed, PREDICATE_STREAM_DISCRIMINANT);

    for sample_index in 0..SAMPLE_COUNT {
        let payload = bounded_u32(&mut seed, PREDICATE_PAYLOAD_MAX);
        let outcome: Result<u32, u32> = if next_u64(&mut seed) & 1 == 0 {
            Ok(payload)
        } else {
            Err(payload)
        };
        let sample = sample_index + SAMPLE_INDEX_OFFSET;

        let left = on::error(|err: &u32| err % 2 == 0);
        let right = on::ok(|value: &u32| *value < OK_THRESHOLD);
        let left_value = left.should_retry(&outcome);
        let right_value = right.should_retry(&outcome);

        let either =
            on::error(|err: &u32| err % 2 == 0) | on::ok(|value: &u32| *value < OK_THRESHOLD);
        let both =
            on::error(|err: &u32| err % 2 == 0) & on::ok(|value: &u32| *value < OK_THRESHOLD);

        assert_eq!(
            either.should_retry(&outcome),
            left_value || right_value,
            "repro: {}={:#018x}; sample={}; invariant=predicate-or",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );
        assert_eq!(
            both.should_retry(&outcome),
            left_value && right_value,
            "repro: {}={:#018x}; sample={}; invariant=predicate-and",
            PROPTEST_SEED_ENV,
            run_seed,
            sample
        );
    }
}

#[test]
fn seeded_run_is_exactly_reproducible_when_seed_env_is_set() {
    let Ok(raw_seed) = env::var(PROPTEST_SEED_ENV) else {
        return;
    };

    let expected_seed = parse_seed(&raw_seed).unwrap_or_else(|err| {
        panic!(
            "invalid {} value {:?}: {}; expected decimal or 0x-prefixed hex u64",
            PROPTEST_SEED_ENV, raw_seed, err
        )
    });
    let effective_seed = run_seed();
    assert_eq!(
        effective_seed, expected_seed,
        "repro seed mismatch: env {}={}; effective={:#018x}",
        PROPTEST_SEED_ENV, raw_seed, effective_seed
    );

    let first_digest = stream_signature(effective_seed, STOP_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, STOP_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "stop stream mismatch under fixed seed: {}={:#018x}",
        PROPTEST_SEED_ENV, effective_seed
    );

    let first_digest = stream_signature(effective_seed, WAIT_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, WAIT_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "wait stream mismatch under fixed seed: {}={:#018x}",
        PROPTEST_SEED_ENV, effective_seed
    );

    let first_digest = stream_signature(effective_seed, PREDICATE_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, PREDICATE_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "predicate stream mismatch under fixed seed: {}={:#018x}",
        PROPTEST_SEED_ENV, effective_seed
    );
}
