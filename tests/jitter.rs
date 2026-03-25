//! Acceptance tests for no_std and feature compatibility.

#[cfg(feature = "jitter")]
use core::time::Duration;
#[cfg(feature = "jitter")]
use std::cell::RefCell;
#[cfg(feature = "jitter")]
use tenacious::RetryPolicy;
#[cfg(feature = "jitter")]
use tenacious::Wait;
#[cfg(feature = "jitter")]
use tenacious::{stop, wait};

#[cfg(feature = "jitter")]
const BASE_WAIT: Duration = Duration::from_millis(20);
#[cfg(feature = "jitter")]
const MAX_JITTER: Duration = Duration::from_millis(10);
#[cfg(feature = "jitter")]
const WAIT_CAP: Duration = Duration::from_millis(25);
#[cfg(feature = "jitter")]
const SEEDED_NONCE_A: u64 = 7;
#[cfg(feature = "jitter")]
const SEEDED_NONCE_B: u64 = 8;
#[cfg(feature = "jitter")]
const SEEDED_ATTEMPT_COUNT: u32 = 8;
#[cfg(feature = "jitter")]
const SEEDED_JITTER_SEED: [u8; 32] = [0x11; 32];

#[cfg(feature = "jitter")]
fn state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, None)
}

#[cfg(feature = "jitter")]
#[test]
fn jitter_stays_within_expected_bounds() {
    let strategy = wait::fixed(BASE_WAIT).jitter(MAX_JITTER);
    let upper = BASE_WAIT.saturating_add(MAX_JITTER);

    for attempt in 1..=64 {
        let delay = strategy.next_wait(&state(attempt));
        assert!(delay >= BASE_WAIT);
        assert!(delay <= upper);
    }
}

#[cfg(feature = "jitter")]
#[test]
fn cap_is_applied_after_jitter_even_when_cap_called_first() {
    let capped_then_jittered = wait::fixed(BASE_WAIT).cap(WAIT_CAP).jitter(MAX_JITTER);

    for attempt in 1..=64 {
        let delay = capped_then_jittered.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[cfg(feature = "jitter")]
#[test]
fn jitter_then_cap_respects_cap() {
    let jittered_then_capped = wait::fixed(BASE_WAIT).jitter(MAX_JITTER).cap(WAIT_CAP);

    for attempt in 1..=64 {
        let delay = jittered_then_capped.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[cfg(feature = "jitter")]
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

#[cfg(feature = "jitter")]
#[test]
fn jitter_seed_and_nonce_make_sequence_reproducible() {
    let template = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_A);
    let first = template.clone();
    let second = template;

    for attempt in 1..=SEEDED_ATTEMPT_COUNT {
        assert_eq!(
            first.next_wait(&state(attempt)),
            second.next_wait(&state(attempt))
        );
    }
}

#[cfg(feature = "jitter")]
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
