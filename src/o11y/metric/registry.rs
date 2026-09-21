//! The process-wide list of live instruments, and the snapshot the exporter reads from it.

use std::fmt;
use std::sync::{LazyLock, Mutex, PoisonError, Weak};

use super::tags::Tags;

/// Something the exporter can read a set of data points out of.
pub(crate) trait Instrument: Send + Sync + fmt::Debug {
    /// The instrument's current value, as of this instant.
    fn snapshot(&self) -> MetricSnapshot;
}

/// Every instrument created in this process, whether or not telemetry was ever initialised.
///
/// This is deliberately **independent of [`init`](crate::o11y::init)**. Instruments are meant to live in a `static`
/// behind a `LazyLock`, and a `static` is first touched by whatever code path reaches it — which may well be before
/// `main` has called `init`, or in a process that never calls it at all. Tying registration to initialisation would
/// make an instrument's data silently depend on that ordering.
///
/// Holding [`Weak`] references rather than strong ones means the registry never keeps a dropped instrument alive,
/// and never leaks for a program that creates instruments dynamically.
static INSTRUMENTS: LazyLock<Mutex<Vec<Weak<dyn Instrument>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Adds an instrument to the registry. Called once, when the instrument is created.
pub(super) fn register(instrument: Weak<dyn Instrument>) {
    INSTRUMENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(instrument);
}

/// Reads every live instrument, pruning any that have been dropped since the last flush.
pub(crate) fn snapshot() -> Vec<MetricSnapshot> {
    let mut instruments = INSTRUMENTS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut snapshots = Vec::with_capacity(instruments.len());

    instruments.retain(|weak| match weak.upgrade() {
        Some(instrument) => {
            snapshots.push(instrument.snapshot());
            true
        }
        None => false,
    });

    snapshots
}

/// Removes every registered instrument. Test-only, so one test's instruments cannot show up in another's snapshot.
#[cfg(test)]
pub(crate) fn clear() {
    INSTRUMENTS.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// One instrument's readings at a point in time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetricSnapshot {
    /// The instrument name, as given to [`counter`](super::counter) and friends.
    pub(crate) name: String,
    /// Its data points, one per tag set.
    pub(crate) data: MetricData,
}

/// The data points of one instrument, shaped by what kind of instrument produced them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MetricData {
    /// A monotonically increasing total.
    Sum(Vec<SumPoint>),
    /// A value that moves in both directions.
    Gauge(Vec<GaugePoint>),
    /// A distribution over fixed buckets.
    Histogram(Vec<HistogramPoint>),
}

/// One series of a counter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SumPoint {
    /// The tags identifying this series. Empty for the untagged one.
    pub(crate) tags: Tags,
    /// The running total since the process started.
    pub(crate) value: u64,
}

/// One series of a gauge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GaugePoint {
    /// The tags identifying this series. Empty for the untagged one.
    pub(crate) tags: Tags,
    /// The most recently set value.
    pub(crate) value: f64,
}

/// One series of a histogram.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HistogramPoint {
    /// The tags identifying this series. Empty for the untagged one.
    pub(crate) tags: Tags,
    /// How many values were recorded.
    pub(crate) count: u64,
    /// Their total.
    pub(crate) sum: f64,
    /// The upper bounds of every bucket but the last, which catches everything above the final bound.
    pub(crate) bounds: Vec<f64>,
    /// One count per bucket, always one longer than `bounds`.
    pub(crate) bucket_counts: Vec<u64>,
}
