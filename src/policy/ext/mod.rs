mod sync_builder;
pub use sync_builder::{RetryExt, SyncRetryBuilder, SyncRetryBuilderWithStats};

#[cfg(feature = "alloc")]
mod async_builder;
#[cfg(feature = "alloc")]
pub use async_builder::{AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt};
