use core::cell::Cell;
use core::time::Duration;
use relentless::{RetryPolicy, predicate, sleep, stop, wait};

#[tokio::main]
async fn main() {
    // Represents a remote status endpoint that returns "deploying" until the service
    // is up. Using a counter keeps the example self-contained.
    let checks_remaining = Cell::new(3_u32);
    let check_deploy_status = |_| {
        let status = if checks_remaining.get() > 0 {
            checks_remaining.set(checks_remaining.get() - 1);
            "deploying"
        } else {
            "ready"
        };
        async move { Ok::<_, &str>(status) }
    };

    // `.until()` inverts the predicate: retry while the Ok value does NOT match.
    // This is more readable than `.when(predicate::ok(|s| *s != "ready"))`.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(25)))
        .until(predicate::ok(|status: &&str| *status == "ready"));

    let result = policy
        .retry_async(check_deploy_status)
        .sleep(sleep::tokio())
        .await;

    assert_eq!(result, Ok("ready"));
}
