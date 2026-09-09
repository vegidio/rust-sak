use std::fmt;
use std::path::Path;
use std::time::Duration;

use super::Result;

/// One record read back out of a store.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    /// The raw value, exactly as the caller stored it.
    pub(super) value: Vec<u8>,
    /// How much longer the entry may be served, or `None` for one with no deadline.
    pub(super) remaining: Option<Duration>,
}

/// The byte-level cache a [`Memo`](super::Memo) sits on.
///
/// Deliberately crate-private, mirroring the Go package's `internal` store: the three shapes are reached through
/// [`Memo::memory`](super::Memo::memory), [`Memo::disk`](super::Memo::disk) and
/// [`Memo::memory_disk`](super::Memo::memory_disk), not by implementing this.
///
/// There is no `close`: dropping the last handle releases the store, which is what Go needs an explicit `Close` for.
pub(super) trait Store: fmt::Debug + Send + Sync {
    /// Reads `key`. `Ok(None)` covers a miss, an expired entry *and* a record that cannot be read back.
    fn get(&self, key: &str) -> Result<Option<Entry>>;

    /// Writes `value` under `key`, to be served for at most `ttl`.
    fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()>;

    /// Reclaims the space held by entries whose TTL has elapsed. Stores with nothing to reclaim return `Ok(())`.
    fn cleanup(&self) -> Result<()>;

    /// The directory backing this store, or `None` for one that keeps nothing on disk.
    fn path(&self) -> Option<&Path>;

    /// Whether calls into this store block on I/O.
    ///
    /// The async path routes blocking stores through `spawn_blocking` and calls the others inline, because a
    /// `spawn_blocking` round-trip costs more than a moka lookup does.
    fn blocking(&self) -> bool;
}
