use core::cell::Cell;
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};

#[tokio::main]
async fn main() {
    // Simulate a deployment that takes a few checks before it's ready.
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

    // Poll until the service reports "ready", checking every 25 ms.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(25)))
        .until(predicate::ok(|status: &&str| *status == "ready"));

    let result = policy
        .retry_async(check_deploy_status)
        .sleep(tenacious::sleep::tokio())
        .await;

    assert_eq!(result, Ok("ready"));
}
