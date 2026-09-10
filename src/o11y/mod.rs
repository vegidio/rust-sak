//! Observability: OTLP telemetry for applications.
//!
//! This module exposes [`Telemetry`], a handle that ships structured log records to an OpenTelemetry collector over
//! OTLP/HTTP, enriching every record with a fixed set of attributes describing the machine and the current session
//! (version, a service-scoped machine id, OS and architecture, a session id, and optionally an IP-based location).
//!
//! Records are built fluently — [`Telemetry::event`] returns an [`Event`] that takes fields of any type convertible
//! into a [`Value`], and is sent by one of its severity methods. Export happens on a background thread, so **this
//! module needs no async runtime** and emitting a record never blocks.
//!
//! Two independent switches control what leaves the machine:
//! [`enabled`](TelemetryBuilder::enabled) (defaults to `true`) turns the whole module on or off, and
//! [`geolocation`](TelemetryBuilder::geolocation) (defaults to **`false`**) opts into the one enrichment that
//! discloses the public IP address to a third party.
//!
//! ```no_run
//! # fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_sak::o11y::{Environment, Telemetry};
//!
//! let telemetry = Telemetry::builder("https://collector.example.com", "my-app")
//!     .version(env!("CARGO_PKG_VERSION"))
//!     .environment(Environment::Production)
//!     .enabled(true)
//!     .build()?;
//!
//! telemetry
//!     .event("export.finished")
//!     .field("format", "avif")
//!     .field("bytes", 1_048_576u64)
//!     .info();
//!
//! telemetry.shutdown()?;
//! # Ok(())
//! # }
//! ```

mod builder;
mod enrichment;
mod environment;
mod error;
mod event;
mod geolocation;
mod machine_id;
mod value;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use builder::TelemetryBuilder;
pub use environment::Environment;
pub use error::{O11yError, Result};
pub use event::Event;
pub use geolocation::{Geolocation, fetch_geolocation, fetch_geolocation_from, fetch_geolocation_with};
pub use value::Value;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use opentelemetry::Key;
use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};

use enrichment::Enrichment;

/// A handle that emits enriched log records to an OpenTelemetry collector.
///
/// Build one with [`Telemetry::builder`], keep it for the lifetime of the application, and share it across threads
/// behind an [`Arc`] — the emit methods take `&self`. Records are batched and exported on a background thread; call
/// [`shutdown`](Telemetry::shutdown) before exiting to flush them, which [`Drop`] also does.
///
/// A handle built with [`enabled(false)`](TelemetryBuilder::enabled), or produced by [`Telemetry::disabled`], is
/// fully usable and simply discards every record without touching the network.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::Telemetry;
///
/// let telemetry = Telemetry::builder("https://collector.example.com", "my-app").build()?;
///
/// telemetry.info("app.started");
/// telemetry.event("file.opened").field("bytes", 4_096u64).info();
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Telemetry {
    /// The export machinery, or `None` when collection is disabled — in which case every record is dropped on the
    /// floor. Pairing the two in a struct is what makes "enabled" a single fact: a handle cannot report itself
    /// enabled and then have nothing to flush.
    active: Option<Active>,
    /// The attributes shared by every record. Shared with the background geolocation thread, if one was started.
    enrichment: Arc<Enrichment>,
    /// Guards against shutting the provider down twice, so `shutdown` stays idempotent under an explicit call
    /// followed by `Drop`.
    shut_down: AtomicBool,
}

/// The logger and the provider that owns it, which exist together or not at all.
#[derive(Debug)]
struct Active {
    /// The logger records are emitted through.
    logger: SdkLogger,
    /// Kept so the batch processor can be flushed and shut down.
    provider: SdkLoggerProvider,
}

impl Telemetry {
    /// Starts configuring a handle that reports to the OTLP collector at `endpoint` as `service_name`.
    ///
    /// `endpoint` is the collector's base URL; the `/v1/logs` path is appended to it. Because the endpoint is always
    /// supplied programmatically, the `OTEL_EXPORTER_OTLP_*` environment variables are ignored.
    ///
    /// ```no_run
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use rust_sak::o11y::Telemetry;
    ///
    /// let telemetry = Telemetry::builder("https://collector.example.com", "my-app")
    ///     .version("1.2.3")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder(endpoint: impl Into<String>, service_name: impl Into<String>) -> TelemetryBuilder {
        TelemetryBuilder::new(endpoint, service_name)
    }

    /// Creates a handle that discards every record and never touches the network.
    ///
    /// Useful as an infallible fallback when the real handle could not be built:
    ///
    /// ```no_run
    /// use rust_sak::o11y::Telemetry;
    ///
    /// let telemetry = Telemetry::builder("https://collector.example.com", "my-app")
    ///     .build()
    ///     .unwrap_or_else(|_| Telemetry::disabled());
    /// ```
    pub fn disabled() -> Self {
        Self::from_parts(None, Arc::new(Enrichment::new("", "")))
    }

    /// Assembles a handle from an optional provider and the enrichment gathered by the builder.
    ///
    /// Private, but reachable from the sibling modules that make up `o11y`, which is exactly the intended scope.
    fn from_parts(provider: Option<SdkLoggerProvider>, enrichment: Arc<Enrichment>) -> Self {
        let active = provider.map(|provider| Active {
            logger: provider.logger("rust-sak/o11y"),
            provider,
        });

        Self {
            active,
            enrichment,
            shut_down: AtomicBool::new(false),
        }
    }

    /// Starts a record named `name`, to which fields can be attached before it is emitted.
    ///
    /// See [`Event`] for the field and severity methods.
    pub fn event(&self, name: impl Into<String>) -> Event<'_> {
        Event::new(self, name)
    }

    /// Emits a record named `event` at `Debug` severity with no fields of its own.
    pub fn debug(&self, event: impl Into<String>) {
        self.event(event).debug();
    }

    /// Emits a record named `event` at `Info` severity with no fields of its own.
    pub fn info(&self, event: impl Into<String>) {
        self.event(event).info();
    }

    /// Emits a record named `event` at `Warn` severity with no fields of its own.
    pub fn warn(&self, event: impl Into<String>) {
        self.event(event).warn();
    }

    /// Emits a record named `event` at `Error` severity, attaching `err`.
    ///
    /// ```no_run
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use rust_sak::o11y::Telemetry;
    ///
    /// let telemetry = Telemetry::builder("https://collector.example.com", "my-app").build()?;
    ///
    /// if let Err(err) = std::fs::read("/nope") {
    ///     telemetry.error("config.read_failed", &err);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn error<E: std::error::Error + ?Sized>(&self, event: impl Into<String>, err: &E) {
        self.event(event).error(err);
    }

    /// Assigns a fresh session id to every record emitted from now on.
    ///
    /// Safe to call concurrently with any number of threads emitting records.
    pub fn renew_session(&self) {
        self.enrichment.renew_session();
    }

    /// The session id currently attached to records, as a UUID string.
    pub fn session_id(&self) -> String {
        self.enrichment.session_id()
    }

    /// Whether this handle actually exports records.
    pub fn is_enabled(&self) -> bool {
        self.active.is_some()
    }

    /// Exports every record buffered so far, blocking until the batch has been sent.
    ///
    /// # Errors
    ///
    /// Returns [`O11yError::Sdk`] if the export fails or times out. Always `Ok` for a disabled handle.
    pub fn flush(&self) -> Result<()> {
        match &self.active {
            Some(active) => Ok(active.provider.force_flush()?),
            None => Ok(()),
        }
    }

    /// Flushes buffered records and shuts the exporter down.
    ///
    /// Idempotent, and also run by [`Drop`], so calling it explicitly is only needed to observe the error.
    ///
    /// # Errors
    ///
    /// Returns [`O11yError::Sdk`] if the final export fails or times out. Always `Ok` for a disabled handle, and for
    /// a second call.
    pub fn shutdown(&self) -> Result<()> {
        if self.shut_down.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        match &self.active {
            Some(active) => Ok(active.provider.shutdown()?),
            None => Ok(()),
        }
    }

    /// Renders the enrichment plus a record's own fields and emits it.
    ///
    /// An enrichment attribute whose key the record also sets is skipped, so the record's own value wins without
    /// either list being mutated.
    fn emit(&self, name: String, severity: Severity, fields: Vec<(Key, AnyValue)>) {
        let Some(active) = &self.active else {
            return;
        };

        let enrichment = self.enrichment.attributes();

        let mut record = active.logger.create_log_record();
        record.set_timestamp(SystemTime::now());
        record.set_severity_number(severity);
        record.set_severity_text(severity.name());
        record.set_body(AnyValue::from(name));

        record.add_attributes(
            enrichment
                .iter()
                .filter(|(key, _)| !fields.iter().any(|(own, _)| own == key))
                .cloned(),
        );
        record.add_attributes(fields);

        active.logger.emit(record);
    }

    /// The enrichment attributes as a plain string map, for assertions.
    #[cfg(test)]
    fn enrichment_snapshot(&self) -> std::collections::HashMap<String, String> {
        self.enrichment.snapshot()
    }
}

impl Drop for Telemetry {
    /// Flushes and shuts the exporter down, so records buffered at exit are not lost.
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
