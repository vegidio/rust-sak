use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

/// The severity of a log record, and the threshold the log macros gate on.
///
/// The discriminants are the OTLP severity numbers, so a level is both the comparison key for
/// [`min_level`](super::ConfigBuilder::min_level) and the value that goes on the wire. Ordering follows severity:
/// `Debug < Info < Warn < Error`.
///
/// ```
/// use rust_sak::o11y::Level;
///
/// assert!(Level::Debug < Level::Error);
/// assert_eq!(Level::Info.severity_text(), "INFO");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Level {
    /// Detail useful while diagnosing a problem, too noisy to keep on by default.
    Debug = 5,
    /// The default: something worth recording happened.
    Info = 9,
    /// Something unexpected that the application recovered from.
    Warn = 13,
    /// Something failed.
    Error = 17,
}

impl Level {
    /// The OTLP `severityNumber` for this level.
    pub fn severity_number(self) -> u8 {
        self as u8
    }

    /// The OTLP `severityText` for this level.
    pub fn severity_text(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.severity_text())
    }
}

/// The active threshold, as a severity number. Records at or above it are emitted.
///
/// Starts at [`u8::MAX`], which no level reaches, so a process that never calls [`init`](super::init) — or one that
/// disabled telemetry — discards everything after a single relaxed load. [`init`](super::init) lowers it, and
/// [`shutdown`](super::shutdown) raises it back so late records are not queued into a drained buffer.
static THRESHOLD: AtomicU8 = AtomicU8::new(u8::MAX);

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

/// Lowers the threshold so records at or above `level` are recorded.
pub(super) fn set_threshold(level: Level) {
    THRESHOLD.store(level as u8, Ordering::Relaxed);
}

/// Raises the threshold so nothing is recorded.
pub(super) fn silence() {
    THRESHOLD.store(u8::MAX, Ordering::Relaxed);
}
