//! Cancellation support for retry loops.
//!
//! The [`Canceler`] trait allows external signals to interrupt a retry loop
//! between attempts. Cancellation is checked at two points:
//!
//! 1. Before starting a new attempt (including the very first).
//! 2. After sleeping between attempts.
//! 3. While sleeping in async retries, by polling a cancellation future.
//!
//! # Implementations
//!
//! | Type | Feature gate | Notes |
//! |------|-------------|-------|
//! | [`NeverCancel`] | *(always)* | Zero-cost no-op; the default. |
//! | `&AtomicBool` | *(always)* | Shared flag; `Acquire` load. |
//! | `Arc<AtomicBool>` | `alloc` | Owned shared flag. |
//! | `CancellationToken` | `tokio-cancel` | Tokio-util token. |
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
#[derive(Debug, Clone, Copy)]
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
