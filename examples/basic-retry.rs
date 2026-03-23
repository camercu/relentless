use core::cell::Cell;
use core::time::Duration;
use tenacious::{RetryExt, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn main() {
    let attempts = Cell::new(0_u32);

    let result = (|_: tenacious::RetryState| {
        attempts.set(attempts.get().saturating_add(1));
        if attempts.get() < MAX_ATTEMPTS {
            Err::<&'static str, &'static str>("transient")
        } else {
            Ok("ready")
        }
    })
    .retry()
    .stop(stop::attempts(MAX_ATTEMPTS))
    .wait(wait::fixed(WAIT_DURATION))
    .sleep(|_dur| {})
    .call();

    assert_eq!(result, Ok("ready"));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}
