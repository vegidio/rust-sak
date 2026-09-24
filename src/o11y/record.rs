//! The internal shapes that travel from an emitting thread, through a [`Buffer`](super::buffer::Buffer), to the
//! exporter. None of this is public: it is the wire format's staging area, not an API.

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use super::Level;
use super::Value;
use super::enrichment::Attributes;
use super::trace::SpanContext;

/// A set of key/value pairs attached to a log record, a span or a span event, in the order they were written.
///
/// Public, at [`log::Fields`](super::log::Fields), for the caller that assembles fields at runtime rather than writing
/// them at a macro call site.
pub type Fields = Vec<(Cow<'static, str>, Value)>;

/// Wall-clock nanoseconds since the Unix epoch, which is the only timestamp OTLP accepts.
///
/// A clock set before 1970 saturates to zero rather than panicking. That is a nonsensical reading either way, and a
/// telemetry module has no business taking the process down over one.
pub(super) fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_nanos()).unwrap_or(u64::MAX))
}

/// A 16-byte trace identifier, shared by every span and log record in one trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TraceId(pub(super) [u8; 16]);

/// An 8-byte span identifier, unique within a trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SpanId(pub(super) [u8; 8]);

impl TraceId {
    /// The lowercase hex form OTLP/JSON requires. Note that OTLP/JSON uses hex here, **not** the base64 it uses for
    /// other `bytes` fields — a collector silently rejects the payload if this is got wrong.
    pub(super) fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl SpanId {
    /// The lowercase hex form OTLP/JSON requires. See [`TraceId::to_hex`].
    pub(super) fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

/// One log record, captured at the call site and waiting to be exported.
#[derive(Debug)]
pub(super) struct LogRecord {
    /// When the record was emitted.
    pub(super) time_unix_nano: u64,
    /// Its severity.
    pub(super) level: Level,
    /// The message, which becomes the OTLP record body.
    pub(super) body: String,
    /// The key/value pairs supplied at the call site.
    pub(super) fields: Fields,
    /// The enrichment current when the record was emitted, held by reference so emitting costs a refcount bump.
    pub(super) enrichment: Attributes,
    /// The enclosing span, when there was one. This is what lets a backend put a log next to its trace.
    pub(super) trace_id: Option<TraceId>,
    /// The enclosing span's own id, present exactly when `trace_id` is.
    pub(super) span_id: Option<SpanId>,
}

/// One finished span.
#[derive(Debug)]
pub(super) struct SpanRecord {
    /// The trace this span belongs to.
    pub(super) trace_id: TraceId,
    /// This span's identifier.
    pub(super) span_id: SpanId,
    /// The enclosing span, absent for a root span.
    pub(super) parent_span_id: Option<SpanId>,
    /// The span name.
    pub(super) name: Cow<'static, str>,
    /// When the span opened.
    pub(super) start_unix_nano: u64,
    /// When the guard dropped.
    pub(super) end_unix_nano: u64,
    /// Fields from the call site, plus anything added through [`current`](super::trace::current).
    pub(super) attributes: Fields,
    /// Point-in-time events recorded inside the span.
    pub(super) events: Vec<SpanEvent>,
    /// Other spans this one is causally related to without being their child.
    pub(super) links: Vec<SpanContext>,
    /// Whether the span's work failed.
    pub(super) status: Status,
    /// The enrichment current when the span opened.
    pub(super) enrichment: Attributes,
}

/// A span's outcome, as far as a trace backend is concerned.
///
/// Only failure is ever set. OTLP's third state, `Ok`, means "a person has checked this and it is fine", which no
/// instrumentation can say on its own; an unset status is what a successful span normally carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum Status {
    /// Nothing was said about the outcome. Encoded as no `status` at all.
    #[default]
    Unset,
    /// The work failed, for the reason given.
    Error(String),
}

/// A point-in-time event recorded on a span.
#[derive(Debug)]
pub(super) struct SpanEvent {
    /// When it happened.
    pub(super) time_unix_nano: u64,
    /// What happened.
    pub(super) name: Cow<'static, str>,
    /// Any fields attached to it.
    pub(super) attributes: Fields,
}
