//! The per-thread stack of open spans.

use std::borrow::Cow;
use std::cell::RefCell;

use super::super::enrichment::Attributes;
use super::super::record::{Fields, SpanEvent, SpanId, SpanRecord, TraceId};

thread_local! {
    /// The spans open on this thread, innermost last.
    ///
    /// A stack, because spans nest. Thread-local, because a span's scope is a stretch of one thread's execution —
    /// which is exactly why an `async fn` cannot use a bare guard and needs
    /// [`Instrumented`](super::Instrumented) instead.
    ///
    /// `const`-initialised so that a thread which never opens a span pays nothing to touch it.
    static STACK: RefCell<Vec<SpanState>> = const { RefCell::new(Vec::new()) };
}

/// An open span. The live state lives here rather than in the guard, which is what lets
/// [`current`](super::current) reach it from anywhere in the call tree without a handle being threaded through.
#[derive(Debug)]
pub(super) struct SpanState {
    /// The trace this span belongs to, inherited from its parent.
    pub(super) trace_id: TraceId,
    /// This span's own identifier.
    pub(super) span_id: SpanId,
    /// The enclosing span, absent for a root span.
    pub(super) parent_span_id: Option<SpanId>,
    /// The span name.
    pub(super) name: Cow<'static, str>,
    /// When the span opened.
    pub(super) start_unix_nano: u64,
    /// Fields from the call site, plus anything added later.
    pub(super) attributes: Fields,
    /// Point-in-time events recorded inside the span.
    pub(super) events: Vec<SpanEvent>,
    /// The enrichment current when the span opened.
    pub(super) enrichment: Attributes,
}

impl SpanState {
    /// Closes the span, turning it into the record the exporter sends.
    pub(super) fn finish(self, end_unix_nano: u64) -> SpanRecord {
        SpanRecord {
            trace_id: self.trace_id,
            span_id: self.span_id,
            parent_span_id: self.parent_span_id,
            name: self.name,
            start_unix_nano: self.start_unix_nano,
            end_unix_nano,
            attributes: self.attributes,
            events: self.events,
            enrichment: self.enrichment,
        }
    }
}

/// The trace and span ids of the innermost open span, if there is one.
///
/// This is what stamps a log record with the span it was emitted inside, giving a backend the link between the two.
pub(in crate::o11y) fn current_ids() -> Option<(TraceId, SpanId)> {
    STACK.with_borrow(|stack| stack.last().map(|span| (span.trace_id, span.span_id)))
}

/// The trace id and parent a span opened right now would inherit.
pub(super) fn inherit() -> (TraceId, Option<SpanId>) {
    STACK.with_borrow(|stack| match stack.last() {
        Some(parent) => (parent.trace_id, Some(parent.span_id)),
        None => (super::super::ids::new_trace_id(), None),
    })
}

/// Pushes an open span onto this thread's stack.
pub(super) fn push(state: SpanState) {
    STACK.with_borrow_mut(|stack| stack.push(state));
}

/// Removes the span with `span_id`, wherever it sits on the stack.
///
/// Normally it is the top, since guards drop in reverse order of creation. A guard stored in a struct, or moved into
/// a collection, can drop out of order — in which case the entries above it keep the parent they were given when
/// they opened, which is the honest record of what actually nested inside what.
pub(super) fn remove(span_id: SpanId) -> Option<SpanState> {
    STACK.with_borrow_mut(|stack| {
        let index = stack.iter().rposition(|span| span.span_id == span_id)?;

        Some(stack.remove(index))
    })
}

/// Runs `visit` against the innermost open span, doing nothing if there is none.
pub(super) fn with_current<R>(visit: impl FnOnce(&mut SpanState) -> R) -> Option<R> {
    STACK.with_borrow_mut(|stack| stack.last_mut().map(visit))
}

/// How many spans are open on this thread. Used by the tests to prove entering and exiting balance.
#[cfg(test)]
pub(in crate::o11y) fn depth() -> usize {
    STACK.with_borrow(Vec::len)
}
