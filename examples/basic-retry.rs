use core::cell::Cell;
use core::time::Duration;
use tenacious::{RetryExt, stop, wait};

fn main() {
    // Simulate an unreliable service that fails twice before succeeding.
    let failures_left = Cell::new(2_u32);
    let fetch_config = || -> Result<&str, &str> {
        if failures_left.get() > 0 {
            failures_left.set(failures_left.get() - 1);
            return Err("connection refused");
        }
        Ok("config_value=42")
    };

    // Retry up to 5 times, waiting 10 ms between attempts.
    let result = fetch_config
        .retry()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::from_millis(10)))
        .sleep(|_dur| {}) // no-op sleep for this demo
        .call();

    assert_eq!(result, Ok("config_value=42"));
}
