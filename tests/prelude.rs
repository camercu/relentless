//! Verifies that `use relentless::prelude::*` brings the combinator
//! traits into scope so the builder DSL (`.cap()`, `.full_jitter()`, `.or()`,
//! `.and()`) resolves without importing each trait by name.

use core::time::Duration;
use relentless::prelude::*;
use relentless::{stop, wait};

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
