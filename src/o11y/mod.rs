//! Observability: logs, metrics and traces for applications.
//!
//! One call to [`init`] at startup, and every other function in the program can record telemetry without holding a
//! handle — the same shape the `log` and `tracing` crates use.
//!
//! ```no_run
//! # fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_sak::o11y::{self, Config, Environment, log, metric, trace};
//!
//! o11y::init(
//!     Config::builder("https://collector.example.com", [("Authorization", "Bearer secret")])
//!         .service_name("checkout-service")
//!         .service_version(env!("CARGO_PKG_VERSION"))
//!         .environment(Environment::Production)
//!         .build(),
//! )?;
//!
//! let _span = trace::span!("checkout", order_id = "ord_8812");
//! log::info!("order received", order_id = "ord_8812", amount = 129.5);
//! metric::counter("orders_total").increment(1);
//!
//! o11y::shutdown();
//! # Ok(())
//! # }
//! ```
//!
//! Recording is designed to cost the caller as little as possible: a gated-out log line is an atomic load and a
//! branch with its arguments never evaluated, a metric is a single atomic operation, and a record that is kept goes
//! onto a bounded in-memory buffer and returns. Everything else — batching, serialising, and talking to the
//! collector — happens on a dedicated thread, so **this module needs no async runtime** and an unreachable
//! collector can cost records but never latency.

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod buffer;
mod capture;
mod config;
mod enrichment;
mod environment;
mod error;
mod gate;
mod geolocation;
mod ids;
mod level;
mod machine_id;
mod macros;
mod otlp;
mod pipeline;
mod record;
mod signal;
mod value;
mod worker;

pub mod log;
pub mod metric;
pub mod trace;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

// Named by the `#[instrument]` expansion, which happens in the caller's crate, so they have to be reachable from
// outside. Plumbing, not API.
#[doc(hidden)]
pub use capture::{ValueViaDebug, ValueViaInto};
pub use config::{Config, ConfigBuilder, NO_HEADERS};
pub use environment::Environment;
pub use error::{O11yError, Result};
pub use geolocation::{Geolocation, fetch_geolocation, fetch_geolocation_from, fetch_geolocation_with};
pub use level::Level;
pub use signal::Signal;
pub use value::Value;

/// Starts telemetry for this process.
///
/// Call it once, at startup, before the first record. Everything recorded before it — a log macro, a span, a metric
/// — is simply discarded, so an early call site is a missing record rather than a failure.
///
/// Metric instruments are the exception: a `static` counter accumulates from the moment it is first touched, whether
/// or not `init` has run, and its total is exported once telemetry starts.
///
/// # Errors
///
/// Returns [`O11yError::InvalidEndpoint`] if the endpoint is not a valid URL, [`O11yError::InvalidHeader`] if a
/// header cannot be sent over HTTP, or [`O11yError::AlreadyInitialized`] if telemetry has already been started. In
/// the last case the first configuration stays in effect.
///
/// Nothing after this point is reported through a `Result`: export failures reach the
/// [`on_export_error`](ConfigBuilder::on_export_error) callback instead.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::{self, Config, NO_HEADERS};
///
/// o11y::init(Config::builder("https://collector.example.com", NO_HEADERS)
///     .service_name("my-app")
///     .build())?;
/// # Ok(())
/// # }
/// ```
pub fn init(config: Config) -> Result<()> {
    pipeline::init(config)
}

/// Stops telemetry, exporting whatever is still buffered.
///
/// **Call this before the process exits.** There is no handle to drop, so nothing else can flush the buffers — this
/// is the one piece of ceremony a process-global API cannot avoid. It blocks for at most one export
/// [`timeout`](ConfigBuilder::timeout).
///
/// Idempotent, and safe to call without a matching [`init`]. Records emitted afterwards are discarded.
pub fn shutdown() {
    pipeline::shutdown();
}

/// Exports everything buffered so far, blocking until the export thread has been round once.
///
/// Rarely needed — the worker exports on its own schedule — but useful before a checkpoint where losing the last
/// few seconds of telemetry would matter. Does nothing if telemetry is not running.
pub fn flush() {
    pipeline::flush();
}

/// Whether telemetry is installed and exporting.
///
/// `false` before [`init`], after [`shutdown`], and when `init` was given
/// [`enabled(false)`](ConfigBuilder::enabled).
pub fn is_enabled() -> bool {
    pipeline::is_enabled()
}

/// The session id attached to records right now, or `None` if telemetry is not running.
pub fn session_id() -> Option<String> {
    pipeline::session_id()
}

/// Assigns a fresh session id to everything recorded from now on.
///
/// Safe to call while other threads are recording. Records already buffered keep the session they were recorded
/// under, rather than picking up whichever one happened to be current when the exporter reached them.
pub fn renew_session() {
    pipeline::renew_session();
}
