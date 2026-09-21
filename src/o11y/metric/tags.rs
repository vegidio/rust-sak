//! The per-tag-set storage shared by all three instruments.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

/// One tag set, owned, as stored against a series.
///
/// Behind an `Arc` because the exporter takes a copy of every series' tags on every flush, and a tag set never
/// changes after the series is created. A `Vec<(String, String)>` would mean two heap allocations per tag per
/// series per flush — roughly seven thousand of them for a counter at the series cap with three tags each — to
/// reproduce bytes that already exist. Cloning this is a ref-count bump.
pub(super) type Tags = Arc<[(Box<str>, Box<str>)]>;

/// The most distinct tag sets one instrument will track before folding the rest into an overflow series.
pub(crate) const MAX_SERIES: usize = 1024;

/// The tag set of a series that carries no tags: the untagged one.
pub(super) fn no_tags() -> Tags {
    static NO_TAGS: LazyLock<Tags> = LazyLock::new(|| Arc::from([]));
    Arc::clone(&NO_TAGS)
}

/// The tag set reported for samples that arrived after [`MAX_SERIES`] was reached.
pub(super) fn overflow_tags() -> Tags {
    static OVERFLOW: LazyLock<Tags> =
        LazyLock::new(|| Arc::from([(Box::from("o11y.series_overflow"), Box::from("true"))]));
    Arc::clone(&OVERFLOW)
}

/// A map from a tag set to whatever that series accumulates into.
///
/// The shape exists to keep the tagged hot path allocation-free. Keying a `HashMap` on `Vec<(String, String)>`
/// directly would force every `add_with_tags` call to allocate an owned copy of its tags just to perform the lookup.
/// Hashing the borrowed slice instead, and keeping the handful of tag sets that collide in a small vector beside the
/// hash, means a call that has been seen before takes a read lock, hashes, compares and returns — no allocation at
/// all. Only the first sighting of a tag set takes the write lock.
///
/// The hash is the XOR of each pair's hash, so it does not depend on the order the tags were written in. Two call
/// sites tagging the same counter `&[("a", "1"), ("b", "2")]` and `&[("b", "2"), ("a", "1")]` therefore land on one
/// series rather than silently splitting into two.
#[derive(Debug)]
pub(super) struct TagMap<T> {
    /// Tag-set hash to the series sharing it. The inner vector is virtually always one element long.
    entries: RwLock<HashMap<u64, Vec<(Tags, T)>>>,
    /// How many series `entries` holds, tracked separately so the cap can be checked without walking it.
    series: AtomicUsize,
}

impl<T> Default for TagMap<T> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            series: AtomicUsize::new(0),
        }
    }
}

impl<T> TagMap<T> {
    /// Runs `use_series` against the series for `tags`, creating it with `make` the first time it is seen.
    ///
    /// Returns `None` when the tag set is new and the instrument already holds [`MAX_SERIES`] of them. An unbounded
    /// tag domain — a user id, a request path, a raw error string — would otherwise grow this map forever, turning a
    /// telemetry call into a memory leak. The caller folds a refused sample into an overflow series instead, so the
    /// total stays right even though the breakdown stops.
    pub(super) fn with<R>(
        &self,
        tags: &[(&str, &str)],
        make: impl FnOnce() -> T,
        use_series: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        let hash = hash_tags(tags);
        let at_capacity = self.series.load(Ordering::Relaxed) >= MAX_SERIES;

        // The overwhelmingly common path: the series already exists, so a read lock is enough and several threads
        // can be recording against different tag sets at once.
        {
            let entries = self.entries.read().unwrap_or_else(PoisonError::into_inner);

            if let Some(bucket) = entries.get(&hash)
                && let Some((_, series)) = bucket.iter().find(|(stored, _)| same_tags(stored, tags))
            {
                return Some(use_series(series));
            }
        }

        // Checked before taking the write lock, so a flood of one-off tag sets at the cap does not serialise every
        // recording thread behind a lock it would only be refused by anyway.
        if at_capacity {
            return None;
        }

        let mut entries = self.entries.write().unwrap_or_else(PoisonError::into_inner);
        let bucket = entries.entry(hash).or_default();

        // Re-check: another thread may have inserted this very tag set between dropping the read lock and taking
        // the write lock.
        let index = match bucket.iter().position(|(stored, _)| same_tags(stored, tags)) {
            Some(index) => index,
            None => {
                bucket.push((owned(tags), make()));
                self.series.fetch_add(1, Ordering::Relaxed);
                bucket.len() - 1
            }
        };

        Some(use_series(&bucket[index].1))
    }

    /// Runs `visit` against every series, for the exporter to read.
    pub(super) fn each(&self, mut visit: impl FnMut(&Tags, &T)) {
        let entries = self.entries.read().unwrap_or_else(PoisonError::into_inner);

        for bucket in entries.values() {
            for (tags, series) in bucket {
                visit(tags, series);
            }
        }
    }
}

/// Hashes a tag set without regard to the order it was written in.
fn hash_tags(tags: &[(&str, &str)]) -> u64 {
    tags.iter().fold(0, |combined, (key, value)| {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        value.hash(&mut hasher);

        combined ^ hasher.finish()
    })
}

/// Whether a stored tag set holds exactly the pairs in `probe`, in any order.
fn same_tags(stored: &Tags, probe: &[(&str, &str)]) -> bool {
    stored.len() == probe.len()
        && probe
            .iter()
            .all(|(key, value)| stored.iter().any(|(k, v)| &**k == *key && &**v == *value))
}

/// Copies a borrowed tag set into an owned one, for storage.
fn owned(tags: &[(&str, &str)]) -> Tags {
    tags.iter()
        .map(|(key, value)| (Box::from(*key), Box::from(*value)))
        .collect()
}
