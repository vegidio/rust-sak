//! The gates that decide whether anything is recorded at all.
//!
//! There are two, because the two signals ask different questions: a log record compares its severity against a
//! threshold, while opening a span is a plain yes/no and deserves a single load rather than a comparison.
//!
//! They are opened and closed **together**, and that is the reason they live in one module. Telemetry being on or
//! off is one fact, so it gets one pair of operations — [`open`] and [`close`] — rather than being reassembled at
//! each of the three call sites that need it (`pipeline::init`, `pipeline::shutdown`, and the export thread's
//! `Sentinel` when it dies). Reassembling it is how the `Sentinel` came to close one gate and leave the other
//! standing, so spans kept filling a buffer no thread was left to drain.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::Level;

/// The active threshold, as a severity number. Records at or above it are emitted.
///
/// Starts at [`u8::MAX`], which no level reaches, so a process that never calls [`init`](super::init) — or one that
/// disabled telemetry — discards everything after a single relaxed load.
static THRESHOLD: AtomicU8 = AtomicU8::new(u8::MAX);

/// Whether spans are being recorded. A dedicated flag so opening a span is one load, not a level comparison.
static TRACING: AtomicBool = AtomicBool::new(false);

/// Whether a record at `level` would be recorded.
///
/// This is the whole of the disabled-path cost: one relaxed load and a comparison, which the log macros perform
/// *before* evaluating any of their field arguments. `Relaxed` is the right ordering because a racing
/// [`init`](super::init) only decides whether one record near the boundary is kept, and no other memory is being
/// published through this flag.
#[inline]
pub fn enabled(level: Level) -> bool {
    THRESHOLD.load(Ordering::Relaxed) <= level as u8
}

/// Whether spans are being recorded.
#[inline]
pub(super) fn tracing_enabled() -> bool {
    TRACING.load(Ordering::Relaxed)
}

/// Opens both gates, admitting log records at or above `min_level` and allowing spans to record.
///
/// Called last by [`init`](super::init), after the pipeline is published: a producer that passes a gate must find a
/// pipeline behind it. The `Release` store is what pairs with `OnceLock::get`'s acquire load to guarantee that.
pub(super) fn open(min_level: Level) {
    THRESHOLD.store(min_level as u8, Ordering::Relaxed);
    TRACING.store(true, Ordering::Release);
}

/// Shuts both gates, so nothing new is recorded.
///
/// Called first by [`shutdown`](super::shutdown), before the drain, so nothing is queued behind it — and by the
/// export thread's `Sentinel`, so a thread that dies takes the gates down with it.
pub(super) fn close() {
    THRESHOLD.store(u8::MAX, Ordering::Relaxed);
    TRACING.store(false, Ordering::Release);
}
