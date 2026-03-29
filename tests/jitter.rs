//! Tests for additive jitter on wait strategies.
//!
//! Verifies that jitter stays within [base, base+max], that `.cap()` order does not
//! affect the cap invariant, and that each policy invocation and each clone produces
//! a distinct sequence (decorrelation). Seeded tests confirm reproducibility.

use core::time::Duration;
use std::cell::RefCell;
use tenacious::RetryPolicy;
use tenacious::Wait;
use tenacious::{stop, wait};

const BASE_WAIT: Duration = Duration::from_millis(20);
const MAX_JITTER: Duration = Duration::from_millis(10);
const WAIT_CAP: Duration = Duration::from_millis(25);
const SEEDED_NONCE_A: u64 = 7;
const SEEDED_NONCE_B: u64 = 8;
const SEEDED_ATTEMPT_COUNT: u32 = 8;
const SEEDED_JITTER_SEED: u64 = 0x11;

fn state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, None)
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
