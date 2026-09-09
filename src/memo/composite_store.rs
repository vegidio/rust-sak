use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::disk_store::DiskStore;
use super::memory_store::MemoryStore;
use super::store::{Entry, Store};
use super::{CacheOpts, Result};

/// Memory in front of disk: reads try memory first, writes go to both.
#[derive(Debug)]
pub(super) struct CompositeStore {
    /// The front tier, consulted first and written to on every [`Store::set`].
    pub(super) memory: MemoryStore,
    /// The backing tier, consulted on a memory miss and written to on every [`Store::set`].
    pub(super) disk: Arc<DiskStore>,
    /// How long a disk hit is kept in memory. [`Duration::ZERO`] turns promotion off.
    promote_ttl: Duration,
}

impl CompositeStore {
    /// Builds a two-tier store over the disk store in `directory`.
    pub(super) fn open(directory: impl AsRef<Path>, opts: CacheOpts, promote_ttl: Duration) -> Result<Self> {
        Ok(CompositeStore {
            memory: MemoryStore::new(opts),
            disk: DiskStore::open(directory, opts)?,
            promote_ttl,
        })
    }
}

impl Store for CompositeStore {
    fn get(&self, key: &str) -> Result<Option<Entry>> {
        if let Some(entry) = self.memory.get(key)? {
            return Ok(Some(entry));
        }

        let Some(entry) = self.disk.get(key)? else {
            return Ok(None);
        };

        if !self.promote_ttl.is_zero() {
            // Never let the promoted copy outlive the entry it came from: a disk entry with 5 seconds left must not
            // be served from memory for an hour.
            let ttl = entry
                .remaining
                .map_or(self.promote_ttl, |remaining| self.promote_ttl.min(remaining));

            // Best-effort. A promotion that is declined or fails costs a future hit, nothing more, and must not stop
            // this hit being returned.
            let _ = self.memory.set(key, &entry.value, ttl);
        }

        Ok(Some(entry))
    }

    fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
        // Both tiers are always visited, so a failing one cannot stop the other being written.
        let disk = self.disk.set(key, value, ttl);
        let memory = self.memory.set(key, value, ttl);

        match (disk, memory) {
            (Ok(()), _) | (_, Ok(())) => Ok(()),
            // The disk error is reported in preference to the memory one: a memory failure costs a future hit, while
            // a disk failure is the one that actually loses the data.
            (Err(disk), Err(_)) => Err(disk),
        }
    }

    fn cleanup(&self) -> Result<()> {
        self.memory.cleanup()?;
        self.disk.cleanup()
    }

    fn path(&self) -> Option<&Path> {
        self.disk.path()
    }

    fn blocking(&self) -> bool {
        true
    }
}
