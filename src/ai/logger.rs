//! The logger every environment is started with: it counts the runtime's out-of-memory records on the thread that
//! logged them, then hands each record on to the caller's logger, or to `tracing` in `ort`'s own shape.
//!
//! A failed build's own error often says only that TensorRT *"failed to create engine"*; that the device ran out of
//! memory is in a record logged on the way there. The runtime logs a build's records on the thread that builds, so a
//! per-thread count read before and after the build tells [`build`](super::build) what happened in it.

use std::cell::Cell;
use std::sync::Arc;

use ort::logging::{LogLevel, LoggerFunction};
use tracing::Level;

thread_local! {
    /// How many out-of-memory records the runtime has logged on this thread.
    ///
    /// `const`-initialised and without a destructor, so reading it from the callback is sound even while the thread is
    /// being torn down: a panic there would cross the runtime's `extern "system"` boundary and abort the process.
    static OUT_OF_MEMORY: Cell<u64> = const { Cell::new(0) };
}

/// The logger the environment is started with: it counts, then forwards each record to `logger`, or to `tracing` when
/// that is `None`.
pub(super) fn wrap(logger: Option<LoggerFunction>) -> LoggerFunction {
    Arc::new(move |severity, category, id, location, message| {
        count(message);
        match &logger {
            Some(logger) => logger(severity, category, id, location, message),
            None => to_tracing(severity, id, location, message),
        }
    })
}

/// How many out-of-memory records this thread has seen so far, to hand back to [`out_of_memory_since`].
pub(super) fn out_of_memory_mark() -> u64 {
    OUT_OF_MEMORY.with(Cell::get)
}

/// Whether the runtime logged an out-of-memory record on this thread since `mark` was taken.
pub(super) fn out_of_memory_since(mark: u64) -> bool {
    out_of_memory_mark() != mark
}

/// Counts `message` on this thread when it reports a failed device allocation.
fn count(message: &str) {
    if is_out_of_memory_record(message) {
        OUT_OF_MEMORY.with(|count| count.set(count.get().wrapping_add(1)));
    }
}

/// Whether a record reports a device allocation that failed: TensorRT's `Error Code 2: OutOfMemory`, or the CUDA
/// provider's `out of memory`.
fn is_out_of_memory_record(message: &str) -> bool {
    message.contains("OutOfMemory") || message.contains("out of memory")
}

/// Whether a runtime error's message is a failed device allocation, which is how the CUDA provider reports it.
pub(super) fn says_out_of_memory(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("out of memory") || message.contains("outofmemory") || message.contains("failed to allocate")
}

/// One record, as `ort`'s own `tracing` bridge writes it: an event under the target `ort::logging` whose parent is a
/// TRACE span named `ort` carrying `id` and `location`.
fn to_tracing(severity: LogLevel, id: &str, location: &str, message: &str) {
    let span = tracing::span!(target: "ort::logging", Level::TRACE, "ort", id = id, location = location);
    match severity {
        LogLevel::Verbose => tracing::event!(target: "ort::logging", parent: &span, Level::TRACE, "{message}"),
        LogLevel::Info => tracing::event!(target: "ort::logging", parent: &span, Level::INFO, "{message}"),
        LogLevel::Warning => tracing::event!(target: "ort::logging", parent: &span, Level::WARN, "{message}"),
        LogLevel::Error => tracing::event!(target: "ort::logging", parent: &span, Level::ERROR, "{message}"),
        LogLevel::Fatal => tracing::event!(target: "ort::logging", parent: &span, Level::ERROR, "(FATAL): {message}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::*;

    /// The record from the field that fails a TensorRT build: an RTX 5080 with another model resident.
    const FAILED_BUILD: &str = "[2026-09-27 06:51:55   ERROR] [globWriter.cpp::nvinfer1::builder::HybridGlobWriter::\
                                HybridGlobWriter::496] Error Code 2: OutOfMemory (Requested size was 1024 bytes.)";

    #[test]
    fn an_out_of_memory_record_marks_the_thread_that_logged_it() {
        let logger = wrap(Some(Arc::new(|_, _, _, _, _| {})));
        let mark = out_of_memory_mark();
        logger(LogLevel::Warning, "", "app", "here", "Some nodes were not assigned");
        assert!(!out_of_memory_since(mark), "a record about something else was counted");

        logger(LogLevel::Error, "", "app", "here", FAILED_BUILD);
        assert!(out_of_memory_since(mark));

        let elsewhere = std::thread::spawn(|| {
            let mark = out_of_memory_mark();
            out_of_memory_since(mark)
        });
        assert!(
            !elsewhere.join().unwrap(),
            "another thread's records are not this one's"
        );
    }

    #[test]
    fn a_record_on_another_thread_does_not_mark_this_one() {
        let logger = wrap(Some(Arc::new(|_, _, _, _, _| {})));
        let mark = out_of_memory_mark();
        std::thread::spawn(move || logger(LogLevel::Error, "", "app", "here", FAILED_BUILD))
            .join()
            .unwrap();
        assert!(!out_of_memory_since(mark));
    }

    #[test]
    fn every_record_reaches_the_callers_logger_unchanged() {
        type Seen = Vec<(LogLevel, String, String, String, String)>;
        let seen: Arc<Mutex<Seen>> = Arc::default();
        let into = Arc::clone(&seen);
        let logger = wrap(Some(Arc::new(move |level, category, id, location, message| {
            into.lock().unwrap().push((
                level,
                category.to_owned(),
                id.to_owned(),
                location.to_owned(),
                message.to_owned(),
            ));
        })));
        logger(LogLevel::Error, "VLOG", "app", "trt.h:98", FAILED_BUILD);
        logger(LogLevel::Info, "", "app", "here", "Graph optimized");
        assert_eq!(
            *seen.lock().unwrap(),
            [
                (
                    LogLevel::Error,
                    "VLOG".into(),
                    "app".into(),
                    "trt.h:98".into(),
                    FAILED_BUILD.into()
                ),
                (
                    LogLevel::Info,
                    String::new(),
                    "app".into(),
                    "here".into(),
                    "Graph optimized".into()
                ),
            ]
        );
    }

    #[test]
    fn an_error_message_naming_a_failed_allocation_is_out_of_memory() {
        assert!(says_out_of_memory("CUDA failure 2: out of memory"));
        assert!(says_out_of_memory("Error Code 2: OutOfMemory"));
        assert!(says_out_of_memory("Failed to allocate memory for requested buffer"));
        assert!(!says_out_of_memory(
            "TensorRT EP failed to create engine from network for fused node: TRTKernel_graph_0"
        ));
    }

    /// Every span and event while it is the default subscriber, each with its target and fields.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<String>>>);

    struct Fields(String);

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push_str(&format!(" {}={value}", field.name()));
        }
    }

    impl Subscriber for Captured {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let mut fields = Fields(format!(
                "span {} {} {}",
                span.metadata().target(),
                span.metadata().level(),
                span.metadata().name()
            ));
            span.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
            Id::from_u64(7)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(format!(
                "event {} {} parent={:?}",
                event.metadata().target(),
                event.metadata().level(),
                event.parent().map(Id::into_u64)
            ));
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    #[test]
    fn without_a_logger_a_record_goes_to_tracing_in_orts_shape() {
        let captured = Captured::default();
        let logger = wrap(None);
        let mark = out_of_memory_mark();
        tracing::subscriber::with_default(captured.clone(), || {
            logger(
                LogLevel::Warning,
                "",
                "app",
                "session_state.cc:1166",
                "Some nodes were not assigned",
            );
            logger(LogLevel::Fatal, "", "app", "here", FAILED_BUILD);
        });
        assert!(out_of_memory_since(mark), "the record was counted on the way");
        assert_eq!(
            *captured.0.lock().unwrap(),
            [
                "span ort::logging TRACE ort id=app location=session_state.cc:1166".to_owned(),
                "event ort::logging WARN parent=Some(7) message=Some nodes were not assigned".to_owned(),
                "span ort::logging TRACE ort id=app location=here".to_owned(),
                format!("event ort::logging ERROR parent=Some(7) message=(FATAL): {FAILED_BUILD}"),
            ]
        );
    }
}
