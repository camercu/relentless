use core::cell::Cell;
use core::time::Duration;
use relentless::{RetryExt, stop, wait};

fn main() {
    // Represents a remote call that fails transiently before eventually succeeding.
    // Using a counter here rather than a real network call keeps the example self-contained.
    let failures_left = Cell::new(2_u32);
    let fetch_config = || -> Result<&str, &str> {
        if failures_left.get() > 0 {
            failures_left.set(failures_left.get() - 1);
            return Err("connection refused");
        }
        Ok("config_value=42")
    };

    // The closure is the operation to retry. `.retry()` attaches a default policy;
    // `.stop()` and `.wait()` override individual strategy components.
    let result = fetch_config
        .retry()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::from_millis(10)))
        .sleep(|_dur| {}) // replaced with std::thread::sleep in production
        .call();

    assert_eq!(result, Ok("config_value=42"));
}
