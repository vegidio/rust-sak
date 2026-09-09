use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moka::Expiry;
use moka::sync::Cache;

use super::store::{Entry, Store};
use super::{CacheOpts, MemoError, Result};

/// One cached value and its deadline.
#[derive(Clone, Debug)]
struct MemoryEntry {
    /// The raw value. An [`Arc`] so moka's internal clones are pointer copies rather than allocations.
    value: Arc<[u8]>,
    /// When the entry stops being served, or `None` for one with no deadline.
    ///
    /// Monotonic, so unlike the disk tier's persisted deadline a wall-clock jump cannot disturb it.
    expires_at: Option<Instant>,
}

/// Gives every entry the TTL it was written with, rather than one deadline for the whole cache.
struct DeadlineExpiry;

impl Expiry<String, MemoryEntry> for DeadlineExpiry {
    fn expire_after_create(&self, _key: &String, value: &MemoryEntry, created_at: Instant) -> Option<Duration> {
        value.expires_at.map(|at| at.saturating_duration_since(created_at))
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &MemoryEntry,
        updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        value.expires_at.map(|at| at.saturating_duration_since(updated_at))
    }

    // `expire_after_read` is deliberately left at its default, which keeps the existing deadline: reading an entry
    // must not extend the TTL it was written with.
}

/// A cache that keeps everything in memory and nothing on disk.
#[derive(Debug)]
pub(super) struct MemoryStore {
    /// The entries. Values carry their own deadline, so expiry is decided in one place.
    cache: Cache<String, MemoryEntry>,
    /// The byte ceiling, kept so a value that could never fit can be refused rather than silently evicted.
    max_capacity: u64,
}

impl MemoryStore {
    /// Builds a store sized by `opts`.
    pub(super) fn new(opts: CacheOpts) -> Self {
        let cache = Cache::builder()
            .max_capacity(opts.max_capacity)
            .initial_capacity(usize::try_from(opts.max_entries).unwrap_or(usize::MAX))
            // Weigh entries by their length, which is what makes `max_capacity` a byte budget rather than a count.
            .weigher(|_key, entry: &MemoryEntry| u32::try_from(entry.value.len()).unwrap_or(u32::MAX))
            .expire_after(DeadlineExpiry)
            .build();

        MemoryStore {
            cache,
            max_capacity: opts.max_capacity,
        }
    }
}

impl Store for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Entry>> {
        let Some(entry) = self.cache.get(key) else {
            return Ok(None);
        };

        // Re-check the deadline rather than trusting moka to have reaped it already. Expiry there is applied by
        // periodic housekeeping, so an entry can outlive its TTL by a little; a memoizer must never serve one.
        let remaining = match entry.expires_at {
            Some(at) => match at.checked_duration_since(Instant::now()) {
                Some(remaining) if !remaining.is_zero() => Some(remaining),
                _ => return Ok(None),
            },
            None => None,
        };

        Ok(Some(Entry {
            value: entry.value.to_vec(),
            remaining,
        }))
    }

    fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
        // Both cases are writes that are provably lost, so they are reported rather than accepted: a zero TTL expires
        // the instant it lands, and a value over the whole budget would be admitted and then immediately evicted.
        if ttl.is_zero() || value.len() as u64 > self.max_capacity {
            return Err(MemoError::NotAdmitted);
        }

        let entry = MemoryEntry {
            value: Arc::from(value),
            expires_at: Instant::now().checked_add(ttl),
        };
        self.cache.insert(key.to_owned(), entry);

        Ok(())
    }

    fn cleanup(&self) -> Result<()> {
        // Nothing is on disk to reclaim; draining the pending work is all there is to do.
        self.cache.run_pending_tasks();
        Ok(())
    }

    fn path(&self) -> Option<&Path> {
        None
    }

    fn blocking(&self) -> bool {
        false
    }
}
