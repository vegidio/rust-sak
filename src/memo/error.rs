use std::fmt;
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
#[derive(Debug)]
pub enum MemoError {
    /// The `compute` closure failed. Nothing was cached and the next call retries.
    ///
    /// Held behind an [`Arc`] because every caller that joined the same in-flight computation is handed this exact
    /// error instance rather than a copy of it.
    Compute(Arc<dyn std::error::Error + Send + Sync + 'static>),
    /// The caller computing this key went away before publishing a usable result — it panicked, its future was
    /// dropped, or what it published could not be read back.
    ///
    /// Nothing was cached and the next call retries.
    ComputeAbandoned,
    /// The computed value could not be encoded for storage.
    Encode(postcard::Error),
    /// A filesystem operation on the cache directory failed.
    Io(std::io::Error),
    /// The cache declined the write. The value is simply not cached; nothing else is lost.
    NotAdmitted,
    /// The on-disk database could not be opened, read, written or compacted.
    Storage(redb::Error),
}

impl fmt::Display for MemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoError::Compute(err) => write!(f, "computing the value failed: {err}"),
            MemoError::ComputeAbandoned => f.write_str("the caller computing this value was abandoned"),
            MemoError::Encode(err) => write!(f, "the value could not be encoded: {err}"),
            MemoError::Io(err) => write!(f, "cache directory operation failed: {err}"),
            MemoError::NotAdmitted => f.write_str("the value was not admitted to the cache"),
            MemoError::Storage(err) => write!(f, "cache storage failed: {err}"),
        }
    }
}

impl std::error::Error for MemoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MemoError::Compute(err) => Some(&**err),
            MemoError::Encode(err) => Some(err),
            MemoError::Io(err) => Some(err),
            MemoError::Storage(err) => Some(err),
            MemoError::ComputeAbandoned | MemoError::NotAdmitted => None,
        }
    }
}

impl From<std::io::Error> for MemoError {
    fn from(err: std::io::Error) -> Self {
        MemoError::Io(err)
    }
}

impl From<postcard::Error> for MemoError {
    fn from(err: postcard::Error) -> Self {
        MemoError::Encode(err)
    }
}

impl From<redb::Error> for MemoError {
    fn from(err: redb::Error) -> Self {
        MemoError::Storage(err)
    }
}

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
