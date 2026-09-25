//! The `tracing` bridge, driven through a real `tracing_subscriber` registry and captured in-process.
//!
//! Every test installs its subscriber as the thread's default rather than the global one, so the tests stay
//! independent; the cross-thread test hands the same `Dispatch` to each thread it starts.

use std::borrow::Cow;

use ::tracing::{Dispatch, dispatcher};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

use super::super::record::{LogRecord, SpanRecord, Status};
use super::super::test_support::Capture;
use super::super::trace::SpanContext;
use super::super::{Level, Value, trace};
use super::{TracingLayer, layer, remote_parent, with_parent};

/// A dispatcher with `layer` as its only layer.
fn dispatch(layer: TracingLayer) -> Dispatch {
    Dispatch::new(Registry::default().with(layer))
}

/// Runs `body` with `layer` installed on this thread.
fn with_layer<R>(layer: TracingLayer, body: impl FnOnce() -> R) -> R {
    dispatcher::with_default(&dispatch(layer), body)
}

/// The value of the field `name`, if the record has one.
fn field<'a>(fields: &'a [(Cow<'static, str>, Value)], name: &str) -> Option<&'a Value> {
    fields.iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

/// The one captured span named `name`.
fn only_span(capture: &Capture, name: &str) -> SpanRecord {
    let mut spans = capture.spans_named(name);
    assert_eq!(spans.len(), 1, "expected exactly one span {name:?}");

    spans.remove(0)
}

/// Whether `record` was emitted inside `span`.
fn is_inside(record: &LogRecord, span: &SpanRecord) -> bool {
    record.trace_id == Some(span.trace_id) && record.span_id == Some(span.span_id)
}

// ---------------------------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn each_tracing_level_maps_onto_an_o11y_level() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::error!("level error");
        ::tracing::warn!("level warn");
        ::tracing::info!("level info");
        ::tracing::debug!("level debug");
        ::tracing::trace!("level trace");
    });

    assert_eq!(capture.log_named("level error").level, Level::Error);
    assert_eq!(capture.log_named("level warn").level, Level::Warn);
    assert_eq!(capture.log_named("level info").level, Level::Info);
    assert_eq!(capture.log_named("level debug").level, Level::Debug);
    assert_eq!(
        capture.log_named("level trace").level,
        Level::Debug,
        "o11y has nothing finer than debug"
    );
}

#[test]
fn an_event_below_the_o11y_threshold_is_not_forwarded() {
    let capture = Capture::start(Level::Warn);

    with_layer(layer(), || {
        ::tracing::info!("below the threshold");
        ::tracing::warn!("at the threshold");
    });

    let logs = capture.logs();
    assert!(logs.iter().all(|record| record.body != "below the threshold"));
    assert!(logs.iter().any(|record| record.body == "at the threshold"));
}

#[test]
fn the_message_is_the_body_and_an_event_without_one_uses_its_callsite_name() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::info!(order_id = "ord_8812", "order {} received", 8812);
        ::tracing::info!(no_message_marker = true);
    });

    let formatted = capture.log_named("order 8812 received");
    assert!(
        field(&formatted.fields, "message").is_none(),
        "the message is the body, not also a field"
    );

    let unnamed = capture
        .logs()
        .into_iter()
        .find(|record| field(&record.fields, "no_message_marker").is_some())
        .unwrap();
    assert!(
        unnamed.body.starts_with("event "),
        "a callsite name, got {:?}",
        unnamed.body
    );
    assert!(
        unnamed.body.contains("tests.rs"),
        "which names the file, got {:?}",
        unnamed.body
    );
}

#[test]
fn each_field_type_lands_in_its_own_value_variant() {
    #[derive(Debug)]
    struct Opaque;

    let capture = Capture::start(Level::Debug);
    let error = std::fmt::Error;

    with_layer(layer(), || {
        ::tracing::info!(
            signed = -3_i64,
            small = 5_u64,
            huge = u64::MAX,
            ratio = 0.5_f64,
            flag = true,
            text = "avif",
            debugged = ?Opaque,
            displayed = %"shown",
            failure = &error as &(dyn std::error::Error + 'static),
            "typed fields"
        );
    });

    let record = capture.log_named("typed fields");

    assert_eq!(field(&record.fields, "signed"), Some(&Value::Int(-3)));
    assert_eq!(field(&record.fields, "small"), Some(&Value::Int(5)));
    assert_eq!(
        field(&record.fields, "huge"),
        Some(&Value::String(u64::MAX.to_string())),
        "a u64 past i64::MAX keeps its exact value as a string rather than saturating"
    );
    assert_eq!(field(&record.fields, "ratio"), Some(&Value::Double(0.5)));
    assert_eq!(field(&record.fields, "flag"), Some(&Value::Bool(true)));
    assert_eq!(field(&record.fields, "text"), Some(&Value::from("avif")));
    assert_eq!(field(&record.fields, "debugged"), Some(&Value::from("Opaque")));
    assert_eq!(field(&record.fields, "displayed"), Some(&Value::from("shown")));
    assert_eq!(
        field(&record.fields, "failure"),
        Some(&Value::String(error.to_string()))
    );
}

#[test]
fn every_record_carries_the_event_target() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::info!("default target");
        ::tracing::warn!(target: "ort", "explicit target");
    });

    assert_eq!(
        field(&capture.log_named("default target").fields, "target"),
        Some(&Value::from(module_path!()))
    );
    assert_eq!(
        field(&capture.log_named("explicit target").fields, "target"),
        Some(&Value::from("ort"))
    );
}

#[test]
fn an_event_is_attached_to_its_span_or_else_to_the_o11y_stack() {
    let capture = Capture::start(Level::Debug);

    let guard_context = with_layer(layer(), || {
        ::tracing::info_span!("bridged span").in_scope(|| ::tracing::info!("inside a tracing span"));

        let guard = trace::span!("o11y guard");
        let guard_context = trace::current().context().unwrap();
        ::tracing::info!("inside only an o11y guard");
        drop(guard);

        ::tracing::info!("inside nothing");

        guard_context
    });

    let span = only_span(&capture, "bridged span");
    assert!(is_inside(&capture.log_named("inside a tracing span"), &span));

    let inside_guard = capture.log_named("inside only an o11y guard");
    assert_eq!(
        inside_guard.span_id,
        Some(guard_context.span_id),
        "emit still correlates with a guard"
    );

    let outside = capture.log_named("inside nothing");
    assert_eq!(outside.trace_id, None);
    assert_eq!(outside.span_id, None);
}

#[test]
fn an_event_with_an_explicit_parent_goes_to_that_span_not_the_entered_one() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        let elsewhere = ::tracing::info_span!("explicit parent");
        let _entered = ::tracing::info_span!("entered span").entered();

        ::tracing::info!(parent: &elsewhere, "explicitly parented");
    });

    let parent = only_span(&capture, "explicit parent");
    assert!(is_inside(&capture.log_named("explicitly parented"), &parent));
}

#[test]
fn map_field_rewrites_and_drops_fields_of_events_and_spans() {
    let capture = Capture::start(Level::Debug);

    let redacting = layer().map_field(|name, value| match name {
        "path" => None,
        "user" => Some(Value::from("redacted")),
        _ => Some(value),
    });

    with_layer(redacting, || {
        ::tracing::info_span!("mapped span", path = "/Users/ada/photo.jpg", user = "ada", size = 3).in_scope(|| {
            ::tracing::info!(path = "/Users/ada/photo.jpg", user = "ada", size = 3, "mapped event");
        });
    });

    let event = capture.log_named("mapped event");
    assert_eq!(field(&event.fields, "path"), None);
    assert_eq!(field(&event.fields, "user"), Some(&Value::from("redacted")));
    assert_eq!(field(&event.fields, "size"), Some(&Value::Int(3)));

    let span = only_span(&capture, "mapped span");
    assert_eq!(field(&span.attributes, "path"), None);
    assert_eq!(field(&span.attributes, "user"), Some(&Value::from("redacted")));
    assert_eq!(field(&span.attributes, "size"), Some(&Value::Int(3)));
}

// ---------------------------------------------------------------------------------------------------------------
// Spans
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_nested_tracing_span_is_parented_by_its_tracing_parent() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::info_span!("tracing outer", model = "osaka").in_scope(|| {
            ::tracing::debug_span!("tracing inner").in_scope(|| {});
        });
    });

    let outer = only_span(&capture, "tracing outer");
    let inner = only_span(&capture, "tracing inner");

    assert_eq!(outer.parent_span_id, None);
    assert_eq!(field(&outer.attributes, "model"), Some(&Value::from("osaka")));
    assert_eq!(inner.trace_id, outer.trace_id);
    assert_eq!(inner.parent_span_id, Some(outer.span_id));
}

#[test]
fn a_tracing_span_with_no_tracing_parent_nests_under_the_open_o11y_guard() {
    let capture = Capture::start(Level::Debug);

    let guard_context = with_layer(layer(), || {
        let _guard = trace::span!("guard around tracing");
        let context = trace::current().context().unwrap();
        ::tracing::info_span!("tracing under guard").in_scope(|| {});

        context
    });

    let span = only_span(&capture, "tracing under guard");
    assert_eq!(span.trace_id, guard_context.trace_id);
    assert_eq!(span.parent_span_id, Some(guard_context.span_id));
}

#[test]
fn fields_recorded_later_become_attributes_and_an_error_field_fails_the_span() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        let span = ::tracing::info_span!(
            "recorded later",
            bytes = ::tracing::field::Empty,
            error = ::tracing::field::Empty
        );
        span.record("bytes", 1024_u64);
        span.record("error", "decoder refused the file");
    });

    let span = only_span(&capture, "recorded later");
    assert_eq!(field(&span.attributes, "bytes"), Some(&Value::Int(1024)));
    assert_eq!(
        field(&span.attributes, "error"),
        Some(&Value::from("decoder refused the file"))
    );
    assert_eq!(span.status, Status::Error("decoder refused the file".to_string()));
}

#[test]
fn a_field_recorded_twice_on_an_exported_span_keeps_one_attribute_with_the_later_value() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        let span = ::tracing::info_span!("recorded twice", stage = "decode");
        span.record("stage", "encode");
    });

    let span = only_span(&capture, "recorded twice");
    let stages: Vec<_> = span.attributes.iter().filter(|(name, _)| name == "stage").collect();
    assert_eq!(stages.len(), 1);
    assert_eq!(field(&span.attributes, "stage"), Some(&Value::from("encode")));
}

#[test]
fn an_error_field_given_at_creation_fails_the_span_even_when_mapped_out() {
    let capture = Capture::start(Level::Debug);

    let dropping_errors = layer().map_field(|name, value| (name != "error").then_some(value));

    with_layer(dropping_errors, || {
        ::tracing::info_span!("failed at birth", error = "private detail").in_scope(|| {});
    });

    let span = only_span(&capture, "failed at birth");
    assert_eq!(
        field(&span.attributes, "error"),
        None,
        "the map still decides what leaves the process"
    );
    assert_eq!(
        span.status,
        Status::Error(String::new()),
        "but the span is still marked failed"
    );
}

#[test]
fn an_error_event_inside_a_span_fails_the_nearest_exported_span() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::info_span!("fails by event").in_scope(|| {
            ::tracing::warn!("only a warning");
            ::tracing::error!("session build failed");
        });
        ::tracing::info_span!("only warns").in_scope(|| ::tracing::warn!("just a warning"));
    });

    assert_eq!(
        only_span(&capture, "fails by event").status,
        Status::Error("session build failed".to_string())
    );
    assert_eq!(only_span(&capture, "only warns").status, Status::Unset);
}

#[test]
fn a_span_created_entered_and_closed_on_three_threads_exports_once_with_its_parent_and_link() {
    let capture = Capture::start(Level::Debug);
    let dispatch = dispatch(layer());

    // Created on this thread, with its parent and the span it follows from.
    let (span, cause) = dispatcher::with_default(&dispatch, || {
        let cause = ::tracing::info_span!("cause");
        let parent = ::tracing::info_span!("three-thread parent");
        let span = ::tracing::info_span!(parent: &parent, "three threads");
        span.follows_from(&cause);

        (span, cause)
    });

    // Entered on a second.
    let entering = span.clone();
    let second = dispatch.clone();
    std::thread::spawn(move || {
        dispatcher::with_default(&second, || {
            entering.in_scope(|| ::tracing::info!("emitted on the second thread"))
        });
    })
    .join()
    .unwrap();

    // Closed on a third, which holds the last handle.
    let third = dispatch.clone();
    std::thread::spawn(move || dispatcher::with_default(&third, move || drop(span)))
        .join()
        .unwrap();

    dispatcher::with_default(&dispatch, || drop(cause));

    let span = only_span(&capture, "three threads");
    let parent = only_span(&capture, "three-thread parent");
    let cause = only_span(&capture, "cause");

    assert_eq!(span.trace_id, parent.trace_id);
    assert_eq!(span.parent_span_id, Some(parent.span_id));
    assert_eq!(span.links.len(), 1);
    assert_eq!(span.links[0].trace_id, cause.trace_id);
    assert_eq!(span.links[0].span_id, cause.span_id);
    assert!(is_inside(&capture.log_named("emitted on the second thread"), &span));
}

// ---------------------------------------------------------------------------------------------------------------
// Remote parents
// ---------------------------------------------------------------------------------------------------------------

/// A context as a frontend would send it.
fn remote(header: &str) -> SpanContext {
    SpanContext::from_traceparent(header).unwrap()
}

const WINDOW: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const OUTER_WINDOW: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

#[test]
fn a_span_opened_inside_with_parent_continues_the_remote_trace_under_its_span() {
    let capture = Capture::start(Level::Debug);
    let context = remote(WINDOW);

    with_layer(layer(), || {
        with_parent(context, || ::tracing::info_span!("remote child")).in_scope(|| {});
    });

    let span = only_span(&capture, "remote child");
    assert_eq!(span.trace_id, context.trace_id);
    assert_eq!(span.parent_span_id, Some(context.span_id));
}

#[test]
fn an_exported_ancestor_wins_over_the_remote_context() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        ::tracing::info_span!("local ancestor").in_scope(|| {
            with_parent(remote(WINDOW), || ::tracing::info_span!("locally parented")).in_scope(|| {});
        });
    });

    let ancestor = only_span(&capture, "local ancestor");
    let span = only_span(&capture, "locally parented");
    assert_eq!(span.trace_id, ancestor.trace_id);
    assert_eq!(span.parent_span_id, Some(ancestor.span_id));
}

#[test]
fn the_remote_context_wins_over_an_open_o11y_guard() {
    let capture = Capture::start(Level::Debug);
    let context = remote(WINDOW);

    with_layer(layer(), || {
        let _guard = trace::span!("guard beside a remote parent");
        with_parent(context, || ::tracing::info_span!("remote over guard")).in_scope(|| {});
    });

    let span = only_span(&capture, "remote over guard");
    assert_eq!(span.trace_id, context.trace_id);
    assert_eq!(span.parent_span_id, Some(context.span_id));
}

#[test]
fn a_child_of_a_remotely_parented_span_inherits_the_remote_trace() {
    let capture = Capture::start(Level::Debug);
    let context = remote(WINDOW);

    with_layer(layer(), || {
        // The child opens after `with_parent` has returned, so only the span tree can carry it into the trace.
        let parent = with_parent(context, || ::tracing::info_span!("remote parent"));
        parent.in_scope(|| ::tracing::info_span!("remote grandchild").in_scope(|| {}));
    });

    let parent = only_span(&capture, "remote parent");
    let child = only_span(&capture, "remote grandchild");
    assert_eq!(child.trace_id, context.trace_id);
    assert_eq!(child.parent_span_id, Some(parent.span_id));
}

#[test]
fn nesting_restores_the_outer_context_and_leaving_clears_it() {
    let capture = Capture::start(Level::Debug);
    let outer = remote(OUTER_WINDOW);
    let inner = remote(WINDOW);

    with_layer(layer(), || {
        with_parent(outer, || {
            with_parent(inner, || assert_eq!(remote_parent(), Some(inner)));
            ::tracing::info_span!("after the inner scope").in_scope(|| {});
        });
        assert_eq!(remote_parent(), None);
        ::tracing::info_span!("after both scopes").in_scope(|| {});
    });

    let restored = only_span(&capture, "after the inner scope");
    assert_eq!(restored.trace_id, outer.trace_id);
    assert_eq!(restored.parent_span_id, Some(outer.span_id));

    let cleared = only_span(&capture, "after both scopes");
    assert_ne!(cleared.trace_id, outer.trace_id);
    assert_eq!(cleared.parent_span_id, None);
}

#[test]
fn a_panic_inside_with_parent_restores_the_outer_context() {
    let _capture = Capture::start(Level::Debug);
    let outer = remote(OUTER_WINDOW);

    with_layer(layer(), || {
        with_parent(outer, || {
            let unwound = std::panic::catch_unwind(|| with_parent(remote(WINDOW), || panic!("inside with_parent")));
            assert!(unwound.is_err());
            assert_eq!(remote_parent(), Some(outer));
        });
        assert_eq!(remote_parent(), None);
    });
}

#[test]
fn an_event_inside_with_parent_is_routed_as_without_it() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        with_parent(remote(WINDOW), || {
            ::tracing::info!("remote event outside any span");
            ::tracing::info_span!("span around a remote event").in_scope(|| ::tracing::info!("remote event inside"));
        });
    });

    let outside = capture.log_named("remote event outside any span");
    assert_eq!(
        outside.trace_id, None,
        "a remote span is not one this process can emit into"
    );
    assert_eq!(outside.span_id, None);

    let span = only_span(&capture, "span around a remote event");
    assert!(is_inside(&capture.log_named("remote event inside"), &span));
}

#[test]
fn with_tracing_off_with_parent_sets_nothing() {
    let capture = Capture::start(Level::Debug);
    capture.close_gates();

    let seen = with_layer(layer(), || with_parent(remote(WINDOW), remote_parent));

    assert_eq!(seen, None);
    assert!(capture.spans().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------------------------------------------

/// A layer that folds `ort`'s per-record spans, as a consumer running ONNX Runtime would configure it.
fn folding_ort() -> TracingLayer {
    layer().fold_spans(|metadata| metadata.target() == "ort")
}

#[test]
fn a_folded_span_lends_its_fields_to_its_event_and_is_never_exported() {
    let capture = Capture::start(Level::Debug);

    with_layer(folding_ort(), || {
        let _session = ::tracing::info_span!("session.run", model = "osaka").entered();

        // The shape `ort` produces for every runtime diagnostic: a TRACE span carrying the record's id and source
        // location, given as the event's explicit parent.
        let record = ::tracing::trace_span!(target: "ort", "ort", id = 7, location = "session_state.cc:1136");
        ::tracing::warn!(target: "ort", parent: &record, "runtime diagnostic");
    });

    let session = only_span(&capture, "session.run");
    let event = capture.log_named("runtime diagnostic");

    assert_eq!(
        field(&event.fields, "location"),
        Some(&Value::from("session_state.cc:1136"))
    );
    assert_eq!(field(&event.fields, "id"), Some(&Value::Int(7)));
    assert!(
        is_inside(&event, &session),
        "the event belongs to the exported span around the folded one"
    );
    assert!(capture.spans_named("ort").is_empty(), "a folded span is never exported");
}

#[test]
fn the_innermost_value_wins_and_children_parent_past_a_folded_span() {
    let capture = Capture::start(Level::Debug);

    with_layer(folding_ort(), || {
        let _outer = ::tracing::info_span!("exported outer").entered();
        let _far = ::tracing::trace_span!(target: "ort", "ort", location = "far", only_far = true).entered();
        let _near = ::tracing::trace_span!(target: "ort", "ort", location = "near").entered();

        ::tracing::info!("folded twice");
        ::tracing::info!(location = "own", "own location");
        ::tracing::info_span!("child of folded").in_scope(|| {});
    });

    let outer = only_span(&capture, "exported outer");

    let folded_twice = capture.log_named("folded twice");
    assert_eq!(field(&folded_twice.fields, "location"), Some(&Value::from("near")));
    assert_eq!(field(&folded_twice.fields, "only_far"), Some(&Value::Bool(true)));
    assert!(is_inside(&folded_twice, &outer));

    let own = capture.log_named("own location");
    assert_eq!(
        field(&own.fields, "location"),
        Some(&Value::from("own")),
        "an event's own field beats a span's"
    );

    let child = only_span(&capture, "child of folded");
    assert_eq!(child.parent_span_id, Some(outer.span_id));
}

#[test]
fn a_field_recorded_later_on_a_folded_span_replaces_the_earlier_value() {
    let capture = Capture::start(Level::Debug);

    with_layer(folding_ort(), || {
        let span = ::tracing::trace_span!(target: "ort", "ort", location = "before");
        span.record("location", "after");
        span.in_scope(|| ::tracing::info!("after recording"));
    });

    assert_eq!(
        field(&capture.log_named("after recording").fields, "location"),
        Some(&Value::from("after"))
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The gate and the panic rule
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn nothing_is_recorded_while_telemetry_has_never_been_started() {
    let capture = Capture::start(Level::Debug);
    capture.close_gates();

    with_layer(layer(), || {
        ::tracing::info_span!("never started", id = 1).in_scope(|| ::tracing::error!("never started event"));
    });

    assert!(capture.logs().is_empty());
    assert!(capture.spans().is_empty());
}

#[test]
fn nothing_is_recorded_after_shutdown_even_for_a_span_opened_before_it() {
    let capture = Capture::start(Level::Debug);

    with_layer(layer(), || {
        let before = ::tracing::info_span!("opened before shutdown", late = ::tracing::field::Empty);

        capture.close_gates();

        before.in_scope(|| ::tracing::error!("after shutdown"));
        before.record("late", 1);
        ::tracing::info_span!("opened after shutdown").in_scope(|| {});
        drop(before);
    });

    assert!(capture.logs().is_empty());
    assert!(capture.spans().is_empty());
}

#[test]
fn a_panicking_map_field_drops_the_field_and_does_not_unwind() {
    let capture = Capture::start(Level::Debug);

    let panicking = layer().map_field(|name, value| {
        assert_ne!(name, "boom", "a consumer hook that panics");
        Some(value)
    });

    with_layer(panicking, || {
        ::tracing::info_span!("survives a panicking map", boom = 1, kept = 2).in_scope(|| {
            ::tracing::info!(boom = 1, kept = 2, "survived");
        });
    });

    let event = capture.log_named("survived");
    assert_eq!(field(&event.fields, "boom"), None);
    assert_eq!(field(&event.fields, "kept"), Some(&Value::Int(2)));

    let span = only_span(&capture, "survives a panicking map");
    assert_eq!(field(&span.attributes, "boom"), None);
    assert_eq!(field(&span.attributes, "kept"), Some(&Value::Int(2)));
}

#[test]
fn a_panicking_fold_spans_exports_the_span_and_does_not_unwind() {
    let capture = Capture::start(Level::Debug);

    let panicking = layer().fold_spans(|_| panic!("a consumer hook that panics"));

    with_layer(panicking, || {
        ::tracing::info_span!("fold hook panicked").in_scope(|| {});
    });

    assert_eq!(only_span(&capture, "fold hook panicked").status, Status::Unset);
}
