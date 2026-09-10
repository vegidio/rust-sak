use std::sync::Arc;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, MemoError>;

/// An error produced by a cache operation.
///
/// Note what is deliberately *not* here. A store that fails to read is a miss, not an error; an entry whose bytes
/// cannot be decoded is a miss, not an error; and a cache write that fails does not stop
/// [`get_or_compute`](super::Memo::get_or_compute) from returning the value it just computed. Only
/// [`get_bytes`](super::Memo::get_bytes), [`set_bytes`](super::Memo::set_bytes), [`cleanup`](super::Memo::cleanup)
/// and the constructors surface [`MemoError::Storage`], because those are the calls whose whole purpose is the store.
#[derive(Debug, thiserror::Error)]
pub enum MemoError {
    /// The `compute` closure failed. Nothing was cached and the next call retries.
    ///
    /// Held behind an [`Arc`] because every caller that joined the same in-flight computation is handed this exact
    /// error instance rather than a copy of it.
    #[error("computing the value failed: {0}")]
    Compute(Arc<dyn std::error::Error + Send + Sync + 'static>),
    /// The caller computing this key went away before publishing a usable result — it panicked, its future was
    /// dropped, or what it published could not be read back.
    ///
    /// Nothing was cached and the next call retries.
    #[error("the caller computing this value was abandoned")]
    ComputeAbandoned,
    /// The computed value could not be encoded for storage.
    #[error("the value could not be encoded: {0}")]
    Encode(#[from] postcard::Error),
    /// A filesystem operation on the cache directory failed.
    #[error("cache directory operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// The cache declined the write. The value is simply not cached; nothing else is lost.
    #[error("the value was not admitted to the cache")]
    NotAdmitted,
    /// The on-disk database could not be opened, read, written or compacted.
    #[error("cache storage failed: {0}")]
    Storage(#[from] redb::Error),
}

// `#[from]` generates one conversion per variant, keyed on the field's type, so it covers `redb::Error` itself.
// The six conversions below funnel *different* redb error types into that same `Storage` variant, which the derive
// cannot express — they stay hand-written.

impl From<redb::DatabaseError> for MemoError {
    fn from(err: redb::DatabaseError) -> Self {
        MemoError::Storage(err.into())
    }
}

impl From<redb::TransactionError> for MemoError {
    fn from(err: redb::TransactionError) -> Self {
        MemoError::Storage(err.into())
    }
}

impl From<redb::TableError> for MemoError {
    fn from(err: redb::TableError) -> Self {
        MemoError::Storage(err.into())
    }
}

impl From<redb::StorageError> for MemoError {
    fn from(err: redb::StorageError) -> Self {
        MemoError::Storage(err.into())
    }
}

impl From<redb::CommitError> for MemoError {
    fn from(err: redb::CommitError) -> Self {
        MemoError::Storage(err.into())
    }
}

impl From<redb::CompactionError> for MemoError {
    fn from(err: redb::CompactionError) -> Self {
        MemoError::Storage(err.into())
    }
}
