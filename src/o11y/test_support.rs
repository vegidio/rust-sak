//! Test-only helpers for `o11y`.
//!
//! The HTTP collector is shared with the end-to-end integration test, which is a separate binary and cannot reach
//! an in-crate `#[cfg(test)]` module. Including its file here rather than restating it is what keeps the two from
//! drifting — see the notes in that file.

#[path = "../../tests/common/mod.rs"]
mod common;

pub(super) use common::*;

use std::sync::{Arc, Mutex, PoisonError};

use super::Level;
use super::record::{LogRecord, SpanRecord};

/// Serialises the tests that touch the process globals.
///
/// `cargo nextest` gives every test its own process, but `cargo llvm-cov --html` — which `scripts/coverage.sh`
/// runs by default — goes through `cargo test`, where the whole suite shares one process and therefore one set of
/// `static`s. Without this the two runners would disagree about whether the suite passes.
///
/// Poisoning is recovered from rather than propagated, so one failing test does not cascade into every other.
pub(super) fn global_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());

    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where [`record_log`](super::pipeline::record_log) and [`record_span`](super::pipeline::record_span) send records
/// while a [`Capture`] is live, in place of the pipeline.
///
/// `init` succeeds once per process, so a unit test cannot install a real pipeline to see what a call emitted. This
/// is the in-process equivalent: the records exactly as they would have been buffered, before encoding.
static SINK: Mutex<Option<Arc<Sink>>> = Mutex::new(None);

/// The records captured so far.
#[derive(Debug, Default)]
pub(super) struct Sink {
    logs: Mutex<Vec<LogRecord>>,
    spans: Mutex<Vec<SpanRecord>>,
}

impl Sink {
    /// Keeps a log record.
    pub(super) fn log(&self, record: LogRecord) {
        self.logs.lock().unwrap_or_else(PoisonError::into_inner).push(record);
    }

    /// Keeps a finished span.
    pub(super) fn span(&self, record: SpanRecord) {
        self.spans.lock().unwrap_or_else(PoisonError::into_inner).push(record);
    }
}

/// The live sink, if a test installed one.
pub(super) fn capture() -> Option<Arc<Sink>> {
    SINK.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Captures every record emitted while it is alive, with both gates open at `min_level`.
///
/// Holds [`global_lock`] for its whole life, since the gates and the sink are process globals. Tests that do not
/// take the lock still run alongside it and may land records here too — the span tests open guards freely — so
/// assertions find their records by name rather than by position.
pub(super) struct Capture {
    sink: Arc<Sink>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Capture {
    /// Installs a fresh sink and opens the gates.
    pub(super) fn start(min_level: Level) -> Self {
        let lock = global_lock();
        let sink = Arc::new(Sink::default());

        *SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&sink));
        super::gate::open(min_level);

        Self { sink, _lock: lock }
    }

    /// Shuts the gates, as `shutdown` would, while keeping the sink installed — so a test can prove that nothing
    /// arrives afterwards.
    pub(super) fn close_gates(&self) {
        super::gate::close();
    }

    /// Takes the log records captured so far. Only the bridge's tests need everything at once.
    #[cfg(feature = "o11y-tracing")]
    pub(super) fn logs(&self) -> Vec<LogRecord> {
        std::mem::take(&mut *self.sink.logs.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Takes the spans captured so far.
    #[cfg(feature = "o11y-tracing")]
    pub(super) fn spans(&self) -> Vec<SpanRecord> {
        std::mem::take(&mut *self.sink.spans.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Removes and returns the captured log record with body `body`, which the test expects to be the only one.
    pub(super) fn log_named(&self, body: &str) -> LogRecord {
        let mut logs = self.sink.logs.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut matching, rest): (Vec<_>, Vec<_>) = logs.drain(..).partition(|record| record.body == body);
        *logs = rest;

        assert_eq!(matching.len(), 1, "expected exactly one log record {body:?}");
        matching.remove(0)
    }

    /// Removes and returns the captured spans named `name`.
    pub(super) fn spans_named(&self, name: &str) -> Vec<SpanRecord> {
        let mut spans = self.sink.spans.lock().unwrap_or_else(PoisonError::into_inner);
        let (matching, rest): (Vec<_>, Vec<_>) = spans.drain(..).partition(|span| span.name == name);
        *spans = rest;

        matching
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        super::gate::close();
        *SINK.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}
