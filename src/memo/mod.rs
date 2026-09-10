//! Memoization with pluggable storage.
//!
//! Wrap a computation that is expensive to repeat — an HTTP request, a database query, a hardware probe that shells
//! out — and [`Memo`] will cache its result under a key for as long as the TTL you give it, deduplicate concurrent
//! calls for that key so the work happens once, and optionally keep the result on disk so it survives a restart.
//!
//! Everything here is **synchronous** unless the `memo-async` feature is on, which adds
//! [`get_or_compute_async`](Memo::get_or_compute_async) for computations that are themselves `async`. The store is
//! the same either way, so the two can be mixed.
//!
//! See `README.md` in this module for the guarantees in full — what is cached, what is not, and what happens when a
//! store breaks.
//!
//! ```
//! use std::time::Duration;
//! use rust_sak::memo::{CacheOpts, Memo};
//!
//! let memo = Memo::memory(CacheOpts::new())?;
//!
//! let answer: u32 = memo.get_or_compute("answer", Duration::from_secs(60), || {
//!     Ok::<_, std::convert::Infallible>(6 * 7)
//! })?;
//!
//! assert_eq!(answer, 42);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod cache_opts;
mod composite_store;
mod disk_store;
mod error;
mod flight;
#[cfg(feature = "memo-async")]
mod get_or_compute_async;
mod header;
mod key_builder;
mod key_from;
mod memory_store;
mod store;

pub use cache_opts::CacheOpts;
pub use error::{MemoError, Result};
pub use key_builder::KeyBuilder;
pub use key_from::key_from;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use composite_store::CompositeStore;
use disk_store::DiskStore;
use flight::{Flight, Outcome};
use memory_store::MemoryStore;
use store::Store;

/// A cache handle: a store, plus the computations currently running against it.
///
/// Cloning is cheap, and every clone shares the same store *and* the same in-flight table — so build one and clone it
/// to wherever it is needed. Two separately constructed handles for one directory share the store but not the
/// deduplication, so they can compute the same key twice.
///
/// The cache is released when the last handle is dropped. There is no `close`.
#[derive(Clone, Debug)]
pub struct Memo {
    /// The backing store: memory, disk, or memory in front of disk.
    store: Arc<dyn Store>,
    /// The computations currently running, so that many callers asking for one missing key compute it once.
    flight: Arc<Flight>,
}

impl Memo {
    /// Opens a cache that keeps everything in memory and nothing on disk.
    ///
    /// ```
    /// use rust_sak::memo::{CacheOpts, Memo};
    ///
    /// let memo = Memo::memory(CacheOpts::new().max_capacity(256 << 20))?;
    ///
    /// assert!(memo.path().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Never, today. It returns a [`Result`] so that gaining a fallible step later is not a breaking change.
    pub fn memory(opts: CacheOpts) -> Result<Self> {
        Ok(Memo::with_store(Arc::new(MemoryStore::new(opts))))
    }

    /// Opens a cache backed by a database in `directory`, creating the directory if it does not exist.
    ///
    /// A directory this process already has open yields the **same** store rather than a second one, so `opts` is
    /// honoured only by whichever call opens it first. That is what makes opening a path twice safe: the database
    /// takes an exclusive file lock, and a plain re-open would fail with an error indistinguishable from another
    /// process holding it.
    ///
    /// # Errors
    ///
    /// Returns [`MemoError::Io`] if `directory` cannot be created or resolved, and [`MemoError::Storage`] if the
    /// database cannot be opened.
    pub fn disk(directory: impl AsRef<Path>, opts: CacheOpts) -> Result<Self> {
        Ok(Memo::with_store(DiskStore::open(directory, opts)?))
    }

    /// Opens a two-tier cache: memory in front of the database in `directory`.
    ///
    /// Reads try memory first and fall back to disk; a disk hit is copied back into memory for `promote_ttl`, or for
    /// the entry's own remaining lifetime if that is shorter. Writes go to both tiers. A `promote_ttl` of
    /// [`Duration::ZERO`] turns promotion off, so disk hits are served without being cached in memory.
    ///
    /// # Errors
    ///
    /// As [`Memo::disk`].
    pub fn memory_disk(directory: impl AsRef<Path>, opts: CacheOpts, promote_ttl: Duration) -> Result<Self> {
        Ok(Memo::with_store(Arc::new(CompositeStore::open(
            directory,
            opts,
            promote_ttl,
        )?)))
    }

    /// Wraps `store` in a fresh handle with an empty in-flight table.
    fn with_store(store: Arc<dyn Store>) -> Self {
        Memo {
            store,
            flight: Arc::new(Flight::default()),
        }
    }

    /// The directory backing this cache, or `None` for one that keeps nothing on disk.
    ///
    /// The path is canonical, so it may differ from the one passed to the constructor.
    pub fn path(&self) -> Option<&Path> {
        self.store.path()
    }

    /// Reclaims the storage still held by entries whose TTL has elapsed.
    ///
    /// Expired entries are never served, but on disk the space they occupy is not returned to the filesystem on its
    /// own. A disk-backed cache already runs this once in the background when it opens, so calling it is optional —
    /// do so to reclaim space at a chosen moment, such as before shutting down or after a large batch expires.
    ///
    /// Prefer a quiet moment: the sweep briefly excludes readers while it compacts. It is a no-op for a memory-only
    /// cache, and a call made while another sweep is running returns immediately rather than queueing behind it.
    ///
    /// # Errors
    ///
    /// Returns [`MemoError::Storage`] if the sweep or the compaction fails.
    pub fn cleanup(&self) -> Result<()> {
        self.store.cleanup()
    }

    /// Reads the raw bytes stored under `key`.
    ///
    /// Returns `None` for a miss, for an entry whose TTL has elapsed, and for a record that cannot be read back.
    /// These bytes are stored verbatim — unlike [`get_or_compute`](Memo::get_or_compute), which frames values with a
    /// header of its own — so this and [`set_bytes`](Memo::set_bytes) round-trip exactly.
    ///
    /// # Errors
    ///
    /// Returns [`MemoError::Storage`] if the store fails.
    pub fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.store.get(key)?.map(|entry| entry.value.to_vec()))
    }

    /// Writes raw bytes under `key`, to be served for at most `ttl`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoError::NotAdmitted`] if the cache declined the write — a zero `ttl`, or a value larger than the
    /// whole memory budget — and [`MemoError::Storage`] if the store failed.
    pub fn set_bytes(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
        self.store.set(key, Arc::from(value), ttl)
    }

    /// Returns the value cached under `key`, running `compute` only when there is not one.
    ///
    /// Concurrent callers asking for the same missing key are coalesced: `compute` runs once and every caller
    /// receives that result. A failure is not cached — it reaches every waiter and the next call retries — and a
    /// cache write that fails does not stop the computed value being returned, so a broken cache degrades to no
    /// cache rather than breaking the call path.
    ///
    /// ```
    /// use std::time::Duration;
    /// use rust_sak::memo::{CacheOpts, Memo};
    ///
    /// let memo = Memo::memory(CacheOpts::new())?;
    /// let ttl = Duration::from_secs(300);
    ///
    /// let first: String = memo.get_or_compute("greeting", ttl, || expensive())?;
    /// let second: String = memo.get_or_compute("greeting", ttl, || expensive())?;
    ///
    /// assert_eq!(first, second);
    ///
    /// fn expensive() -> Result<String, std::convert::Infallible> {
    ///     Ok("hello".to_owned())
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// This method blocks. Calling it from inside an async task can park a runtime worker for as long as `compute`
    /// takes — use [`get_or_compute_async`](Memo::get_or_compute_async) there, or `tokio::task::spawn_blocking`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoError::Compute`] if `compute` failed, [`MemoError::Encode`] if its result could not be encoded,
    /// and [`MemoError::ComputeAbandoned`] if the caller that was computing this key went away without publishing a
    /// usable result.
    pub fn get_or_compute<T, F, E>(&self, key: &str, ttl: Duration, compute: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> std::result::Result<T, E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let fingerprint = header::fingerprint::<T>();

        if let Some((_, value)) = self.load_raw(key, fingerprint) {
            return Ok(value);
        }

        let mut leader = match self.flight.enter(flight_key(fingerprint, key)) {
            flight::Entry::Follower(call) => return resolve(call.wait(), fingerprint),
            flight::Entry::Leader(leader) => leader,
        };

        // Look again now that we hold the flight: a caller that arrived just after the previous leader finished
        // should be served from the cache rather than recompute what is already there.
        if let Some((bytes, value)) = self.load_raw(key, fingerprint) {
            leader.publish(Outcome::Ready(bytes));
            return Ok(value);
        }

        let value = match compute() {
            Ok(value) => value,
            Err(err) => return Err(publish_compute_error(&mut leader, err)),
        };

        let bytes = encode_or_publish(&mut leader, fingerprint, &value)?;

        // Best-effort by design: a store that cannot take this value costs a future hit, nothing more.
        let _ = self.store.set(key, Arc::clone(&bytes), ttl);

        leader.publish(Outcome::Ready(bytes));
        Ok(value)
    }

    /// Reads and decodes the entry under `key`, handing back the stored bytes alongside the value so a leader can
    /// publish them to its followers. `None` when there is nothing this caller can use.
    ///
    /// A store failure is a miss, exactly as an absent key is: a cache that cannot be read must not break the call
    /// path it is meant to speed up.
    fn load_raw<T>(&self, key: &str, fingerprint: u64) -> Option<(Arc<[u8]>, T)>
    where
        T: DeserializeOwned,
    {
        decode_entry(fingerprint, self.store.get(key).ok().flatten()?.value)
    }
}

/// Decodes stored bytes, handing them back alongside the value.
///
/// The bytes travel with the value because a leader publishes them to its followers verbatim, so every caller gets
/// byte-identical data. Shared with the async loader, which differs only in how it reads the entry.
fn decode_entry<T>(fingerprint: u64, bytes: Arc<[u8]>) -> Option<(Arc<[u8]>, T)>
where
    T: DeserializeOwned,
{
    let value = header::decode(fingerprint, &bytes)?;

    Some((bytes, value))
}

/// The key one computation is deduplicated under.
///
/// The type fingerprint is part of it, so two calls that share a cache key but expect different types are separate
/// computations and cannot be handed each other's bytes.
fn flight_key(fingerprint: u64, key: &str) -> String {
    format!("{fingerprint:016x}\0{key}")
}

/// Encodes `value`, publishing the failure to the followers if it cannot be encoded.
///
/// Shared by the synchronous and async leaders so the publish protocol — which [`Outcome`] goes out on which path —
/// is stated once. Nothing here awaits, which is what lets both paths use it.
fn encode_or_publish<T>(leader: &mut flight::Leader<'_>, fingerprint: u64, value: &T) -> Result<Arc<[u8]>>
where
    T: Serialize,
{
    match header::encode(fingerprint, value) {
        Ok(bytes) => Ok(Arc::from(bytes)),
        Err(err) => {
            leader.publish(Outcome::Encode(err.clone()));
            Err(MemoError::Encode(err))
        }
    }
}

/// Publishes a failed computation to the followers and returns the error the leader itself should report.
///
/// As [`encode_or_publish`], shared so that both paths hand followers the very same error instance.
fn publish_compute_error<E>(leader: &mut flight::Leader<'_>, err: E) -> MemoError
where
    E: std::error::Error + Send + Sync + 'static,
{
    let err: Arc<dyn std::error::Error + Send + Sync> = Arc::new(err);
    leader.publish(Outcome::Compute(Arc::clone(&err)));
    MemoError::Compute(err)
}

/// Turns what a leader published into this caller's result.
fn resolve<T>(outcome: Outcome, fingerprint: u64) -> Result<T>
where
    T: DeserializeOwned,
{
    match outcome {
        Outcome::Ready(bytes) => header::decode(fingerprint, &bytes).ok_or(MemoError::ComputeAbandoned),
        other => Err(other.into_error().unwrap_or(MemoError::ComputeAbandoned)),
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
