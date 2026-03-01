//! Acceptance tests for no_std and feature compatibility.

#[cfg(any(feature = "jitter", feature = "serde"))]
use core::time::Duration;
#[cfg(feature = "jitter")]
use std::cell::RefCell;
#[cfg(any(feature = "jitter", feature = "serde"))]
use tenacious::RetryPolicy;
#[cfg(any(feature = "jitter", feature = "serde"))]
use tenacious::Wait;
#[cfg(feature = "jitter")]
use tenacious::WaitExt;
#[cfg(any(feature = "jitter", feature = "serde"))]
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
#[cfg(feature = "serde")]
const SERIALIZED_ATTEMPTS_KEY: &str = "max";
#[cfg(feature = "serde")]
const INVALID_ATTEMPTS_VALUE: u64 = 0;
#[cfg(feature = "serde")]
const SUBUNIT_EXPONENTIAL_BASE: f64 = 0.5;

#[cfg(any(feature = "jitter", feature = "serde"))]
fn state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: None,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}

#[cfg(feature = "jitter")]
#[test]
fn jitter_stays_within_expected_bounds() {
    let mut strategy = wait::fixed(BASE_WAIT).jitter(MAX_JITTER);
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
    let mut capped_then_jittered = wait::fixed(BASE_WAIT).cap(WAIT_CAP).jitter(MAX_JITTER);

    for attempt in 1..=64 {
        let delay = capped_then_jittered.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[cfg(feature = "jitter")]
#[test]
fn jitter_then_cap_respects_cap() {
    let mut jittered_then_capped = wait::fixed(BASE_WAIT).jitter(MAX_JITTER).cap(WAIT_CAP);

    for attempt in 1..=64 {
        let delay = jittered_then_capped.next_wait(&state(attempt));
        assert!(delay <= WAIT_CAP);
    }
}

#[cfg(feature = "jitter")]
#[test]
fn jitter_sequence_changes_between_policy_invocations() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::fixed(Duration::ZERO).jitter(MAX_JITTER));

    let first: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let second: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|| Err::<(), _>("retry"))
        .sleep(|dur| first.borrow_mut().push(dur))
        .call();

    let _ = policy
        .retry(|| Err::<(), _>("retry"))
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
    let mut first = template.clone();
    let mut second = template;

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
    let mut first = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_A);
    let mut second = wait::fixed(BASE_WAIT)
        .jitter(MAX_JITTER)
        .with_seed(SEEDED_JITTER_SEED)
        .with_nonce(SEEDED_NONCE_B);

    let first_delay = first.next_wait(&state(1));
    let second_delay = second.next_wait(&state(1));
    assert_ne!(first_delay, second_delay);
}

#[cfg(feature = "serde")]
#[test]
fn retry_policy_serialization_omits_hooks() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(5)))
        .before_attempt(|_state| {})
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {});

    let value = serde_json::to_value(&policy).expect("policy should serialize");
    let object = value
        .as_object()
        .expect("serialized value should be an object");

    assert!(object.contains_key("stop"));
    assert!(object.contains_key("wait"));
    assert!(object.contains_key("predicate"));
    assert!(object.contains_key("predicate_is_default"));
    assert!(!object.contains_key("before_attempt"));
    assert!(!object.contains_key("after_attempt"));
    assert!(!object.contains_key("before_sleep"));
    assert!(!object.contains_key("on_exhausted"));
}

#[cfg(feature = "serde")]
#[test]
fn retry_policy_round_trips_without_hooks() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(5)));

    let json = serde_json::to_string(&policy).expect("policy should serialize");
    let mut decoded: RetryPolicy<
        tenacious::stop::StopAfterAttempts,
        tenacious::wait::WaitFixed,
        tenacious::on::AnyError,
    > = serde_json::from_str(&json).expect("policy should deserialize");

    let result = decoded
        .retry(|| Err::<(), _>("always fails"))
        .sleep(|_dur| {})
        .call();
    assert!(matches!(
        result,
        Err(tenacious::RetryError::Exhausted { attempts: 3, .. })
    ));
}

#[cfg(feature = "serde")]
#[test]
fn retry_policy_deserialization_rejects_zero_attempts() {
    let mut value = serde_json::to_value(
        RetryPolicy::new()
            .stop(stop::attempts(3))
            .wait(wait::fixed(Duration::from_millis(5))),
    )
    .expect("policy should serialize");
    value["stop"][SERIALIZED_ATTEMPTS_KEY] = serde_json::json!(INVALID_ATTEMPTS_VALUE);

    let decoded: Result<
        RetryPolicy<tenacious::stop::StopAfterAttempts, tenacious::wait::WaitFixed>,
        _,
    > = serde_json::from_value(value);
    assert!(decoded.is_err(), "zero attempts must fail deserialization");
}

#[cfg(feature = "serde")]
#[test]
fn wait_exponential_deserialization_clamps_subunit_base() {
    let value = serde_json::json!({
        "initial": Duration::from_millis(5),
        "base": SUBUNIT_EXPONENTIAL_BASE
    });
    let mut strategy: tenacious::wait::WaitExponential =
        serde_json::from_value(value).expect("wait::exponential should deserialize");

    let first = strategy.next_wait(&state(1));
    let second = strategy.next_wait(&state(2));
    assert_eq!(first, second, "base below 1.0 must clamp to 1.0");
}

#[cfg(all(feature = "serde", feature = "jitter"))]
#[test]
fn jitter_strategy_serializes_as_configuration() {
    let policy = RetryPolicy::new()
        .wait(wait::fixed(BASE_WAIT).jitter(MAX_JITTER))
        .stop(stop::attempts(2));

    let value = serde_json::to_value(&policy).expect("jitter policy should serialize");
    assert!(value.get("wait").is_some());

    let wait_value = value
        .get("wait")
        .expect("serialized policy should contain wait");
    let mut wait_strategy: tenacious::wait::WaitJitter<tenacious::wait::WaitFixed> =
        serde_json::from_value(wait_value.clone()).expect("wait jitter should deserialize");
    let next = wait_strategy.next_wait(&state(1));
    assert!(next >= BASE_WAIT);
    assert!(next <= BASE_WAIT.saturating_add(MAX_JITTER));
}
