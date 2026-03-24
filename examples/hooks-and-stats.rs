use core::cell::Cell;
use core::time::Duration;
use tenacious::{RetryError, RetryExt, predicate, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn main() {
    let attempts = Cell::new(0_u32);

    let (result, stats) = (|| {
        attempts.set(attempts.get().saturating_add(1));
        Err::<(), &'static str>("control plane unavailable")
    })
    .retry()
    .stop(stop::attempts(MAX_ATTEMPTS))
    .wait(wait::fixed(WAIT_DURATION))
    .when(predicate::any_error())
    .after_attempt(|state: &tenacious::AttemptState<(), &str>| {
        if let Err(error) = state.outcome {
            eprintln!("attempt {} failed: {error}", state.attempt);
        }
    })
    .sleep(|_dur: Duration| {})
    .with_stats()
    .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}
