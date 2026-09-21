use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use super::registry::{HistogramPoint, Instrument, MetricData, MetricSnapshot, register};
use super::tags::{TagMap, Tags, no_tags, overflow_tags};

/// The bucket bounds used when [`Histogram::with_buckets`] was never called.
///
/// These are OpenTelemetry's own default explicit bounds, which suit durations expressed in seconds and are a
/// defensible shape for most other quantities.
const DEFAULT_BOUNDS: &[f64] = &[
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0, 1000.0, 2500.0, 5000.0, 7500.0, 10000.0,
];

/// A distribution of recorded values over fixed buckets, such as a request duration or an order value.
///
/// ```
/// use std::sync::LazyLock;
/// use rust_sak::o11y::metric::{self, Histogram};
///
/// static ORDER_VALUE: LazyLock<Histogram> =
///     LazyLock::new(|| metric::histogram("order_value_dollars").with_buckets(&[10.0, 50.0, 100.0, 500.0]));
///
/// ORDER_VALUE.record(129.5);
/// ```
///
/// Recording finds the bucket with a short linear scan and performs two atomic adds plus one compare-and-swap on the
/// running total. There is no allocation and no lock on the untagged path.
#[derive(Debug, Clone)]
pub struct Histogram {
    /// Shared with the registry, which holds a [`Weak`](std::sync::Weak) reference to it.
    state: Arc<HistogramState>,
}

/// The accumulating half of a [`Histogram`].
#[derive(Debug)]
struct HistogramState {
    /// The instrument name, as it appears on the wire.
    name: String,
    /// The bounds and the untagged buckets, which cannot be sized until the bounds are known.
    ///
    /// Deferred into a [`OnceLock`] so that `histogram(name)` and `with_buckets(..)` act on **one** registered
    /// instrument. Registering in `histogram` and then replacing the state in `with_buckets` would briefly leave two
    /// instruments of the same name in the registry, and a flush landing in that window would export both.
    layout: OnceLock<Layout>,
    /// One set of buckets per tag set seen.
    tagged: TagMap<Buckets>,
    /// Everything that arrived after the per-instrument series cap was reached.
    overflow: OnceLock<Buckets>,
}

/// The bounds and the buckets belonging to the untagged series.
#[derive(Debug)]
struct Layout {
    /// Bucket upper bounds, ascending.
    bounds: Box<[f64]>,
    /// The untagged series.
    untagged: Buckets,
}

impl Layout {
    /// Builds a layout from `bounds`, which the caller has already sorted.
    fn new(bounds: Box<[f64]>) -> Self {
        let untagged = Buckets::new(bounds.len());

        Self { bounds, untagged }
    }
}

/// One series of a histogram: a count per bucket, plus the total count and sum.
#[derive(Debug)]
struct Buckets {
    /// One counter per bucket, always one longer than the bounds — the last catches everything above the final one.
    counts: Box<[AtomicU64]>,
    /// How many values have been recorded.
    count: AtomicU64,
    /// Their running total, as `f64::to_bits`.
    sum: AtomicU64,
}

impl Buckets {
    /// Allocates the counters for a histogram with `bound_count` explicit bounds.
    fn new(bound_count: usize) -> Self {
        Self {
            counts: (0..=bound_count).map(|_| AtomicU64::new(0)).collect(),
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }

    /// Files `value` into its bucket and folds it into the count and sum.
    fn record(&self, bounds: &[f64], value: f64) {
        // Linear rather than binary: bucket lists are short enough that the scan wins on a predictable branch, and
        // it keeps the common "first or last bucket" cases at one or two comparisons.
        let index = bounds.iter().position(|bound| value <= *bound).unwrap_or(bounds.len());

        self.counts[index].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);

        // `f64` has no atomic add, so the total is a compare-and-swap loop over its bit pattern. Contention here is
        // between threads recording into the *same* series, and the loop body is two instructions.
        let mut current = self.sum.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(current) + value).to_bits();

            match self
                .sum
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Reads the series into an exportable point. The bucket bounds are the enclosing instrument's, so they are
    /// recorded once beside the points rather than copied into each one.
    fn snapshot(&self, tags: Tags) -> HistogramPoint {
        HistogramPoint {
            tags,
            count: self.count.load(Ordering::Relaxed),
            sum: f64::from_bits(self.sum.load(Ordering::Relaxed)),
            bucket_counts: self.counts.iter().map(|count| count.load(Ordering::Relaxed)).collect(),
        }
    }
}

/// Creates a histogram named `name`, using the default bucket bounds until [`Histogram::with_buckets`] replaces them.
pub fn histogram(name: impl Into<String>) -> Histogram {
    let state = Arc::new(HistogramState {
        name: name.into(),
        layout: OnceLock::new(),
        tagged: TagMap::default(),
        overflow: OnceLock::new(),
    });

    register(Arc::downgrade(&state) as _);

    Histogram { state }
}

impl Histogram {
    /// Sets the bucket upper bounds. Values are sorted, so they need not be supplied in order.
    ///
    /// Only the first call has any effect, and a call after the first [`record`](Histogram::record) has none: the
    /// bucket counters are sized from the bounds, so changing them later would invalidate everything recorded so
    /// far. Ignoring the later call is what keeps the `LazyLock` pattern safe — two `static`s naming one metric with
    /// different bounds is a bug, but it must not be a panic inside a lazy initialiser.
    #[must_use = "with_buckets returns the configured histogram rather than modifying in place"]
    pub fn with_buckets(self, bounds: &[f64]) -> Self {
        let mut sorted = bounds.to_vec();
        sorted.sort_by(f64::total_cmp);

        let _ = self.state.layout.set(Layout::new(sorted.into_boxed_slice()));

        self
    }

    /// Files `value` into the untagged series.
    pub fn record(&self, value: f64) {
        let layout = self.layout();
        layout.untagged.record(&layout.bounds, value);
    }

    /// Files `value` into the series identified by `tags`. See
    /// [`Counter::add_with_tags`](super::Counter::add_with_tags) for how tag sets are matched and capped.
    pub fn record_with_tags(&self, value: f64, tags: &[(&str, &str)]) {
        let layout = self.layout();
        let bound_count = layout.bounds.len();

        let recorded = self.state.tagged.with(
            tags,
            || Buckets::new(bound_count),
            |series| series.record(&layout.bounds, value),
        );

        if recorded.is_none() {
            self.state
                .overflow
                .get_or_init(|| Buckets::new(bound_count))
                .record(&layout.bounds, value);
        }
    }

    /// The layout, defaulting the bounds if `with_buckets` was never called.
    fn layout(&self) -> &Layout {
        self.state.layout.get_or_init(|| Layout::new(DEFAULT_BOUNDS.into()))
    }
}

impl Instrument for HistogramState {
    fn snapshot(&self) -> MetricSnapshot {
        // An instrument nothing has recorded into has no layout yet, and therefore no data points.
        let Some(layout) = self.layout.get() else {
            return MetricSnapshot {
                name: self.name.clone(),
                data: MetricData::Histogram {
                    bounds: Box::from([]),
                    points: Vec::new(),
                },
            };
        };

        let mut points = Vec::new();

        if layout.untagged.count.load(Ordering::Relaxed) > 0 {
            points.push(layout.untagged.snapshot(no_tags()));
        }

        self.tagged.each(|tags, series| {
            points.push(series.snapshot(tags.clone()));
        });

        if let Some(overflow) = self.overflow.get() {
            points.push(overflow.snapshot(overflow_tags()));
        }

        MetricSnapshot {
            name: self.name.clone(),
            data: MetricData::Histogram {
                bounds: layout.bounds.clone(),
                points,
            },
        }
    }
}
