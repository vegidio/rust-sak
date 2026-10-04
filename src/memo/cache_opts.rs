use std::time::Duration;

/// Sizing hints and the write-durability cadence for a store.
///
/// The sizes are hints rather than hard limits, and both are clamped to whatever the underlying engine accepts, so no
/// value here can stop a store from opening.
///
/// ```
/// use std::time::Duration;
/// use rust_sak::memo::CacheOpts;
///
/// let opts = CacheOpts::new()
///     .max_entries(100_000)
///     .max_capacity(256 << 20)
///     .flush_every(64)
///     .flush_interval(Duration::from_secs(1));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheOpts {
    /// Roughly how many entries the store is sized for.
    pub(super) max_entries: u64,
    /// Roughly how many bytes the store is sized for.
    pub(super) max_capacity: u64,
    /// A disk write is made durable at least once every this many writes. Never zero.
    pub(super) flush_every: u32,
    /// A disk write is also made durable once this long has passed since the last durable one. Zero turns it off.
    pub(super) flush_interval: Duration,
}

/// Room for about 10,000 entries and 1 GiB.
const DEFAULT_MAX_ENTRIES: u64 = 10_000;
const DEFAULT_MAX_CAPACITY: u64 = 1 << 30;

/// Every disk write durable, as it was before the cadence existed.
const DEFAULT_FLUSH_EVERY: u32 = 1;

impl Default for CacheOpts {
    fn default() -> Self {
        CacheOpts {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_capacity: DEFAULT_MAX_CAPACITY,
            flush_every: DEFAULT_FLUSH_EVERY,
            flush_interval: Duration::ZERO,
        }
    }
}

impl CacheOpts {
    /// The defaults: room for about 10,000 entries and 1 GiB, and every disk write durable.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sizes the store for roughly this many entries. Zero restores the default.
    ///
    /// This shapes the memory tier only. The disk tier has no per-entry sizing to map it onto, and ignores it.
    pub fn max_entries(mut self, entries: u64) -> Self {
        self.max_entries = if entries == 0 { DEFAULT_MAX_ENTRIES } else { entries };
        self
    }

    /// Sizes the store for roughly this many bytes. Zero restores the default.
    ///
    /// For the memory tier this is a real ceiling: entries are weighed by their encoded length and evicted to stay
    /// under it. For the disk tier it sizes the read cache, **not** the directory — a disk cache is bounded by entry
    /// TTLs and [`cleanup`](super::Memo::cleanup), not by a byte budget.
    pub fn max_capacity(mut self, bytes: u64) -> Self {
        self.max_capacity = if bytes == 0 { DEFAULT_MAX_CAPACITY } else { bytes };
        self
    }

    /// Makes a disk write durable only once every `writes` writes. Zero restores the default of `1`: every write.
    ///
    /// The writes in between are committed, so they are readable at once, but not forced to disk: a process killed
    /// without unwinding loses at most the writes since the last durable one. Dropping the last handle to the store,
    /// or calling [`flush`](super::Memo::flush), makes them durable. Raising this is what makes bulk writes cheap,
    /// since each durable write waits on an fsync. The memory tier ignores it.
    pub fn flush_every(mut self, writes: u32) -> Self {
        self.flush_every = if writes == 0 { DEFAULT_FLUSH_EVERY } else { writes };
        self
    }

    /// Also makes a disk write durable once `interval` has passed since the last durable one. Zero, the default,
    /// turns the time trigger off.
    ///
    /// The interval is checked only when a write happens; there is no timer. A store that goes quiet keeps its
    /// deferred writes until the next write, a [`flush`](super::Memo::flush), or the drop of its last handle. The
    /// memory tier ignores it.
    pub fn flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_make_every_write_durable_with_no_time_trigger() {
        let opts = CacheOpts::new();

        assert_eq!(opts.flush_every, 1);
        assert_eq!(opts.flush_interval, Duration::ZERO);
    }

    #[test]
    fn a_zero_flush_every_restores_the_default() {
        assert_eq!(CacheOpts::new().flush_every(64).flush_every(0).flush_every, 1);
    }

    #[test]
    fn the_cadence_chains_with_the_sizing_and_keeps_the_opts_copy() {
        let opts = CacheOpts::new()
            .max_capacity(4 << 20)
            .flush_every(64)
            .flush_interval(Duration::from_secs(1))
            .max_entries(500);
        let copy = opts;

        assert_eq!(opts.flush_every, 64);
        assert_eq!(opts.flush_interval, Duration::from_secs(1));
        assert_eq!(opts.max_capacity, 4 << 20);
        assert_eq!(opts.max_entries, 500);
        assert_eq!(copy, opts);
    }
}
