use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use tenacious::{RetryError, RetryExt, predicate, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn main() {
    let cancelled = AtomicBool::new(false);
    let attempts = Cell::new(0_u32);

    let (result, stats) = (|_: tenacious::RetryState| {
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
    .sleep(|_dur: Duration| {
        cancelled.store(true, Ordering::Relaxed);
    })
    .cancel_on(&cancelled)
    .with_stats()
    .call();

    assert!(matches!(result, Err(RetryError::Cancelled { .. })));
    assert_eq!(stats.attempts, 1);
    assert_eq!(attempts.get(), 1);
}
