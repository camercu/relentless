//! The defaults every entry point starts from.
//!
//! `retry`, `retry_async`, the `RetryExt`/`AsyncRetryExt` closure extensions and
//! `RetryPolicy::new` must all begin from the same configuration. A consumer who
//! compares two entry points will notice if they disagree. The default stop and
//! wait strategies are built here once, so that agreement is structural rather
//! than three copies kept in step by hand.

use crate::compat::Duration;
use crate::stop::{self, StopAfterAttempts};
use crate::wait::{self, WaitExponential};

/// Attempts allowed before the default stop strategy gives up.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// First inter-attempt delay of the default exponential backoff.
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// The stop strategy every entry point starts from.
pub(crate) fn stop() -> StopAfterAttempts {
    stop::attempts(DEFAULT_MAX_ATTEMPTS)
}

/// The wait strategy every entry point starts from.
pub(crate) fn wait() -> WaitExponential {
    wait::exponential(DEFAULT_INITIAL_WAIT)
}
