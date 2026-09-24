//! Structured log records.
//!
//! The four macros take a message and any number of key/value fields:
//!
//! ```
//! use rust_sak::o11y::log;
//!
//! # let order_id = "ord_8812";
//! # let amount = 129.5;
//! # let error = std::fmt::Error;
//! log::debug!("cache lookup", key = "orders:8812", hit = false);
//! log::info!("order received", order_id = order_id, amount = amount);
//! log::warn!("large order flagged", order_id = order_id, threshold = 10_000.0);
//! log::error!("payment failed", order_id = order_id, error = %error);
//! ```
//!
//! The message is a **constant string, not a format string**: `log::info!("order {id} received")` does not
//! interpolate. Keeping the message fixed and putting the varying parts in fields is what lets a backend group
//! every instance of an event together and filter on `order_id` — which a formatted string cannot support.
//!
//! # Cost when a level is off
//!
//! Each macro expands to a level check wrapping everything else, so a record below the configured
//! [`min_level`](super::ConfigBuilder::min_level) costs one relaxed atomic load and a comparison. **The field
//! expressions are never evaluated** — an expensive `format!` or a method call in a `log::debug!` costs nothing in
//! a build where debug is off. The same is true before [`init`](super::init) has run, and in a process that never
//! calls it.
//!
//! # Correlation with spans
//!
//! A record emitted while a [`span`](super::trace::span) is open carries that span's trace and span ids, so a
//! backend can show the record alongside the trace it belongs to. Nothing is needed at the call site for this.

mod macros;

// `Level` is deliberately not re-exported here — it already lives at `o11y::Level`, and a second public path for one
// type is a second thing for the docs and `tests/public_api.rs` to track.
pub use super::gate::enabled;
pub use super::record::Fields;

use super::level::Level;
use super::trace::SpanContext;

#[doc(inline)]
pub use crate::__rust_sak_o11y_debug as debug;
#[doc(inline)]
pub use crate::__rust_sak_o11y_error as error;
#[doc(inline)]
pub use crate::__rust_sak_o11y_info as info;
#[doc(inline)]
pub use crate::__rust_sak_o11y_warn as warn;

/// Records a message whose level and fields are decided at runtime.
///
/// The macros are the call-site form of this function; `emit` is for the caller that cannot name its fields in
/// source — a bridge from another logging API, or a record assembled from configuration. Like the macros, it
/// attaches the record to the innermost span open on this thread, if there is one.
///
/// **It does not check the level gate**, so a caller that builds its fields at any cost should ask [`enabled`] first,
/// the way the macros do. A record emitted while the gate is shut, or before [`init`](super::init), is discarded.
///
/// ```
/// use std::borrow::Cow;
/// use rust_sak::o11y::{Level, Value, log};
///
/// # let codec = "avif";
/// if log::enabled(Level::Info) {
///     let fields: log::Fields = vec![(Cow::Borrowed("codec"), Value::from(codec))];
///     log::emit(Level::Info, "encode.started", fields);
/// }
/// ```
pub fn emit(level: Level, message: impl Into<String>, fields: Fields) {
    super::pipeline::record_log(level, message.into(), fields, super::trace::current_context());
}

/// Records a message attached to the span `context` names, rather than to whatever is open on this thread.
///
/// For a record whose span is known some other way than the thread's own stack: a worker thread doing work for a
/// span opened elsewhere, or a bridge that tracks spans itself. As with [`emit`], checking [`enabled`] first is the
/// caller's job.
///
/// ```
/// use rust_sak::o11y::{Level, log, trace};
///
/// let _span = trace::span!("encode");
///
/// if let Some(context) = trace::current().context() {
///     std::thread::spawn(move || {
///         log::emit_in(context, Level::Info, "encoded on a worker", Vec::new());
///     })
///     .join()
///     .unwrap();
/// }
/// ```
pub fn emit_in(context: SpanContext, level: Level, message: impl Into<String>, fields: Fields) {
    super::pipeline::record_log(level, message.into(), fields, Some(context));
}
