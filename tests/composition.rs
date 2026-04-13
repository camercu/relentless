//! Seeded property tests verifying Stop, Wait, and Predicate composition
//! obeys boolean/arithmetic algebra.

use core::time::Duration;
use relentless::{Predicate, Stop, Wait, predicate, stop, wait};
use std::env;

const PROPTEST_SEED_ENV: &str = "RELENTLESS_PROPTEST_SEED";

const STREAM_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_MIX_MULTIPLIER_1: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_MIX_MULTIPLIER_2: u64 = 0x94D0_49BB_1331_11EB;
const SPLITMIX_XOR_SHIFT_1: u32 = 30;
const SPLITMIX_XOR_SHIFT_2: u32 = 27;
const SPLITMIX_XOR_SHIFT_3: u32 = 31;
const TIME_ROTATE_LEFT_BITS: u32 = 11;
const PID_ROTATE_LEFT_BITS: u32 = 23;

static RUN_SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(SPLITMIX_INCREMENT);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_1)).wrapping_mul(SPLITMIX_MIX_MULTIPLIER_1);
    mixed = (mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_2)).wrapping_mul(SPLITMIX_MIX_MULTIPLIER_2);
    mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_3)
}

fn parse_seed(raw_seed: &str) -> Result<u64, String> {
    let normalized = raw_seed.trim().replace('_', "");
    let parsed = if let Some(hex_digits) = normalized
        .strip_prefix("0x")
        .or_else(|| normalized.strip_prefix("0X"))
    {
        u64::from_str_radix(hex_digits, 16)
    } else {
        normalized.parse::<u64>()
    };
    parsed.map_err(|err| err.to_string())
}

fn random_default_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos() as u64;
    let secs = now.as_secs();
    let pid = u64::from(std::process::id());
    let mut entropy =
        nanos ^ secs.rotate_left(TIME_ROTATE_LEFT_BITS) ^ pid.rotate_left(PID_ROTATE_LEFT_BITS);
    splitmix64(&mut entropy)
}

fn run_seed() -> u64 {
    *RUN_SEED.get_or_init(|| match env::var(PROPTEST_SEED_ENV) {
        Ok(raw_seed) => parse_seed(&raw_seed).unwrap_or_else(|err| {
            panic!(
                "invalid {PROPTEST_SEED_ENV} value {raw_seed:?}: {err}; expected decimal or 0x-prefixed hex u64"
            )
        }),
        Err(env::VarError::NotPresent) => random_default_seed(),
        Err(env::VarError::NotUnicode(raw_seed)) => {
            panic!(
                "invalid {PROPTEST_SEED_ENV} value {raw_seed:?}: expected valid UTF-8"
            )
        }
    })
}

fn derive_stream_seed(seed: u64, stream_discriminant: u64) -> u64 {
    let mut mixed = seed ^ stream_discriminant.wrapping_mul(STREAM_SALT);
    splitmix64(&mut mixed)
}

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

fn make_state(state: &mut u64) -> relentless::RetryState {
    let attempt = bounded_u32(state, MAX_ATTEMPT);
    let elapsed = if next_u64(state) & 1 == 0 {
        Some(Duration::from_millis(bounded_u64(
            state,
            MAX_ELAPSED_MILLIS,
        )))
    } else {
        None
    };
    // Advance the stream by one slot that was reserved for next_delay when
    // this helper was designed, so all three property tests draw from the
    // same fixed-width per-sample layout and remain reproducible if new
    // fields are added later.
    let _next_delay = Duration::from_millis(bounded_u64(state, MAX_DELAY_MILLIS));

    relentless::RetryState::new(attempt, elapsed)
}

/// Folds `REPRO_SEQUENCE_LENGTH` random draws from a stream into a single
/// digest. Used by the reproducibility test to verify that a fixed seed
/// produces the exact same sequence on every run.
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

#[test]
fn stop_composition_matches_boolean_algebra() {
    let run_seed = run_seed();
    let mut seed = derive_stream_seed(run_seed, STOP_STREAM_DISCRIMINANT);

    for sample_index in 0..SAMPLE_COUNT {
        let state = make_state(&mut seed);
        let max_attempts = bounded_u32(&mut seed, MAX_ATTEMPT);
        let deadline = Duration::from_millis(bounded_u64(&mut seed, MAX_DEADLINE_MILLIS));
        let sample = sample_index + SAMPLE_INDEX_OFFSET;

        let left = stop::attempts(max_attempts);
        let right = stop::elapsed(deadline);
        let left_value = left.should_stop(&state);
        let right_value = right.should_stop(&state);

        let either = stop::attempts(max_attempts) | stop::elapsed(deadline);
        let both = stop::attempts(max_attempts) & stop::elapsed(deadline);

        assert_eq!(
            either.should_stop(&state),
            left_value || right_value,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=stop-or"
        );
        assert_eq!(
            both.should_stop(&state),
            left_value && right_value,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=stop-and"
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

        let left = wait::linear(initial, increment);
        let right = wait::fixed(fallback);
        let left_value = left.next_wait(&state);
        let right_value = right.next_wait(&state);

        let combined = wait::linear(initial, increment) + wait::fixed(fallback);
        assert_eq!(
            combined.next_wait(&state),
            left_value.saturating_add(right_value),
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=wait-add"
        );

        let capped = wait::linear(initial, increment).cap(cap);
        assert!(
            capped.next_wait(&state) <= cap,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=wait-cap"
        );

        let chained =
            wait::linear(initial, increment).chain(wait::fixed(fallback), CHAIN_SWITCH_ATTEMPT);
        let expected = if state.attempt <= CHAIN_SWITCH_ATTEMPT {
            wait::linear(initial, increment).next_wait(&state)
        } else {
            fallback
        };
        assert_eq!(
            chained.next_wait(&state),
            expected,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=wait-chain"
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

        let left = predicate::error(|err: &u32| err % 2 == 0);
        let right = predicate::ok(|value: &u32| *value < OK_THRESHOLD);
        let left_value = left.should_retry(&outcome);
        let right_value = right.should_retry(&outcome);

        let either = predicate::error(|err: &u32| err % 2 == 0)
            | predicate::ok(|value: &u32| *value < OK_THRESHOLD);
        let both = predicate::error(|err: &u32| err % 2 == 0)
            & predicate::ok(|value: &u32| *value < OK_THRESHOLD);

        assert_eq!(
            either.should_retry(&outcome),
            left_value || right_value,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=predicate-or"
        );
        assert_eq!(
            both.should_retry(&outcome),
            left_value && right_value,
            "repro: {PROPTEST_SEED_ENV}={run_seed:#018x}; sample={sample}; invariant=predicate-and"
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
            "invalid {PROPTEST_SEED_ENV} value {raw_seed:?}: {err}; expected decimal or 0x-prefixed hex u64"
        )
    });
    let effective_seed = run_seed();
    assert_eq!(
        effective_seed, expected_seed,
        "repro seed mismatch: env {PROPTEST_SEED_ENV}={raw_seed}; effective={effective_seed:#018x}"
    );

    let first_digest = stream_signature(effective_seed, STOP_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, STOP_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "stop stream mismatch under fixed seed: {PROPTEST_SEED_ENV}={effective_seed:#018x}"
    );

    let first_digest = stream_signature(effective_seed, WAIT_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, WAIT_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "wait stream mismatch under fixed seed: {PROPTEST_SEED_ENV}={effective_seed:#018x}"
    );

    let first_digest = stream_signature(effective_seed, PREDICATE_STREAM_DISCRIMINANT);
    let second_digest = stream_signature(effective_seed, PREDICATE_STREAM_DISCRIMINANT);
    assert_eq!(
        first_digest, second_digest,
        "predicate stream mismatch under fixed seed: {PROPTEST_SEED_ENV}={effective_seed:#018x}"
    );
}
