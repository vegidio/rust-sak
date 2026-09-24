//! Spans: timing a stretch of work, and tying log records to it.
//!
//! A span opens when [`span!`] is called and closes when its guard drops, which covers a normal return, an early
//! return through `?`, and a panic unwinding through the frame:
//!
//! ```
//! use rust_sak::o11y::trace;
//!
//! # #[derive(Debug)] struct PaymentError;
//! # impl std::fmt::Display for PaymentError {
//! #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("failed") }
//! # }
//! # impl std::error::Error for PaymentError {}
//! # fn validate_card() -> Result<(), PaymentError> { Ok(()) }
//! # fn submit(order_id: &str) -> Result<(), PaymentError> { Ok(()) }
//! fn charge_card(order_id: &str) -> Result<(), PaymentError> {
//!     let _span = trace::span!("charge_card", order_id = order_id);
//!     validate_card()?;
//!     submit(order_id)
//! }
//! ```
//!
//! Spans nest by thread: a span opened inside another inherits its trace id and records it as its parent. Any log
//! record emitted while a span is open carries that span's trace and span ids, which is what lets a backend show a
//! trace and its logs together.
//!
//! # Async
//!
//! A bare guard is **sync-only**. Holding one across an `.await` is wrong on a multi-threaded executor: the task can
//! resume on another thread, where the span is not on the stack, while the span stays open on the original thread
//! and mis-parents whatever is scheduled there next.
//!
//! For async work, attach the span to the future instead, with
//! [`Instrument::instrument`] or with `#[instrument]` on the `async fn` — both enter and
//! exit the span around each poll, so it follows the task wherever it runs.

mod context;
mod current;
mod instrumented;
mod macros;
mod owned;
mod span;
mod span_context;

pub use current::{Current, current};
pub use instrumented::{Instrument, Instrumented};
pub use owned::{OwnedSpan, Parent, start};
pub use span::Span;
pub use span_context::SpanContext;

/// Wraps a function body in a span named after the function, capturing its arguments as fields.
///
/// See [the macro's own documentation](o11y_macros::instrument) for its options.
pub use o11y_macros::instrument;

#[doc(inline)]
pub use crate::__rust_sak_o11y_span as span;

pub(in crate::o11y) use context::current_context;

#[cfg(test)]
pub(in crate::o11y) use context::{current_span, depth};

/// Whether spans are being recorded: `false` before [`init`](super::init), after [`shutdown`](super::shutdown), and
/// when telemetry was disabled.
///
/// One relaxed atomic load. [`span!`] and [`start`] check it themselves; it is public for the caller that would
/// otherwise do expensive work to build a span's fields only for the span to be discarded.
#[inline]
pub fn enabled() -> bool {
    super::gate::tracing_enabled()
}

/// Attaches `span` to `future` for `#[instrument]` on an `async fn`. Macro plumbing; not API.
#[doc(hidden)]
pub fn __instrument<F: std::future::Future>(span: Span, future: F) -> Instrumented<F> {
    future.instrument(span)
}
