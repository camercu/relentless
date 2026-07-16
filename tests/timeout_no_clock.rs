//! The timeout-without-clock diagnostic (SPEC 11.2) fires only in `no_std`
//! builds, where `ElapsedTracker` has no fallback clock and a timeout set
//! without `.elapsed_clock(...)` would silently never fire.
//!
//! This is the one configuration where `debug_assert_timeout_has_clock` can
//! trip: under `std` the fallback `Instant` clock makes the asserted condition
//! unconditionally true, so the assertion is unreachable there (and therefore
//! mutation-untestable — see the `exclude_re` note in `.cargo/mutants.toml`).
//! Gated to `no_std` + debug so it neither compiles under the default `std`
//! build nor runs in release, where the assertion compiles out.
#![cfg(all(not(feature = "std"), debug_assertions))]

use core::time::Duration;

use relentless::RetryPolicy;

const ARBITRARY_TIMEOUT: Duration = Duration::from_millis(10);

#[test]
#[should_panic(expected = "timeout configured without an elapsed clock")]
fn timeout_without_clock_panics_in_debug() {
    // The assertion fires at loop entry, before the first attempt, so the
    // operation's outcome is irrelevant — reaching `.call()` is enough.
    let _ = RetryPolicy::new()
        .retry(|_| Ok::<i32, &str>(0))
        .sleep(|_| {})
        .timeout(ARBITRARY_TIMEOUT)
        .call();
}
