use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::registry::{Instrument, MetricData, MetricSnapshot, SumPoint, register};
use super::tags::{TagMap, no_tags, overflow_tags};

/// A monotonically increasing total, such as a request or error count.
///
/// Create one **once** and reuse it. The intended shape is a `static` behind a [`LazyLock`](std::sync::LazyLock),
/// which works whether or not [`init`](crate::o11y::init) has run — or ever runs:
///
/// ```
/// use std::sync::LazyLock;
/// use rust_sak::o11y::metric::{self, Counter};
///
/// static ORDERS: LazyLock<Counter> = LazyLock::new(|| metric::counter("orders_total"));
///
/// ORDERS.increment(1);
/// ORDERS.add_with_tags(1, &[("region", "eu-west-1")]);
/// ```
///
/// Incrementing is a single atomic add against the instrument's own memory. There is no level gate and no check for
/// whether telemetry is running, because the add is cheaper than the branch that would skip it. A counter in a
/// process that never initialises telemetry simply accumulates a number nobody reads.
///
/// Cloning a `Counter` gives another handle to the same series, not a second one.
#[derive(Debug, Clone)]
pub struct Counter {
    /// Shared with the registry, which holds a [`Weak`](std::sync::Weak) so it never keeps a dropped counter alive.
    state: Arc<CounterState>,
}

/// The accumulating half of a [`Counter`].
#[derive(Debug)]
struct CounterState {
    /// The instrument name, as it appears on the wire.
    name: String,
    /// The series with no tags.
    untagged: AtomicU64,
    /// Whether the untagged series has ever been incremented.
    ///
    /// A counter starts at zero and a legitimate `increment(0)` is indistinguishable from an untouched one, so the
    /// flag is what separates "this series sits at zero" from "this series does not exist" — the same distinction
    /// [`Gauge`](super::Gauge) draws with its own `untagged_set`. Without it, an instrument used only through
    /// [`Counter::add_with_tags`] exports a permanent untagged `value: 0` that nothing ever wrote.
    untagged_set: AtomicBool,
    /// One series per tag set seen.
    tagged: TagMap<AtomicU64>,
    /// Everything that arrived after the per-instrument series cap was reached.
    overflow: AtomicU64,
}

/// Creates or re-opens a counter named `name`.
///
/// Two calls with the same name produce two independent instruments rather than one shared series, so call this once
/// and keep the handle. Cloning the handle, or reading the same `static`, is how you share it.
pub fn counter(name: impl Into<String>) -> Counter {
    let state = Arc::new(CounterState {
        name: name.into(),
        untagged: AtomicU64::new(0),
        untagged_set: AtomicBool::new(false),
        tagged: TagMap::default(),
        overflow: AtomicU64::new(0),
    });

    register(Arc::downgrade(&state) as _);

    Counter { state }
}

impl Counter {
    /// Adds `amount` to the untagged series.
    pub fn increment(&self, amount: u64) {
        self.state.untagged.fetch_add(amount, Ordering::Relaxed);
        self.state.untagged_set.store(true, Ordering::Relaxed);
    }

    /// Adds `amount` to the series identified by `tags`.
    ///
    /// Tag order does not matter: `&[("a", "1"), ("b", "2")]` and `&[("b", "2"), ("a", "1")]` are the same series.
    /// Once an instrument has accumulated `MAX_SERIES` distinct tag sets, further new
    /// ones fold into a single `o11y.series_overflow` series, so the total stays correct while memory stays bounded.
    pub fn add_with_tags(&self, amount: u64, tags: &[(&str, &str)]) {
        let recorded = self.state.tagged.with(
            tags,
            || AtomicU64::new(0),
            |series| series.fetch_add(amount, Ordering::Relaxed),
        );

        if recorded.is_none() {
            self.state.overflow.fetch_add(amount, Ordering::Relaxed);
        }
    }
}

impl Instrument for CounterState {
    fn snapshot(&self) -> MetricSnapshot {
        let mut points = Vec::new();

        if self.untagged_set.load(Ordering::Relaxed) {
            points.push(SumPoint {
                tags: no_tags(),
                value: self.untagged.load(Ordering::Relaxed),
            });
        }

        self.tagged.each(|tags, series| {
            points.push(SumPoint {
                tags: tags.clone(),
                value: series.load(Ordering::Relaxed),
            });
        });

        let overflow = self.overflow.load(Ordering::Relaxed);
        if overflow > 0 {
            points.push(SumPoint {
                tags: overflow_tags(),
                value: overflow,
            });
        }

        MetricSnapshot {
            name: self.name.clone(),
            data: MetricData::Sum(points),
        }
    }
}
