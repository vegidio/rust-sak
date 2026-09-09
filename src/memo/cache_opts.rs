/// Sizing hints for a store.
///
/// Both are hints rather than hard limits, and both are clamped to whatever the underlying engine accepts, so no
/// value here can stop a store from opening.
///
/// ```
/// use rust_sak::memo::CacheOpts;
///
/// let opts = CacheOpts::new().max_entries(100_000).max_capacity(256 << 20);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheOpts {
    /// Roughly how many entries the store is sized for.
    pub(super) max_entries: u64,
    /// Roughly how many bytes the store is sized for.
    pub(super) max_capacity: u64,
}

/// Room for about 10,000 entries and 1 GiB.
const DEFAULT_MAX_ENTRIES: u64 = 10_000;
const DEFAULT_MAX_CAPACITY: u64 = 1 << 30;

impl Default for CacheOpts {
    fn default() -> Self {
        CacheOpts {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_capacity: DEFAULT_MAX_CAPACITY,
        }
    }
}

impl CacheOpts {
    /// The defaults: room for about 10,000 entries and 1 GiB.
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
}
