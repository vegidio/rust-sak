//! Spans that belong to a value rather than to a thread.

use std::borrow::Cow;
use std::sync::{Mutex, PoisonError};

use super::super::record::{Fields, now_unix_nano};
use super::super::{Value, ids, pipeline};
use super::context::{self, SpanState};
use super::{SpanContext, enabled};

/// Where a span opened with [`start`] sits in its trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    /// Under the innermost span open on this thread — a [`span!`](super::span) guard or an
    /// [`Instrumented`](super::Instrumented) future — or at the root of a new trace if there is none.
    Current,
    /// Under the span `context` names, in its trace, wherever that span lives.
    Context(SpanContext),
    /// At the root of a new trace.
    Root,
}

/// Opens a span that is not tied to any thread, returning the handle that owns it.
///
/// A [`span!`](super::span) guard lives on the stack of the thread that opened it, which is what lets
/// [`current`](super::current) and the log macros find it. Some spans cannot live there: one opened on a thread
/// that hands its work to another, or one tracked by something that is not a scope — a bridge from another tracing
/// API, a request that finishes in a callback. An `OwnedSpan` holds its own state, can be annotated and ended from
/// any thread, and takes its parent from `parent` rather than from the stack.
///
/// ```
/// use rust_sak::o11y::trace::{self, Parent};
///
/// let span = trace::start("encode", Parent::Current, Vec::new());
///
/// std::thread::spawn(move || {
///     span.set_attribute("codec", "avif");
///     span.end();
/// })
/// .join()
/// .unwrap();
/// ```
///
/// When tracing is off, which includes before [`init`](super::super::init), the handle is a no-op: every method is
/// silent and [`context`](OwnedSpan::context) is `None`.
pub fn start(name: impl Into<Cow<'static, str>>, parent: Parent, fields: Fields) -> OwnedSpan {
    if !enabled() {
        return OwnedSpan { inner: None };
    }

    let (trace_id, parent_span_id) = match parent {
        Parent::Current => context::inherit(),
        Parent::Context(context) => (context.trace_id, Some(context.span_id)),
        Parent::Root => (ids::new_trace_id(), None),
    };

    let state = SpanState::open(trace_id, parent_span_id, name.into(), fields);

    OwnedSpan {
        inner: Some(Inner {
            context: state.context(),
            state: Mutex::new(state),
        }),
    }
}

/// A span owned by this handle rather than by a thread. Opened by [`start`]; closed by [`end`](Self::end) or by
/// dropping it, on whichever thread that happens.
///
/// It is `Send + Sync`, so it can be moved to or shared with the thread that finishes the work.
///
/// **It is never "current".** It is not pushed onto any thread's stack, so [`current`](super::current), the log
/// macros and a [`span!`](super::span) opened while it is live do not see it. That is deliberate: making it current
/// on one thread for a while is exactly the thread-bound behaviour it exists to avoid. A caller that wants a record or
/// a child span under it passes its [`context`](Self::context) explicitly, to
/// [`log::emit_in`](super::super::log::emit_in) or [`Parent::Context`].
#[derive(Debug)]
#[must_use = "a span closes as soon as it is dropped; bind it to a variable to keep it open"]
pub struct OwnedSpan {
    /// The live span, or `None` when tracing was off when it was started.
    inner: Option<Inner>,
}

/// An owned span's state.
#[derive(Debug)]
struct Inner {
    /// Its identity, copied out so reading it takes no lock.
    context: SpanContext,
    /// Everything that changes while it is open. A poisoned lock is recovered from rather than propagated: a panic on
    /// one thread that was annotating the span is no reason for the span to fail everywhere else.
    state: Mutex<SpanState>,
}

impl OwnedSpan {
    /// Runs `visit` against the span's state, doing nothing when it is not recording.
    fn with_state(&self, visit: impl FnOnce(&mut SpanState)) {
        if let Some(inner) = &self.inner {
            visit(&mut inner.state.lock().unwrap_or_else(PoisonError::into_inner));
        }
    }

    /// This span's context, or `None` when it is not recording.
    pub fn context(&self) -> Option<SpanContext> {
        self.inner.as_ref().map(|inner| inner.context)
    }

    /// Whether this span is recording, which it is exactly when tracing was on when it was started.
    pub fn is_recording(&self) -> bool {
        self.inner.is_some()
    }

    /// Attaches an attribute to the span, replacing any earlier value under the same key.
    pub fn set_attribute(&self, key: impl Into<Cow<'static, str>>, value: impl Into<Value>) {
        self.with_state(|state| state.set_attribute(key.into(), value.into()));
    }

    /// Records a point-in-time event on the span.
    pub fn add_event(&self, name: impl Into<Cow<'static, str>>) {
        self.add_event_with(name, Vec::new());
    }

    /// Records a point-in-time event carrying its own fields.
    pub fn add_event_with(&self, name: impl Into<Cow<'static, str>>, attributes: Fields) {
        self.with_state(|state| state.add_event(name.into(), attributes));
    }

    /// Links this span to another it is causally related to without being its child — a piece of work it waited on,
    /// or one of several requests that it serves at once.
    pub fn add_link(&self, context: SpanContext) {
        self.with_state(|state| state.links.push(context));
    }

    /// Marks the span as failed and records `error` on it, as an `exception` event carrying its `Display` form.
    pub fn set_error<E: std::error::Error + ?Sized>(&self, error: &E) {
        self.with_state(|state| state.set_error(error.to_string()));
    }

    /// Marks the span as failed with `message`, without recording an event. For a bridge whose failure is already
    /// on the record some other way.
    #[cfg(feature = "o11y-tracing")]
    pub(in crate::o11y) fn fail(&self, message: String) {
        self.with_state(|state| state.fail(message));
    }

    /// Closes the span now. Dropping it does the same.
    pub fn end(self) {
        drop(self);
    }
}

impl Drop for OwnedSpan {
    fn drop(&mut self) {
        // As for a guard, nothing on this path may panic: it runs wherever the last owner let go, which may be during
        // unwinding or under a foreign frame.
        let Some(inner) = self.inner.take() else { return };
        let state = inner.state.into_inner().unwrap_or_else(PoisonError::into_inner);

        // A span that outlives `shutdown` is discarded, like any record emitted after it. There is no export thread
        // left to drain it.
        if enabled() {
            pipeline::record_span(state.finish(now_unix_nano()));
        }
    }
}
