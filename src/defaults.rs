//! The defaults every entry point starts from.
//!
//! `retry`, `retry_async`, the `RetryExt`/`AsyncRetryExt` closure extensions and
//! `RetryPolicy::new` must all begin from the same configuration — SPEC 6.1.1
//! says so, and a consumer who compares two entry points will notice if they
//! disagree. These constants are declared once so that agreement is structural
//! rather than three copies kept in step by hand.

use crate::compat::Duration;

/// Attempts allowed before the default stop strategy gives up.
pub(crate) const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// First inter-attempt delay of the default exponential backoff.
pub(crate) const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);
