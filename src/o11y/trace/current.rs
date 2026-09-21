use std::borrow::Cow;

use super::super::Value;
use super::super::record::{Fields, SpanEvent, now_unix_nano};
use super::context;

/// A handle to whichever span is open on this thread right now.
///
/// Obtained from [`current`], it lets code annotate a span it does not own — a helper deep in a call tree can add an
/// attribute to whatever its caller opened, without a span handle being threaded through every signature.
///
/// **Every method is a no-op when no span is open.** That is deliberate: the alternative is an `Option` that every
/// call site has to unwrap, for a failure that is not a failure. Library code can annotate unconditionally, and
/// [`is_recording`](Current::is_recording) is there for the caller who does need to know.
///
/// ```
/// use rust_sak::o11y::trace;
///
/// let _span = trace::span!("charge_card");
/// trace::current().set_attribute("order_id", "ord_8812");
/// trace::current().add_event("submitted to processor");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Current(());

/// The span open on this thread right now.
///
/// The handle borrows nothing and re-reads the stack on every call, so it always means "whatever is current *now*"
/// rather than whatever was current when it was obtained.
pub fn current() -> Current {
    Current(())
}

impl Current {
    /// Attaches an attribute to the open span.
    pub fn set_attribute(&self, key: impl Into<Cow<'static, str>>, value: impl Into<Value>) {
        context::with_current(|span| span.attributes.push((key.into(), value.into())));
    }

    /// Records a point-in-time event on the open span.
    pub fn add_event(&self, name: impl Into<Cow<'static, str>>) {
        self.add_event_with(name, Vec::new());
    }

    /// Records a point-in-time event carrying its own fields.
    pub fn add_event_with(&self, name: impl Into<Cow<'static, str>>, attributes: Fields) {
        context::with_current(|span| {
            span.events.push(SpanEvent {
                time_unix_nano: now_unix_nano(),
                name: name.into(),
                attributes,
            });
        });
    }

    /// Records `error` on the open span, as an `exception` event carrying its `Display` form.
    pub fn set_error<E: std::error::Error + ?Sized>(&self, error: &E) {
        self.add_event_with(
            "exception",
            vec![(Cow::Borrowed("exception.message"), Value::String(error.to_string()))],
        );
    }

    /// Whether a span is open on this thread.
    pub fn is_recording(&self) -> bool {
        context::current_ids().is_some()
    }

    /// The open span's trace id, as lowercase hex.
    pub fn trace_id(&self) -> Option<String> {
        context::current_ids().map(|(trace_id, _)| trace_id.to_hex())
    }

    /// The open span's own id, as lowercase hex.
    pub fn span_id(&self) -> Option<String> {
        context::current_ids().map(|(_, span_id)| span_id.to_hex())
    }
}
