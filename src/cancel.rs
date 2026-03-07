//! Cancellation support for retry loops.
//!
//! The [`Canceler`] trait allows external signals to interrupt a retry loop
//! between attempts. Cancellation is checked at three points:
//!
//! 1. Before starting a new attempt (including the very first).
//! 2. After sleeping between attempts.
//! 3. While sleeping in async retries, by racing against a cancellation future.
//!
//! # Implementations
//!
//! | Type | Feature gate | Async sleep interruption |
//! |------|-------------|--------------------------|
//! | [`NeverCancel`] | *(always)* | N/A (no cancellation). |
//! | `&AtomicBool` | *(always)* | **No.** Detected only at check-points 1 and 2. |
//! | `Arc<AtomicBool>` | `alloc` | **No.** Same as `&AtomicBool`. |
//! | `CancellationToken` | `tokio-cancel` | **Yes.** Wakes the sleep future immediately. |
//!
//! `AtomicBool`-based cancelers cannot wake a sleeping async future. During an
//! async sleep, cancellation is only detected when the sleep future itself
//! yields `Pending` and the poll falls through to the `is_cancelled()` check.
//! In practice, this means cancellation latency can be as long as the current
//! sleep duration. For latency-sensitive async cancellation, use
//! `CancellationToken` via the `tokio-cancel` feature.
//!
//! `Canceler` is **not** intended for trait-object use (`dyn Canceler`);
//! all implementations are concrete, zero-cost types.

use core::future::{Future, Pending, pending};
use core::sync::atomic::{AtomicBool, Ordering};

/// A source of cancellation signals for retry loops.
///
/// Implementors return `true` from [`is_cancelled`](Canceler::is_cancelled)
/// when the retry loop should abort. The check uses a cheap synchronous poll
/// rather than an async future, so it is usable in both sync and async engines.
pub trait Canceler {
    /// Future used by async retries to detect cancellation while sleeping.
    type Cancel: Future<Output = ()>;

    /// Returns `true` if cancellation has been requested.
    fn is_cancelled(&self) -> bool;

    /// Returns a future that resolves when cancellation is requested.
    ///
    /// Implementations that don't support wake-driven cancellation can return
    /// `core::future::pending()`.
    fn cancel(&self) -> Self::Cancel;
}

/// A canceler that never cancels. This is the default when no canceler
/// is configured, and compiles down to nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NeverCancel;

impl Canceler for NeverCancel {
    type Cancel = Pending<()>;

    #[inline(always)]
    fn is_cancelled(&self) -> bool {
        false
    }

    #[inline(always)]
    fn cancel(&self) -> Self::Cancel {
        pending()
    }
}

/// Convenience constructor for [`NeverCancel`].
#[inline]
#[must_use]
pub fn never() -> NeverCancel {
    NeverCancel
}

impl Canceler for &AtomicBool {
    type Cancel = Pending<()>;

    #[inline]
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }

    #[inline]
    fn cancel(&self) -> Self::Cancel {
        pending()
    }
}

#[cfg(feature = "alloc")]
impl Canceler for alloc::sync::Arc<AtomicBool> {
    type Cancel = Pending<()>;

    #[inline]
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }

    #[inline]
    fn cancel(&self) -> Self::Cancel {
        pending()
    }
}

#[cfg(feature = "tokio-cancel")]
impl Canceler for tokio_util::sync::CancellationToken {
    type Cancel = tokio_util::sync::WaitForCancellationFutureOwned;

    #[inline]
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    #[inline]
    fn cancel(&self) -> Self::Cancel {
        self.clone().cancelled_owned()
    }
}
