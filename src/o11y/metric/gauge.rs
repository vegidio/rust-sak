use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::registry::{GaugePoint, Instrument, MetricData, MetricSnapshot, register};
use super::tags::{TagMap, no_tags, overflow_tags};

/// A value that moves in both directions, such as a queue depth or a cache size.
///
/// Like the other instruments, create one once and keep it:
///
/// ```
/// use std::sync::LazyLock;
/// use rust_sak::o11y::metric::{self, Gauge};
///
/// static QUEUE_DEPTH: LazyLock<Gauge> = LazyLock::new(|| metric::gauge("queue_depth"));
///
/// QUEUE_DEPTH.set(12);
/// ```
///
/// Setting is a single atomic store of the value's bit pattern. A gauge that has never been set reports no data
/// point at all, rather than a misleading `0`.
#[derive(Debug, Clone)]
pub struct Gauge {
    /// Shared with the registry, which holds a [`Weak`](std::sync::Weak) reference to it.
    state: Arc<GaugeState>,
}

/// The current-value half of a [`Gauge`].
#[derive(Debug)]
struct GaugeState {
    /// The instrument name, as it appears on the wire.
    name: String,
    /// The untagged series, stored as `f64::to_bits` so it fits in an atomic.
    untagged: AtomicU64,
    /// Whether the untagged series has ever been set.
    untagged_set: AtomicBool,
    /// One series per tag set seen.
    tagged: TagMap<AtomicU64>,
    /// The last value that arrived after the per-instrument series cap was reached.
    overflow: AtomicU64,
    /// Whether anything ever overflowed.
    overflow_set: AtomicBool,
}

/// Creates a gauge named `name`.
pub fn gauge(name: impl Into<String>) -> Gauge {
    let state = Arc::new(GaugeState {
        name: name.into(),
        untagged: AtomicU64::new(0),
        untagged_set: AtomicBool::new(false),
        tagged: TagMap::default(),
        overflow: AtomicU64::new(0),
        overflow_set: AtomicBool::new(false),
    });

    register(Arc::downgrade(&state) as _);

    Gauge { state }
}

impl Gauge {
    /// Replaces the untagged series' value.
    ///
    /// Takes anything that converts losslessly into an `f64` — every float, and every integer up to 32 bits. A
    /// `u64`, `i64` or `usize` has to be cast at the call site, since converting one is not always exact.
    pub fn set(&self, value: impl Into<f64>) {
        self.state.untagged.store(value.into().to_bits(), Ordering::Relaxed);
        self.state.untagged_set.store(true, Ordering::Relaxed);
    }

    /// Replaces the value of the series identified by `tags`. See [`Counter::add_with_tags`](super::Counter::add_with_tags)
    /// for how tag sets are matched and capped.
    pub fn set_with_tags(&self, value: impl Into<f64>, tags: &[(&str, &str)]) {
        let bits = value.into().to_bits();

        let recorded = self.state.tagged.with(
            tags,
            || AtomicU64::new(bits),
            |series| series.store(bits, Ordering::Relaxed),
        );

        if recorded.is_none() {
            self.state.overflow.store(bits, Ordering::Relaxed);
            self.state.overflow_set.store(true, Ordering::Relaxed);
        }
    }

    /// The current value of the series identified by `tags`, or `None` for a series never set.
    ///
    /// `&[]` reads the untagged series, the one [`set`](Gauge::set) replaces. Tag matching and the overflow caveat
    /// are as for [`Counter::value`](super::Counter::value).
    pub fn value(&self, tags: &[(&str, &str)]) -> Option<f64> {
        if tags.is_empty() {
            return self
                .state
                .untagged_set
                .load(Ordering::Relaxed)
                .then(|| f64::from_bits(self.state.untagged.load(Ordering::Relaxed)));
        }

        self.state
            .tagged
            .get(tags, |series| f64::from_bits(series.load(Ordering::Relaxed)))
    }
}

impl Instrument for GaugeState {
    fn snapshot(&self) -> MetricSnapshot {
        let mut points = Vec::new();

        if self.untagged_set.load(Ordering::Relaxed) {
            points.push(GaugePoint {
                tags: no_tags(),
                value: f64::from_bits(self.untagged.load(Ordering::Relaxed)),
            });
        }

        self.tagged.each(|tags, series| {
            points.push(GaugePoint {
                tags: tags.clone(),
                value: f64::from_bits(series.load(Ordering::Relaxed)),
            });
        });

        if self.overflow_set.load(Ordering::Relaxed) {
            points.push(GaugePoint {
                tags: overflow_tags(),
                value: f64::from_bits(self.overflow.load(Ordering::Relaxed)),
            });
        }

        MetricSnapshot {
            name: self.name.clone(),
            data: MetricData::Gauge(points),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tags::MAX_SERIES;
    use super::*;
    use crate::o11y::test_support::global_lock;

    #[test]
    fn value_reads_the_untagged_series_through_empty_tags() {
        // Held because creating an instrument registers it, and other tests read the whole registry.
        let _guard = global_lock();
        let gauge = gauge("read_untagged");

        gauge.set(3);
        gauge.set(12);
        gauge.set_with_tags(40, &[("queue", "a")]);

        assert_eq!(gauge.value(&[]), Some(12.0));
    }

    #[test]
    fn value_matches_a_tag_set_in_any_order() {
        let _guard = global_lock();
        let gauge = gauge("read_order");

        gauge.set_with_tags(0.5, &[("a", "1"), ("b", "2")]);

        assert_eq!(gauge.value(&[("a", "1"), ("b", "2")]), Some(0.5));
        assert_eq!(gauge.value(&[("b", "2"), ("a", "1")]), Some(0.5));
    }

    #[test]
    fn a_series_never_set_reads_none_and_a_zero_reads_zero() {
        let _guard = global_lock();
        let gauge = gauge("read_unset");

        gauge.set_with_tags(0, &[("a", "1")]);

        assert_eq!(gauge.value(&[]), None);
        assert_eq!(gauge.value(&[("a", "2")]), None);
        assert_eq!(gauge.value(&[("a", "1")]), Some(0.0), "set to zero is not never set");
    }

    #[test]
    fn a_tag_set_folded_into_overflow_reads_as_never_set() {
        let _guard = global_lock();
        let gauge = gauge("read_overflow");

        for index in 0..MAX_SERIES {
            gauge.set_with_tags(1, &[("index", &index.to_string())]);
        }
        gauge.set_with_tags(9, &[("index", "past_the_cap")]);

        assert_eq!(gauge.value(&[("index", "0")]), Some(1.0));
        assert_eq!(gauge.value(&[("index", "past_the_cap")]), None);
    }
}
