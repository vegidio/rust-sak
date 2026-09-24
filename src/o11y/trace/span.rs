use std::borrow::Cow;

use super::super::pipeline;
use super::super::record::{Fields, SpanId, now_unix_nano};
use super::context::{self, SpanState};

/// An open span, which closes when it is dropped.
///
/// Created by [`span!`](crate::o11y::trace::span). Binding it to a variable is what keeps it open:
///
/// ```
/// use rust_sak::o11y::trace;
///
/// fn charge_card(order_id: &str) -> Result<(), std::fmt::Error> {
///     let _span = trace::span!("charge_card", order_id = order_id);
///     Ok(())
/// }
/// ```
///
/// The span closes on every path out of the scope — a normal return, an early return through `?`, and a panic
/// unwinding through the frame. It does **not** close if the process aborts or calls
/// [`exit`](std::process::exit), since neither runs destructors.
///
/// **Do not hold one across an `.await`.** A task can resume on a different thread, where this span is not on the
/// stack, while the span stays open on the original thread and mis-parents whatever runs there next. Use
/// [`Instrument::instrument`](super::Instrument::instrument) or `#[instrument]` on the `async fn` instead.
#[derive(Debug)]
#[must_use = "a span closes as soon as it is dropped; bind it to a variable to keep it open"]
pub struct Span {
    /// The stack entry this guard owns, or `None` when tracing was off when it was created.
    entered: Option<SpanId>,
}

impl Span {
    /// Opens a span and pushes it onto this thread's stack. Macro plumbing; not API.
    #[doc(hidden)]
    pub fn __enter(name: impl Into<Cow<'static, str>>, attributes: Fields) -> Self {
        let (trace_id, parent_span_id) = context::inherit();
        let state = SpanState::open(trace_id, parent_span_id, name.into(), attributes);
        let span_id = state.span_id;

        context::push(state);

        Self { entered: Some(span_id) }
    }

    /// A span that records nothing, returned when tracing is off. Macro plumbing; not API.
    #[doc(hidden)]
    pub fn __disabled() -> Self {
        Self { entered: None }
    }

    /// Whether this span is recording.
    pub fn is_recording(&self) -> bool {
        self.entered.is_some()
    }

    /// Takes the span off the stack without closing it, handing ownership to a future.
    pub(super) fn detach(mut self) -> PendingSpan {
        // Taken so this guard's own `Drop` becomes a no-op; the `PendingSpan` owns the span from here on.
        let id = self.entered.take();

        PendingSpan {
            state: id.and_then(context::remove),
            id,
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        // This runs during unwinding, where a second panic would abort the process, so nothing in the path from
        // here to the buffer is allowed to panic. Every lock it touches recovers from poisoning rather than
        // unwrapping, and a missing stack entry is simply ignored.
        if let Some(id) = self.entered.take()
            && let Some(state) = context::remove(id)
        {
            pipeline::record_span(state.finish(now_unix_nano()));
        }
    }
}

/// A span that has been lifted off the thread stack, to be re-entered around each poll of a future.
#[derive(Debug)]
pub(super) struct PendingSpan {
    /// The span's identifier, or `None` when tracing was off.
    id: Option<SpanId>,
    /// The span's state while it is *not* on a thread's stack. `None` while entered.
    state: Option<SpanState>,
}

impl PendingSpan {
    /// Pushes the span onto the current thread's stack for the duration of one poll.
    pub(super) fn enter(&mut self) {
        if let Some(state) = self.state.take() {
            context::push(state);
        }
    }

    /// Takes the span back off the current thread's stack when the poll returns.
    pub(super) fn exit(&mut self) {
        if let Some(id) = self.id {
            self.state = context::remove(id);
        }
    }

    /// Closes the span and hands it to the exporter. Idempotent.
    pub(super) fn finish(&mut self) {
        self.exit();

        if let Some(state) = self.state.take() {
            pipeline::record_span(state.finish(now_unix_nano()));
        }

        self.id = None;
    }
}

impl Drop for PendingSpan {
    fn drop(&mut self) {
        // A future that is dropped before it resolves still closes its span, so an abandoned request shows up as a
        // span that ended rather than as one that never returned.
        self.finish();
    }
}
