use core::time::Duration;
use tenacious::{RetryError, RetryExt, predicate, stop, wait};

fn main() {
    // Represents a remote health check that is permanently unavailable.
    let ping_service = || -> Result<(), &str> { Err("control plane unavailable") };

    // `.with_stats()` wraps the builder so `.call()` returns (result, RetryStats).
    // Stats are useful for observability: emitting metrics or surfacing attempt counts
    // in error messages without threading a counter through the closure.
    let (result, stats) = ping_service
        .retry()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(5)))
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
