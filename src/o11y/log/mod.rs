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

pub use super::level::{Level, enabled};

#[doc(inline)]
pub use crate::__rust_sak_o11y_debug as debug;
#[doc(inline)]
pub use crate::__rust_sak_o11y_error as error;
#[doc(inline)]
pub use crate::__rust_sak_o11y_info as info;
#[doc(inline)]
pub use crate::__rust_sak_o11y_warn as warn;

/// Hands a record to the buffer. Macro plumbing; not API.
///
/// The level has already been checked by the macro, so this is only reached for a record that will be kept.
#[doc(hidden)]
pub fn __emit(level: Level, message: impl Into<String>, fields: super::record::Fields) {
    super::pipeline::record_log(level, message.into(), fields);
}
