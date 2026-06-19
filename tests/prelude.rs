//! Verifies that `use relentless::prelude::*` brings the extension/combinator
//! traits into scope so the builder DSL (`.cap()`, `.full_jitter()`, `.or()`,
//! `.and()`, `.retry()`) resolves without importing each trait by name.

use core::time::Duration;
use relentless::prelude::*;
use relentless::{predicate, stop, wait};

const ARBITRARY_DURATION: Duration = Duration::from_millis(10);

#[test]
fn prelude_enables_wait_combinators() {
    // `.full_jitter()` and `.cap()` are `Wait` methods; resolved via prelude.
    let _strategy = wait::exponential(ARBITRARY_DURATION)
        .full_jitter()
        .cap(Duration::from_secs(1));
}

#[test]
fn prelude_enables_stop_combinators() {
    // `.or()` / `.and()` are `Stop` methods; resolved via prelude.
    let _strategy = stop::attempts(3).or(stop::elapsed(Duration::from_secs(1)));
}

#[test]
fn prelude_enables_predicate_combinators() {
    // `.or()` is a `Predicate` method; resolved via prelude. Use an
    // `E`-pinning matcher as the receiver so the named method isn't ambiguous.
    let pred = predicate::error(|e: &&str| *e == "boom").or(predicate::ok(|v: &u32| *v < 2));
    assert!(pred.should_retry(&Err::<u32, &str>("boom")));
    assert!(!pred.should_retry(&Err::<u32, &str>("fatal")));
    assert!(pred.should_retry(&Ok::<u32, &str>(1)));
    assert!(!pred.should_retry(&Ok::<u32, &str>(5)));
}

#[test]
fn prelude_enables_retry_ext() {
    // `.retry()` is the `RetryExt` method on closures; resolved via prelude.
    let result = (|| Ok::<u32, &str>(7)).retry().sleep(|_| {}).call();
    assert_eq!(result.unwrap(), 7);
}
