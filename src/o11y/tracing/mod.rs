//! Forwarding `tracing` into `o11y`: events become log records, spans become spans.
//!
//! A program that already logs through `tracing` gets `o11y`'s exporter by adding one layer to its subscriber, with
//! no second set of calls:
//!
//! ```no_run
//! use rust_sak::o11y::{self, Config, NO_HEADERS};
//! use tracing_subscriber::layer::SubscriberExt;
//!
//! # fn run() -> Result<(), Box<dyn std::error::Error>> {
//! o11y::init(Config::builder("https://collector.example.com", NO_HEADERS).service_name("my-app").build())?;
//!
//! let subscriber = tracing_subscriber::registry().with(o11y::tracing::layer());
//! tracing::subscriber::set_global_default(subscriber)?;
//!
//! tracing::info!(order_id = "ord_8812", "order received");
//! # Ok(())
//! # }
//! ```
//!
//! Parentage comes from `tracing`'s own span tree, not from the thread a record happens to be emitted on, so a span
//! entered on one thread and closed on another — or an event given an explicit parent — lands where `tracing` says
//! it belongs. Each `tracing` span that is exported holds an [`OwnedSpan`] in its extensions; each event is emitted
//! with [`log::emit_in`] against the nearest one.
//!
//! A span with no exported ancestor is parented to this thread's `o11y` stack — unless it opens inside
//! [`with_parent`], which hands it a context from outside the process, typically parsed from a `traceparent` header.
//! The span then continues that trace, and its own children follow it through the span tree as usual.
//!
//! What reaches the layer is the consumer's choice, made with a per-layer filter, as it is for any other layer.
//!
//! # Rules the layer keeps
//!
//! - **Gated.** Every hook asks [`log::enabled`] or [`trace::enabled`] first, so before [`init`](super::init) and
//!   after [`shutdown`](super::shutdown) the layer costs one atomic load per callsite and records nothing.
//! - **Never panics.** It can run inside a C library's logging callback — ONNX Runtime's, through `ort` — under
//!   frames that cannot unwind. The hooks the consumer supplies run behind `catch_unwind`, every hook as a whole does
//!   too, and none of its own code can panic: no `unwrap`, no `expect`, no indexing, which the lints below enforce.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use ::tracing::field::{Field, Visit};
use ::tracing::span::{Attributes, Id, Record};
use ::tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::{LookupSpan, SpanRef};

use super::log::{self, Fields};
use super::trace::{self, OwnedSpan, Parent, SpanContext};
use super::{Level, Value};

/// The field whose presence marks a span as failed.
const ERROR_FIELD: &str = "error";

/// The field the bridge adds to every log record, carrying the event's `tracing` target.
const TARGET_FIELD: &str = "target";

/// `tracing`'s name for an event's formatted message.
const MESSAGE_FIELD: &str = "message";

thread_local! {
    /// The remote context [`with_parent`] set on this thread, for the spans opened while it runs.
    ///
    /// `const`-initialised, so a thread that never calls [`with_parent`] pays nothing to read it.
    static REMOTE_PARENT: Cell<Option<SpanContext>> = const { Cell::new(None) };
}

/// The hook [`TracingLayer::map_field`] installs.
type MapField = dyn Fn(&str, Value) -> Option<Value> + Send + Sync;

/// The hook [`TracingLayer::fold_spans`] installs.
type FoldSpans = dyn Fn(&Metadata<'_>) -> bool + Send + Sync;

/// A layer that forwards `tracing` events and spans to `o11y`, with neither hook installed.
pub fn layer() -> TracingLayer {
    TracingLayer {
        map_field: None,
        fold_spans: None,
    }
}

/// Runs `open` so that a span the layer opens inside it, on this thread, with no exported `tracing` ancestor,
/// continues `context`'s trace as a child of `context`'s span.
///
/// It is how a `tracing` span joins a trace started elsewhere — by a frontend, or another service — whose context
/// arrived as a W3C `traceparent`:
///
/// ```
/// use rust_sak::o11y::{self, trace::SpanContext};
///
/// # let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
/// let span = match SpanContext::from_traceparent(header) {
///     Some(context) => o11y::tracing::with_parent(context, || tracing::info_span!("request")),
///     None => tracing::info_span!("request"),
/// };
/// ```
///
/// - **An exported ancestor still wins.** The context stands in for "nothing local above this span"; it does not
///   re-parent a span that already has a local parent. It does win over this thread's `o11y` guard stack.
/// - **Spans only.** An event emitted inside `open` is routed as it would be without it: a remote span is not one
///   this process can emit into.
/// - **Restored on exit**, including when `open` panics, so nesting restores the outer context and nothing is left
///   behind for the thread's next span.
/// - **Nothing is set while tracing is off.** `open` simply runs.
pub fn with_parent<T>(context: SpanContext, open: impl FnOnce() -> T) -> T {
    if !trace::enabled() {
        return open();
    }

    // Unreachable only while the thread's locals are being destroyed, where running `open` unparented is the most
    // the bridge can do.
    let Ok(previous) = REMOTE_PARENT.try_with(|remote| remote.replace(Some(context))) else {
        return open();
    };

    let _restore = Restore(previous);
    open()
}

/// Puts back the remote context [`with_parent`] replaced, when it returns or unwinds.
struct Restore(Option<SpanContext>);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = REMOTE_PARENT.try_with(|remote| remote.set(self.0));
    }
}

/// The remote context [`with_parent`] set on this thread, if it is running.
fn remote_parent() -> Option<SpanContext> {
    REMOTE_PARENT.try_with(Cell::get).ok().flatten()
}

/// The `tracing_subscriber` layer that feeds `o11y`. Built by [`layer`], and configured by its two hooks.
///
/// | `tracing`                                   | `o11y`                                                                  |
/// |---------------------------------------------|-------------------------------------------------------------------------|
/// | `ERROR` / `WARN` / `INFO`                   | [`Level::Error`] / [`Level::Warn`] / [`Level::Info`]                    |
/// | `DEBUG`, `TRACE`                            | [`Level::Debug`], which is the finest `o11y` has                        |
/// | an event's `message`                        | the record's body; an event with none uses its callsite name            |
/// | an event's other fields                     | the record's fields, plus `target`, the event's target                  |
/// | `i64`, `u64`, `f64`, `bool`, `&str`         | the matching [`Value`]; a `u64` above `i64::MAX`, and `Debug`, a string |
/// | an event's parent span                      | the nearest exported span up its tree, else this thread's `o11y` stack  |
/// | a new span                                  | [`trace::start`] under the nearest exported ancestor, else under the    |
/// |                                             | context [`with_parent`] set, else under this thread's `o11y` stack      |
/// | a span's recorded fields                    | its attributes; one named `error` also marks it failed                  |
/// | `follows_from`                              | a link                                                                  |
/// | an `ERROR` event inside a span              | marks the nearest exported span failed                                  |
/// | a span closing                              | the span ending: it lasts from creation to close                        |
#[derive(Clone)]
pub struct TracingLayer {
    /// Rewrites or drops each field on its way out.
    map_field: Option<Arc<MapField>>,
    /// Picks the spans that are folded into their events rather than exported.
    fold_spans: Option<Arc<FoldSpans>>,
}

impl fmt::Debug for TracingLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracingLayer")
            .field("map_field", &self.map_field.is_some())
            .field("fold_spans", &self.fold_spans.is_some())
            .finish()
    }
}

impl TracingLayer {
    /// Rewrites or drops every field of an event or a span before it leaves the process.
    ///
    /// `map` is given the field's name and value, and returns the value to send, or `None` to send nothing. It is
    /// the place to strip what must not reach a collector:
    ///
    /// ```
    /// use rust_sak::o11y::{self, Value};
    ///
    /// let layer = o11y::tracing::layer()
    ///     .map_field(|name, value| if name.ends_with("path") { None } else { Some(value) });
    /// ```
    ///
    /// A `map` that panics drops the field it panicked on, rather than letting it through unrewritten.
    #[must_use]
    pub fn map_field(mut self, map: impl Fn(&str, Value) -> Option<Value> + Send + Sync + 'static) -> Self {
        self.map_field = Some(Arc::new(map));
        self
    }

    /// Folds the spans `fold` picks into the events inside them, instead of exporting them.
    ///
    /// A folded span is not exported. Its fields are copied onto every event inside it, the innermost value winning
    /// when two spans share a key, and its children are parented to the nearest exported span above it. It is for
    /// spans that only carry context — one wrapping every record a library emits, say — which would otherwise each
    /// become a zero-length span of their own:
    ///
    /// ```
    /// use rust_sak::o11y;
    ///
    /// let layer = o11y::tracing::layer().fold_spans(|metadata| metadata.target() == "ort");
    /// ```
    ///
    /// A `fold` that panics exports the span.
    #[must_use]
    pub fn fold_spans(mut self, fold: impl Fn(&Metadata<'_>) -> bool + Send + Sync + 'static) -> Self {
        self.fold_spans = Some(Arc::new(fold));
        self
    }

    /// Runs `fields` through [`map_field`](Self::map_field), also reporting whether an `error` field was among them
    /// before it ran, and what it became.
    ///
    /// The flag is separate from the value because a map that drops the error's text must not also hide that the
    /// span failed.
    fn map_fields(&self, fields: Fields) -> (Fields, Option<String>) {
        let failed = fields.iter().any(|(name, _)| name == ERROR_FIELD);

        let fields: Fields = match &self.map_field {
            None => fields,
            Some(map) => fields
                .into_iter()
                .filter_map(|(name, value)| {
                    // A panicking map drops the field. Sending the value it failed to rewrite is exactly what the
                    // map was installed to prevent.
                    let mapped = catch_unwind(AssertUnwindSafe(|| map(&name, value))).ok().flatten()?;
                    Some((name, mapped))
                })
                .collect(),
        };

        let error = failed.then(|| {
            fields
                .iter()
                .find(|(name, _)| name == ERROR_FIELD)
                .map(|(_, value)| describe(value))
                .unwrap_or_default()
        });

        (fields, error)
    }

    /// Whether the span `metadata` describes is folded.
    fn folds(&self, metadata: &Metadata<'_>) -> bool {
        self.fold_spans
            .as_ref()
            .is_some_and(|fold| catch_unwind(AssertUnwindSafe(|| fold(metadata))).unwrap_or(false))
    }

    /// Opens the `o11y` side of a new span, or records its fields for folding.
    fn new_span<S>(&self, attributes: &Attributes<'_>, id: &Id, ctx: &Context<'_, S>)
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let Some(span) = ctx.span(id) else { return };

        let mut collected = Collect::span();
        attributes.record(&mut collected);
        let (fields, error) = self.map_fields(collected.fields);

        if self.folds(span.metadata()) {
            span.extensions_mut().replace(Folded(fields));
            return;
        }

        let parent = span
            .parent()
            .and_then(|parent| nearest_exported(&parent))
            .or_else(remote_parent)
            .map_or(Parent::Current, Parent::Context);

        let owned = trace::start(span.name(), parent, fields);

        if let Some(message) = error {
            owned.fail(message);
        }

        span.extensions_mut().replace(Exported(owned));
    }

    /// Adds fields recorded after a span was created.
    fn record<S>(&self, id: &Id, values: &Record<'_>, ctx: &Context<'_, S>)
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let Some(span) = ctx.span(id) else { return };

        let mut collected = Collect::span();
        values.record(&mut collected);
        let (fields, error) = self.map_fields(collected.fields);

        let mut extensions = span.extensions_mut();

        if let Some(Exported(owned)) = extensions.get_mut::<Exported>() {
            for (name, value) in fields {
                owned.set_attribute(name, value);
            }

            if let Some(message) = error {
                owned.fail(message);
            }
        } else if let Some(Folded(folded)) = extensions.get_mut::<Folded>() {
            for (name, value) in fields {
                match folded.iter_mut().find(|(existing, _)| *existing == name) {
                    Some((_, slot)) => *slot = value,
                    None => folded.push((name, value)),
                }
            }
        }
    }

    /// Links a span to the one it follows from.
    fn follows_from<S>(&self, id: &Id, follows: &Id, ctx: &Context<'_, S>)
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let (Some(span), Some(follows)) = (ctx.span(id), ctx.span(follows)) else {
            return;
        };
        let Some(link) = nearest_exported(&follows) else { return };

        if let Some(Exported(owned)) = span.extensions().get::<Exported>() {
            owned.add_link(link);
        }
    }

    /// Emits an event as a log record, under the nearest exported span and carrying every folded span's fields.
    fn event<S>(&self, event: &Event<'_>, level: Level, ctx: &Context<'_, S>)
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let metadata = event.metadata();

        let mut collected = Collect::event();
        event.record(&mut collected);
        let (mut fields, _) = self.map_fields(collected.fields);
        let body = collected.message.unwrap_or_else(|| metadata.name().to_string());

        let mut context = None;

        // Innermost first, so a key already present — the event's own, or a nearer folded span's — is kept.
        if let Some(span) = ctx.event_span(event) {
            for ancestor in span.scope() {
                let extensions = ancestor.extensions();

                if let Some(Folded(folded)) = extensions.get::<Folded>() {
                    for (name, value) in folded {
                        if !fields.iter().any(|(existing, _)| existing == name) {
                            fields.push((name.clone(), value.clone()));
                        }
                    }
                } else if context.is_none()
                    && let Some(Exported(owned)) = extensions.get::<Exported>()
                    && let Some(found) = owned.context()
                {
                    context = Some(found);

                    if level == Level::Error {
                        owned.fail(body.clone());
                    }
                }
            }
        }

        if !fields.iter().any(|(name, _)| name == TARGET_FIELD) {
            fields.push((
                Cow::Borrowed(TARGET_FIELD),
                Value::String(metadata.target().to_string()),
            ));
        }

        match context {
            Some(context) => log::emit_in(context, level, body, fields),
            // No exported `tracing` span encloses the event, but an `o11y` guard span may be open on this thread,
            // and `emit` still correlates with that.
            None => log::emit(level, body, fields),
        }
    }
}

impl<S> Layer<S> for TracingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if trace::enabled() {
            shield(|| self.new_span(attributes, id, &ctx));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if trace::enabled() {
            shield(|| self.record(id, values, &ctx));
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<'_, S>) {
        if trace::enabled() {
            shield(|| self.follows_from(id, follows, &ctx));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let level = level_of(event.metadata().level());

        if log::enabled(level) {
            shield(|| self.event(event, level, &ctx));
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        // With the gate shut there is nothing to do: the registry drops the extension along with the span, and an
        // `OwnedSpan` dropped after `shutdown` discards itself.
        if trace::enabled() {
            shield(|| {
                if let Some(span) = ctx.span(&id) {
                    let exported = span.extensions_mut().remove::<Exported>();
                    drop(exported);
                }
            });
        }
    }
}

/// An exported span's `o11y` counterpart, kept in the `tracing` span's extensions.
struct Exported(OwnedSpan);

/// A folded span's fields, kept in its extensions until the events inside it pick them up.
struct Folded(Fields);

/// The context of the nearest exported span, starting at `span` itself and walking up.
fn nearest_exported<S>(span: &SpanRef<'_, S>) -> Option<SpanContext>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    span.scope().find_map(|ancestor| {
        ancestor
            .extensions()
            .get::<Exported>()
            .and_then(|Exported(owned)| owned.context())
    })
}

/// The `o11y` level for a `tracing` one. `o11y` has nothing finer than `Debug`, so `TRACE` joins it.
fn level_of(level: &::tracing::Level) -> Level {
    if *level == ::tracing::Level::ERROR {
        Level::Error
    } else if *level == ::tracing::Level::WARN {
        Level::Warn
    } else if *level == ::tracing::Level::INFO {
        Level::Info
    } else {
        Level::Debug
    }
}

/// A value as the text of a status message.
fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Int(number) => number.to_string(),
        Value::Double(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => format!("{other:?}"),
    }
}

/// Runs a hook, swallowing any panic rather than letting it unwind into `tracing` — or through a foreign frame.
fn shield(hook: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(hook));
}

/// Collects `tracing` field values as `o11y` fields.
struct Collect {
    /// Whether a field named `message` is the record's body (an event) or an ordinary field (a span).
    message_is_body: bool,
    /// The event's message, when `message_is_body` and there was one.
    message: Option<String>,
    /// Every other field.
    fields: Fields,
}

impl Collect {
    /// A collector for an event's fields.
    fn event() -> Self {
        Self {
            message_is_body: true,
            message: None,
            fields: Vec::new(),
        }
    }

    /// A collector for a span's fields.
    fn span() -> Self {
        Self {
            message_is_body: false,
            ..Self::event()
        }
    }

    /// Keeps one field, diverting the message to the body where that applies.
    fn keep(&mut self, field: &Field, value: Value) {
        match value {
            Value::String(text) if self.message_is_body && field.name() == MESSAGE_FIELD => self.message = Some(text),
            value => self.fields.push((Cow::Borrowed(field.name()), value)),
        }
    }
}

impl Visit for Collect {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.keep(field, Value::Int(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        // OTLP has no unsigned integer. `Value::from` would saturate; a string keeps the number exact.
        let value = i64::try_from(value).map_or_else(|_| Value::String(value.to_string()), Value::Int);
        self.keep(field, value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.keep(field, Value::Double(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.keep(field, Value::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.keep(field, Value::String(value.to_string()));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.keep(field, Value::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.keep(field, Value::String(format!("{value:?}")));
    }
}
