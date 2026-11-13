//! Observing a retry without hooks: log inside the operation, log each delay
//! by wrapping the wait strategy, and read `.call_with_stats()` after the loop ends.
//!
//! Run: `cargo run --example observing-retries`
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryState, Wait, retry, stop, wait};

/// Logs the delay the strategy picks for each retry. The engine asks the wait
/// strategy only once a retry is certain, so every line is a real retry. With
/// `.timeout(...)` set, the engine may sleep less: it clamps the delay to the
/// remaining budget.
struct Logged<W>(W);

impl<W: Wait> Wait for Logged<W> {
    fn next_wait(&self, state: &RetryState) -> Duration {
        let delay = self.0.next_wait(state);
        eprintln!("attempt {} failed; retrying in {delay:?}", state.attempt);
        delay
    }
}

fn main() {
    // Represents a remote health check that is permanently unavailable.
    let ping_service = || -> Result<(), &str> { Err("control plane unavailable") };

    // Per-attempt logging belongs in the operation: it sees each outcome
    // before the engine does.
    let observed_ping = || {
        let outcome = ping_service();
        if let Err(error) = &outcome {
            eprintln!("health check failed: {error}");
        }
        outcome
    };

    // `.call_with_stats()` returns (result, RetryStats):
    // attempt count, total wait and stop reason, without a counter threaded
    // through the closure.
    let (result, stats) = retry(|_| observed_ping())
        .stop(stop::attempts(3))
        .wait(Logged(wait::fixed(Duration::from_millis(5))))
        .clock(VirtualClock::new())
        .call_with_stats();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 3);
    eprintln!(
        "gave up after {} attempts: {}",
        stats.attempts, stats.stop_reason
    );
}
