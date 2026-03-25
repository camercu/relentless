use core::time::Duration;
use tenacious::{RetryError, RetryExt, predicate, stop, wait};

fn main() {
    // Simulate a service that is permanently down.
    let ping_service = || -> Result<(), &str> { Err("control plane unavailable") };

    // Retry with logging and stats collection.
    let (result, stats) = ping_service
        .retry()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(10)))
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
    assert_eq!(stats.attempts, 3);
}
