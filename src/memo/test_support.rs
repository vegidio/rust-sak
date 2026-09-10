//! Helpers shared by this module's tests.
//!
//! The two stores here exist because the interesting guarantees are about what happens when a store *misbehaves* —
//! a write it drops, a read it cannot serve — and a real database is very hard to persuade to do either on demand.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::store::{Entry, Store};
use super::{MemoError, Result};

/// A store that fails every read and every write, and counts both.
#[derive(Debug, Default)]
pub(super) struct FailingStore {
    /// How many reads have been attempted.
    pub(super) gets: AtomicUsize,
    /// How many writes have been attempted.
    pub(super) sets: AtomicUsize,
}

impl Store for FailingStore {
    fn get(&self, _key: &str) -> Result<Option<Entry>> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        Err(MemoError::Io(std::io::Error::other("this store is broken on purpose")))
    }

    fn set(&self, _key: &str, _value: Arc<[u8]>, _ttl: Duration) -> Result<()> {
        self.sets.fetch_add(1, Ordering::Relaxed);
        Err(MemoError::Io(std::io::Error::other("this store is broken on purpose")))
    }

    fn cleanup(&self) -> Result<()> {
        Ok(())
    }

    fn path(&self) -> Option<&Path> {
        None
    }
}

/// Wraps another store and counts the calls that reach it.
#[derive(Debug)]
pub(super) struct CountingStore {
    /// The store doing the actual work.
    inner: Arc<dyn Store>,
    /// How many reads have reached `inner`.
    pub(super) gets: AtomicUsize,
    /// How many writes have reached `inner`.
    pub(super) sets: AtomicUsize,
}

impl CountingStore {
    /// Wraps `inner`.
    pub(super) fn new(inner: Arc<dyn Store>) -> Self {
        CountingStore {
            inner,
            gets: AtomicUsize::new(0),
            sets: AtomicUsize::new(0),
        }
    }
}

impl Store for CountingStore {
    fn get(&self, key: &str) -> Result<Option<Entry>> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: Arc<[u8]>, ttl: Duration) -> Result<()> {
        self.sets.fetch_add(1, Ordering::Relaxed);
        self.inner.set(key, value, ttl)
    }

    fn cleanup(&self) -> Result<()> {
        self.inner.cleanup()
    }

    fn path(&self) -> Option<&Path> {
        self.inner.path()
    }
}

/// Wraps a byte slice as the reference-counted payload [`Store::set`] takes.
pub(super) fn bytes(value: impl AsRef<[u8]>) -> Arc<[u8]> {
    Arc::from(value.as_ref())
}

/// The error type the test computations fail with.
#[derive(Debug)]
pub(super) struct ComputeFailed;

impl std::fmt::Display for ComputeFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the computation failed")
    }
}

impl std::error::Error for ComputeFailed {}

/// A TTL long enough that nothing expires mid-test.
pub(super) const LONG: Duration = Duration::from_secs(300);
