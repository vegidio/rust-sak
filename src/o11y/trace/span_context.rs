//! The public identity of a span: its trace id and its own id, and their W3C `traceparent` form.

use super::super::record::{SpanId, TraceId};

/// Identifies one span within one trace.
///
/// It is what crosses a boundary the span stack cannot: another thread, another process, or a frontend that started
/// the trace. [`to_traceparent`](Self::to_traceparent) and [`from_traceparent`](Self::from_traceparent) carry it in
/// the W3C trace-context header, and [`trace::start`](super::start) and [`log::emit_in`](super::super::log::emit_in)
/// take it as an explicit parent.
///
/// ```
/// use rust_sak::o11y::trace::SpanContext;
///
/// let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
/// let context = SpanContext::from_traceparent(header).unwrap();
///
/// assert_eq!(context.trace_id_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
/// assert_eq!(context.span_id_hex(), "00f067aa0ba902b7");
/// assert_eq!(context.to_traceparent(), header);
/// ```
///
/// Neither id is ever all zeroes: the specification reserves that value as invalid, the generator never produces it,
/// and the parser refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanContext {
    /// The trace the span belongs to.
    pub(in crate::o11y) trace_id: TraceId,
    /// The span's own id.
    pub(in crate::o11y) span_id: SpanId,
}

impl SpanContext {
    /// The 16-byte trace id.
    pub fn trace_id(&self) -> [u8; 16] {
        self.trace_id.0
    }

    /// The 8-byte span id.
    pub fn span_id(&self) -> [u8; 8] {
        self.span_id.0
    }

    /// The trace id as 32 lowercase hex digits.
    pub fn trace_id_hex(&self) -> String {
        self.trace_id.to_hex()
    }

    /// The span id as 16 lowercase hex digits.
    pub fn span_id_hex(&self) -> String {
        self.span_id.to_hex()
    }

    /// The W3C `traceparent` header value: `00-<trace id>-<span id>-01`.
    ///
    /// The flags are always `01`, sampled: this module does not sample, so every span it reports is recorded.
    pub fn to_traceparent(&self) -> String {
        format!("00-{}-{}-01", self.trace_id_hex(), self.span_id_hex())
    }

    /// Parses a W3C `traceparent` header value, or returns `None` if it is not one.
    ///
    /// Follows the parsing rules the specification sets for a receiver:
    ///
    /// - The version is any two lowercase hex digits except `ff`, which is reserved as invalid. A version other than
    ///   `00` may carry more fields after the first four, separated by `-`; they are ignored. Version `00` may not.
    /// - The ids are lowercase hex of exactly 32 and 16 digits, and neither may be all zeroes.
    /// - The flags must be two hex digits, but their value is ignored.
    pub fn from_traceparent(header: &str) -> Option<Self> {
        let header = header.trim();
        let mut parts = header.split('-');

        let version = parts.next()?;
        let trace_id = parts.next()?;
        let span_id = parts.next()?;
        let flags = parts.next()?;

        if !is_lower_hex(version, 2) || version == "ff" || !is_lower_hex(flags, 2) {
            return None;
        }

        // Version 00 has exactly four fields. A later version may append its own, which a receiver that only knows 00
        // is told to skip rather than refuse.
        if version == "00" && parts.next().is_some() {
            return None;
        }

        let trace_id = TraceId(decode(trace_id)?);
        let span_id = SpanId(decode(span_id)?);

        if trace_id.0 == [0; 16] || span_id.0 == [0; 8] {
            return None;
        }

        Some(Self { trace_id, span_id })
    }
}

/// Whether `text` is exactly `len` lowercase hex digits.
fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Decodes exactly `N` bytes of lowercase hex, refusing any other length or case.
fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    if !is_lower_hex(text, N * 2) {
        return None;
    }

    let mut bytes = [0; N];
    hex::decode_to_slice(text, &mut bytes).ok()?;

    Some(bytes)
}
