//! Cancellation support for retry loops.
//!
//! The cancellation system is split into two traits:
//!
//! - [`Canceler`] provides synchronous cancellation checking via
//!   [`is_cancelled`](Canceler::is_cancelled). Used by sync retry.
//! - [`AsyncCanceler`] extends `Canceler` with a wake-driven cancellation
//!   future. Used by async retry.
//!
//! This split ensures that poll-based cancelers (which cannot wake during
//! sleep) produce a type error when passed to async retry, rather than
//! silently degrading to checkpoint-only detection.
//!
//! For users who genuinely want poll-based cancellation in async contexts,
//! [`PolledCanceler`] wraps any `Canceler` for async use.
//!
//! # Implementations
//!
//! | Type | Feature gate | `Canceler` | `AsyncCanceler` |
//! |------|-------------|------------|-----------------|
//! | [`CancelNever`] | *(always)* | yes | yes |
//! | `&AtomicBool` | *(always)* | yes | no |
//! | `Arc<AtomicBool>` | `alloc` | yes | no |
//! | `CancellationToken` | `tokio-cancel` | yes | yes |
//! | [`PolledCanceler<C>`] | *(always)* | yes | yes |

use core::future::{Future, Pending, pending};
use core::sync::atomic::{AtomicBool, Ordering};

/// A source of cancellation signals for retry loops (sync).
///
/// Implementors return `true` from [`is_cancelled`](Canceler::is_cancelled)
/// when the retry loop should abort. The check uses a cheap synchronous poll
/// rather than an async future, so it is usable in both sync and async engines.
pub trait Canceler {
    /// Returns `true` if cancellation has been requested.
    fn is_cancelled(&self) -> bool;
}

/// Async cancellation trait extending [`Canceler`] with a wake-driven future.
///
/// Async retry uses this trait so that cancelers can interrupt sleep futures
/// promptly. Sync-only cancelers (like `&AtomicBool`) intentionally do not
/// implement this trait — use [`PolledCanceler`] to opt in to checkpoint-only
/// async cancellation.
pub trait AsyncCanceler: Canceler {
    /// Future used by async retries to detect cancellation while sleeping.
    type Cancel: Future<Output = ()>;

    /// Returns a future that resolves when cancellation is requested.
    ///
    /// Implementations that support wake-driven cancellation should return a
    /// future that completes when the cancellation signal fires.
    fn cancel(&self) -> Self::Cancel;
}

/// A canceler that never cancels. This is the default when no canceler
/// is configured, and compiles down to nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CancelNever;

impl Canceler for CancelNever {
    #[inline(always)]
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl AsyncCanceler for CancelNever {
    type Cancel = Pending<()>;

    #[inline(always)]
    fn cancel(&self) -> Self::Cancel {
        pending()
    }
}

/// Convenience constructor for [`CancelNever`].
#[inline]
#[must_use]
pub fn never() -> CancelNever {
    CancelNever
}

impl Canceler for &AtomicBool {
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}

#[cfg(feature = "alloc")]
impl Canceler for alloc::sync::Arc<AtomicBool> {
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}

#[cfg(feature = "tokio-cancel")]
impl Canceler for tokio_util::sync::CancellationToken {
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

#[cfg(feature = "tokio-cancel")]
impl AsyncCanceler for tokio_util::sync::CancellationToken {
    type Cancel = tokio_util::sync::WaitForCancellationFutureOwned;

    #[inline]
    fn cancel(&self) -> Self::Cancel {
        self.clone().cancelled_owned()
    }
}

/// Wraps a sync-only [`Canceler`] for use in async contexts.
///
/// Cancellation is detected only at checkpoints (before each attempt and
/// after each sleep completes), not during sleep. The `cancel()` future
/// is permanently pending.
///
/// # Examples
///
/// ```
/// use core::sync::atomic::AtomicBool;
/// use tenacious::cancel::{PolledCanceler, Canceler};
///
/// let flag = AtomicBool::new(false);
/// let canceler = PolledCanceler(&flag);
/// assert!(!canceler.is_cancelled());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PolledCanceler<C: Canceler>(pub C);

impl<C: Canceler> Canceler for PolledCanceler<C> {
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

impl<C: Canceler> AsyncCanceler for PolledCanceler<C> {
    type Cancel = Pending<()>;

    #[inline]
    fn cancel(&self) -> Self::Cancel {
        pending()
    }
}
