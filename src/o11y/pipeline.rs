//! The process-wide telemetry state, and the entry points that feed it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::buffer::Buffer;
use super::config::ExportErrorHandler;
use super::enrichment::{Attributes, Enrichment};
use super::geolocation::fetch_geolocation_with;
use super::record::{Fields, LogRecord, SpanRecord, now_unix_nano};
use super::{Config, Level, O11yError, Result, Value, level, worker};

/// The installed pipeline, or nothing if telemetry was never started.
///
/// Reached only after a gate has already passed, so the common uninitialised path never touches it.
static PIPELINE: OnceLock<Pipeline> = OnceLock::new();

/// Whether spans are being recorded. A dedicated flag so opening a span is one load, not a level comparison.
static TRACING: AtomicBool = AtomicBool::new(false);

/// Whether [`init`] has already been claimed, including by a call that disabled telemetry.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The enrichment handed to records emitted when no pipeline is installed.
static NO_ENRICHMENT: LazyLock<Attributes> = LazyLock::new(|| Arc::new(Vec::new()));

/// Everything the export thread and the emitting threads share.
pub(super) struct Shared {
    /// Buffered log records awaiting export.
    pub(super) logs: Buffer<LogRecord>,
    /// Buffered spans awaiting export.
    pub(super) spans: Buffer<SpanRecord>,
    /// The machine and session attributes.
    pub(super) enrichment: Arc<Enrichment>,
    /// The configuration-derived resource attributes: `service.*` and the deployment environment.
    pub(super) resource: Vec<(Cow<'static, str>, Value)>,
    /// The collector's base URL.
    pub(super) endpoint: String,
    /// Headers put on every export request.
    pub(super) headers: HashMap<String, String>,
    /// How long one export attempt may take.
    pub(super) timeout: Duration,
    /// How long the worker waits between exports.
    pub(super) flush_interval: Duration,
    /// How many buffered records trigger an early export.
    pub(super) max_batch_size: usize,
    /// Where export failures are reported.
    pub(super) on_export_error: ExportErrorHandler,
    /// When this process started recording, reported as every metric's start time.
    pub(super) start_unix_nano: u64,
    /// What the worker should do next.
    pub(super) control: Mutex<Control>,
    /// Signalled when `control` changes, so the worker reacts without polling.
    pub(super) wake: Condvar,
}

/// The worker's instructions, and the counter callers wait on.
#[derive(Debug, Default)]
pub(super) struct Control {
    /// Whether the worker should drain and exit.
    pub(super) shutdown: bool,
    /// Whether an export was asked for before the interval elapsed.
    pub(super) flush_requested: bool,
    /// Incremented after every export cycle, so [`flush`] can tell that its request was served.
    pub(super) generation: u64,
}

// Hand-written for the same reason as `Config`'s: the error callback is a trait object with no `Debug` bound.
impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("endpoint", &self.endpoint)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("timeout", &self.timeout)
            .field("flush_interval", &self.flush_interval)
            .field("max_batch_size", &self.max_batch_size)
            .field("enrichment", &self.enrichment)
            .field("control", &self.control)
            .finish_non_exhaustive()
    }
}

/// The installed pipeline: the shared state, plus the handle needed to join the worker.
#[derive(Debug)]
struct Pipeline {
    /// Shared with the export thread.
    shared: Arc<Shared>,
    /// The export thread, taken by [`shutdown`] when it joins.
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// Starts telemetry. See [`init`](super::init) for the public documentation.
pub(super) fn init(config: Config) -> Result<()> {
    validate_endpoint(&config.endpoint)?;
    validate_headers(&config.headers)?;

    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return Err(O11yError::AlreadyInitialized);
    }

    // The whole of `enabled(false)`: no thread, no socket, no lookup, and the gates stay shut so every record is
    // discarded at the call site.
    if !config.enabled {
        return Ok(());
    }

    let enrichment = Arc::new(Enrichment::new(&config.service_name));

    // The single call site for the location lookup: unreachable unless collection is on *and* it was opted into.
    if config.geolocation {
        let enrichment = Arc::clone(&enrichment);
        let url = config.geolocation_url.clone();
        let timeout = config.geolocation_timeout;

        std::thread::spawn(move || {
            if let Ok(geo) = fetch_geolocation_with(&url, timeout) {
                enrichment.set_location(&geo);
            }
        });
    }

    let shared = Arc::new(Shared {
        logs: Buffer::new(config.max_buffered),
        spans: Buffer::new(config.max_buffered),
        enrichment,
        resource: resource_attributes(&config),
        endpoint: config.endpoint,
        headers: config.headers,
        timeout: config.timeout,
        flush_interval: config.flush_interval,
        max_batch_size: config.max_batch_size,
        on_export_error: config.on_export_error,
        start_unix_nano: now_unix_nano(),
        control: Mutex::new(Control::default()),
        wake: Condvar::new(),
    });

    let worker = worker::spawn(Arc::clone(&shared));

    let _ = PIPELINE.set(Pipeline {
        shared,
        worker: Mutex::new(Some(worker)),
    });

    // Published last: a producer that passes a gate must find a pipeline behind it. `OnceLock::get` is an acquire
    // load, so the store above is visible to anything that gets past these.
    level::set_threshold(config.min_level);
    TRACING.store(true, Ordering::Release);

    Ok(())
}

/// The resource attributes taken from the configuration, as opposed to from the machine.
fn resource_attributes(config: &Config) -> Vec<(Cow<'static, str>, Value)> {
    let mut attributes = vec![(
        Cow::Borrowed("service.name"),
        Value::String(config.service_name.clone()),
    )];

    if !config.service_version.is_empty() {
        attributes.push((
            Cow::Borrowed("service.version"),
            Value::String(config.service_version.clone()),
        ));
    }

    attributes.push((
        // From the OpenTelemetry semantic conventions, v1.27 onwards.
        Cow::Borrowed("deployment.environment.name"),
        Value::String(config.environment.to_string()),
    ));
    attributes.push((Cow::Borrowed("telemetry.sdk.name"), Value::from("rust-sak")));
    attributes.push((Cow::Borrowed("telemetry.sdk.language"), Value::from("rust")));

    attributes
}

/// Rejects an endpoint that is not a usable URL.
fn validate_endpoint(endpoint: &str) -> Result<()> {
    reqwest::Url::parse(endpoint).map_err(|err| O11yError::InvalidEndpoint {
        endpoint: endpoint.to_string(),
        reason: err.to_string(),
    })?;

    Ok(())
}

/// Rejects headers that cannot be put on an HTTP request.
///
/// Failing here points straight at a misspelled authentication header, which would otherwise look like a
/// server-side rejection much later, on a thread the caller never sees.
fn validate_headers(headers: &HashMap<String, String>) -> Result<()> {
    for (name, value) in headers {
        let valid = reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok()
            && reqwest::header::HeaderValue::from_str(value).is_ok();

        if !valid {
            return Err(O11yError::InvalidHeader { name: name.clone() });
        }
    }

    Ok(())
}

/// Stops telemetry, draining what is buffered. See [`shutdown`](super::shutdown).
pub(super) fn shutdown() {
    // Shut the gates first, so nothing new is queued behind the drain.
    level::silence();
    TRACING.store(false, Ordering::Release);

    let Some(pipeline) = PIPELINE.get() else { return };

    {
        let mut control = pipeline.shared.control.lock().unwrap_or_else(PoisonError::into_inner);
        control.shutdown = true;
        pipeline.shared.wake.notify_all();
    }

    // Taken rather than borrowed, so a second call finds `None` and returns immediately.
    let handle = pipeline.worker.lock().unwrap_or_else(PoisonError::into_inner).take();

    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

/// Exports everything buffered so far, blocking until the worker has been round once.
pub(super) fn flush() {
    let Some(pipeline) = PIPELINE.get() else { return };

    let mut control = pipeline.shared.control.lock().unwrap_or_else(PoisonError::into_inner);

    // A worker that has already exited cannot serve the request, and waiting for it would hang.
    if control.shutdown {
        return;
    }

    let target = control.generation + 1;
    control.flush_requested = true;
    pipeline.shared.wake.notify_all();

    while control.generation < target && !control.shutdown {
        let (guard, timeout) = pipeline
            .shared
            .wake
            .wait_timeout(control, pipeline.shared.timeout)
            .unwrap_or_else(PoisonError::into_inner);

        control = guard;

        if timeout.timed_out() {
            return;
        }
    }
}

/// Buffers a log record. Reached only when the level gate has already passed.
pub(super) fn record_log(level: Level, message: String, fields: Fields) {
    let Some(pipeline) = PIPELINE.get() else { return };

    let (trace_id, span_id) = match super::trace::current_ids() {
        Some((trace_id, span_id)) => (Some(trace_id), Some(span_id)),
        None => (None, None),
    };

    let queued = pipeline.shared.logs.push(LogRecord {
        time_unix_nano: now_unix_nano(),
        level,
        body: message,
        fields,
        enrichment: pipeline.shared.enrichment.attributes(),
        trace_id,
        span_id,
    });

    wake_if_batched(&pipeline.shared, queued);
}

/// Buffers a finished span.
pub(super) fn record_span(record: SpanRecord) {
    let Some(pipeline) = PIPELINE.get() else { return };

    let queued = pipeline.shared.spans.push(record);
    wake_if_batched(&pipeline.shared, queued);
}

/// Wakes the worker when a buffer has just reached the batch threshold.
///
/// Deliberately `==` rather than `>=`: a buffer that sits above the threshold while an export is in flight would
/// otherwise notify on every single push, which is exactly the wrong behaviour under load.
fn wake_if_batched(shared: &Shared, queued: usize) {
    if queued == shared.max_batch_size {
        shared.wake.notify_all();
    }
}

/// The enrichment attributes current right now, or an empty set when nothing is installed.
pub(super) fn attributes() -> Attributes {
    match PIPELINE.get() {
        Some(pipeline) => pipeline.shared.enrichment.attributes(),
        None => Arc::clone(&NO_ENRICHMENT),
    }
}

/// Whether spans are being recorded.
#[inline]
pub(super) fn tracing_enabled() -> bool {
    TRACING.load(Ordering::Relaxed)
}

/// Whether telemetry is installed and exporting.
pub(super) fn is_enabled() -> bool {
    PIPELINE.get().is_some()
}

/// The current session id, if telemetry is running.
pub(super) fn session_id() -> Option<String> {
    PIPELINE.get().map(|pipeline| pipeline.shared.enrichment.session_id())
}

/// Assigns a fresh session id to everything recorded from now on.
pub(super) fn renew_session() {
    if let Some(pipeline) = PIPELINE.get() {
        pipeline.shared.enrichment.renew_session();
    }
}

/// Builds a pipeline's shared state directly, bypassing the process globals.
///
/// Almost everything in this module — buffering, enrichment, payload construction, the export request itself — is a
/// function of this struct rather than of any `static`. Constructing one lets the tests drive a full export cycle
/// without calling [`init`], which can only succeed once per process and would otherwise force the whole suite
/// through a single global.
#[cfg(test)]
pub(super) fn test_shared(endpoint: &str, on_export_error: ExportErrorHandler) -> Arc<Shared> {
    let config = Config::builder(endpoint, super::NO_HEADERS)
        .service_name("test-service")
        .service_version("1.2.3")
        .build();

    Arc::new(Shared {
        logs: Buffer::new(config.max_buffered),
        spans: Buffer::new(config.max_buffered),
        enrichment: Arc::new(Enrichment::new(&config.service_name)),
        resource: resource_attributes(&config),
        endpoint: config.endpoint,
        headers: config.headers,
        timeout: Duration::from_secs(5),
        flush_interval: config.flush_interval,
        max_batch_size: config.max_batch_size,
        on_export_error,
        start_unix_nano: now_unix_nano(),
        control: Mutex::new(Control::default()),
        wake: Condvar::new(),
    })
}
