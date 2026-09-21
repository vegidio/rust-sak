use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::buffer::Buffer;
use super::enrichment::Enrichment;
use super::record::{LogRecord, SpanId, TraceId, now_unix_nano};
use super::test_support::{
    captured_at_least, global_lock, spawn_counting_server, spawn_json_server, spawn_recording_collector, wait_until,
};
use super::trace::Span;
use super::*;

/// Collects the errors an export reports, for the tests that assert on them.
#[derive(Debug, Default)]
struct ErrorSink(Mutex<Vec<String>>);

impl ErrorSink {
    /// A handler that appends every error's `Display` form to this sink.
    fn handler(self: &Arc<Self>) -> impl Fn(&O11yError) + Send + Sync + 'static {
        let sink = Arc::clone(self);

        move |error: &O11yError| {
            sink.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(error.to_string());
        }
    }

    /// Everything reported so far.
    fn taken(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The configuration the export-cycle tests share: a collector to post at, a recording error handler, and a
/// timeout short enough that an unreachable endpoint does not stall the suite.
fn test_config(endpoint: &str, sink: &Arc<ErrorSink>) -> Config {
    Config::builder(endpoint, NO_HEADERS)
        .service_name("test-service")
        .service_version("1.2.3")
        .timeout(Duration::from_secs(5))
        .on_export_error(sink.handler())
        .build()
}

/// A finished span, built directly rather than by closing a guard, whose `Drop` routes to the process globals.
fn span_record(name: &'static str) -> record::SpanRecord {
    record::SpanRecord {
        trace_id: TraceId([1; 16]),
        span_id: SpanId([2; 8]),
        parent_span_id: None,
        name: Cow::Borrowed(name),
        start_unix_nano: now_unix_nano(),
        end_unix_nano: now_unix_nano(),
        attributes: Vec::new(),
        events: Vec::new(),
        enrichment: Arc::new(Vec::new()),
    }
}

/// A log record with the given body and no fields, for the buffer and payload tests.
fn log_record(body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: now_unix_nano(),
        level: Level::Info,
        body: body.to_string(),
        fields: vec![(Cow::Borrowed("order_id"), Value::from("ord_8812"))],
        enrichment: Arc::new(Vec::new()),
        trace_id: None,
        span_id: None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The bounded buffer
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_full_buffer_discards_the_oldest_entry() {
    let buffer = Buffer::new(3);

    for index in 0..5 {
        buffer.push(index);
    }

    let (kept, dropped) = buffer.take();

    assert_eq!(kept, vec![2, 3, 4], "the most recent entries are the ones that survive");
    assert_eq!(dropped, 2);
}

#[test]
fn a_buffer_reports_its_drops_once_rather_than_per_drop() {
    let buffer = Buffer::new(1);

    for index in 0..4 {
        buffer.push(index);
    }

    assert_eq!(buffer.take().1, 3);
    assert_eq!(buffer.take().1, 0, "the tally resets once it has been reported");
}

#[test]
fn a_zero_capacity_buffer_still_holds_one_entry() {
    let buffer = Buffer::new(0);
    buffer.push(7);

    assert_eq!(buffer.take().0, vec![7]);
}

#[test]
fn push_reports_the_queue_length_so_the_worker_can_be_woken() {
    let buffer = Buffer::new(8);

    assert_eq!(buffer.push('a'), 1);
    assert_eq!(buffer.push('b'), 2);
    assert_eq!(buffer.len(), 2);
}

// ---------------------------------------------------------------------------------------------------------------
// OTLP/JSON encoding
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn sixty_four_bit_integers_are_encoded_as_json_strings() {
    let payload = otlp::logs(vec![log_record("order received")], &[]);
    let record = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];

    assert!(
        record["timeUnixNano"].is_string(),
        "the proto3 json mapping encodes 64-bit integers as strings, and a collector rejects a bare number",
    );
}

#[test]
fn an_integer_field_is_encoded_as_a_string_too() {
    let mut record = log_record("counted");
    record.fields = vec![(Cow::Borrowed("bytes"), Value::from(1_048_576u64))];

    let payload = otlp::logs(vec![record], &[]);
    let attributes = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"];

    assert_eq!(attributes[0]["value"]["intValue"], "1048576");
}

#[test]
fn a_record_emitted_outside_a_span_carries_no_trace_id() {
    let payload = otlp::logs(vec![log_record("no span")], &[]);
    let record = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];

    assert!(
        record.get("traceId").is_none(),
        "an absent identifier is omitted, not sent as an empty string, which would be malformed",
    );
}

#[test]
fn a_record_emitted_inside_a_span_carries_its_ids_as_lowercase_hex() {
    let mut record = log_record("in a span");
    record.trace_id = Some(TraceId([0xab; 16]));
    record.span_id = Some(SpanId([0xcd; 8]));

    let payload = otlp::logs(vec![record], &[]);
    let encoded = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];

    assert_eq!(encoded["traceId"], "abababababababababababababababab");
    assert_eq!(encoded["spanId"], "cdcdcdcdcdcdcdcd");
}

#[test]
fn records_emitted_under_different_sessions_get_their_own_resource_blocks() {
    let first: enrichment::Attributes = Arc::new(vec![(Cow::Borrowed("session.id"), Value::from("one"))]);
    let second: enrichment::Attributes = Arc::new(vec![(Cow::Borrowed("session.id"), Value::from("two"))]);

    let mut a = log_record("before");
    a.enrichment = Arc::clone(&first);
    let mut b = log_record("after");
    b.enrichment = Arc::clone(&second);
    let mut c = log_record("also before");
    c.enrichment = first;

    let payload = otlp::logs(vec![a, b, c], &[]);
    let blocks = payload["resourceLogs"].as_array().unwrap();

    assert_eq!(
        blocks.len(),
        2,
        "one resource block per distinct enrichment, not per record"
    );
    assert_eq!(blocks[0]["scopeLogs"][0]["logRecords"].as_array().unwrap().len(), 2);
}

#[test]
fn the_service_attributes_go_in_the_resource_block_not_on_each_record() {
    let base = vec![(Cow::Borrowed("service.name"), Value::from("checkout-service"))];
    let payload = otlp::logs(vec![log_record("order received")], &base);

    let attributes = payload["resourceLogs"][0]["resource"]["attributes"].as_array().unwrap();
    assert!(attributes.iter().any(|entry| entry["key"] == "service.name"));

    let record_attributes = payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"]
        .as_array()
        .unwrap();
    assert!(record_attributes.iter().all(|entry| entry["key"] != "service.name"));
}

#[test]
fn a_histogram_has_exactly_one_more_bucket_than_it_has_bounds() {
    let _guard = global_lock();
    metric::clear();

    let histogram = metric::histogram("durations").with_buckets(&[1.0, 2.0]);
    histogram.record(1.5);

    let payload = otlp::metrics(metric::snapshot(), &[], &Arc::new(Vec::new()), 0, 1);
    let point = &payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["histogram"]["dataPoints"][0];

    let bounds = point["explicitBounds"].as_array().unwrap().len();
    let buckets = point["bucketCounts"].as_array().unwrap().len();

    assert_eq!(
        buckets,
        bounds + 1,
        "the extra bucket catches everything above the last bound"
    );
    assert_eq!(
        point["bucketCounts"][1], "1",
        "1.5 falls into the bucket bounded by 2.0"
    );
}

#[test]
fn a_counter_is_reported_as_a_cumulative_monotonic_sum() {
    let _guard = global_lock();
    metric::clear();

    let orders = metric::counter("orders_total");
    orders.increment(3);

    let payload = otlp::metrics(metric::snapshot(), &[], &Arc::new(Vec::new()), 0, 1);
    let sum = &payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["sum"];

    assert_eq!(
        sum["aggregationTemporality"], 2,
        "2 is AGGREGATION_TEMPORALITY_CUMULATIVE"
    );
    assert_eq!(sum["isMonotonic"], true);
    assert_eq!(sum["dataPoints"][0]["asInt"], "3");
}

#[test]
fn every_value_variant_maps_onto_an_otlp_any_value() {
    let mut record = log_record("everything");
    record.fields = vec![
        (Cow::Borrowed("flag"), Value::Bool(true)),
        (Cow::Borrowed("count"), Value::Int(5)),
        (Cow::Borrowed("ratio"), Value::Double(0.5)),
        (Cow::Borrowed("name"), Value::from("avif")),
        (Cow::Borrowed("blob"), Value::Bytes(vec![0, 1, 2])),
        (Cow::Borrowed("list"), Value::from(vec![1i64, 2])),
    ];

    let payload = otlp::logs(vec![record], &[]);
    let attributes = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"];

    assert_eq!(attributes[0]["value"]["boolValue"], true);
    assert_eq!(attributes[1]["value"]["intValue"], "5");
    assert_eq!(attributes[2]["value"]["doubleValue"], 0.5);
    assert_eq!(attributes[3]["value"]["stringValue"], "avif");
    assert_eq!(
        attributes[4]["value"]["bytesValue"], "AAEC",
        "bytes really are base64, unlike the ids"
    );
    assert_eq!(attributes[5]["value"]["arrayValue"]["values"][1]["intValue"], "2");
}

// ---------------------------------------------------------------------------------------------------------------
// The field syntax
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_field_syntax_accepts_all_four_forms() {
    let order_id = "ord_8812";
    let error = std::fmt::Error;

    let fields = crate::__rust_sak_o11y_fields!(
        plain = 1u32,
        display = %error,
        debug = ?error,
        order_id,
    );

    assert_eq!(fields[0], (Cow::Borrowed("plain"), Value::Int(1)));
    assert_eq!(fields[1].1, Value::String(error.to_string()));
    assert_eq!(fields[2].1, Value::String(format!("{error:?}")));
    assert_eq!(
        fields[3],
        (Cow::Borrowed("order_id"), Value::from("ord_8812")),
        "a bare identifier is shorthand for `name = name`",
    );
}

#[test]
fn a_string_literal_key_carries_a_dotted_semantic_convention_name() {
    let fields = crate::__rust_sak_o11y_fields!("http.response.status_code" = 200);

    assert_eq!(fields[0].0, "http.response.status_code");
}

#[test]
fn no_fields_produces_an_empty_set() {
    let fields = crate::__rust_sak_o11y_fields!();

    assert!(fields.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// The level gate
// ---------------------------------------------------------------------------------------------------------------

/// Counts how many times [`side_effect`] has been evaluated.
static SIDE_EFFECTS: AtomicUsize = AtomicUsize::new(0);

/// A field value whose evaluation is observable.
fn side_effect() -> u64 {
    SIDE_EFFECTS.fetch_add(1, Ordering::SeqCst);
    1
}

#[test]
fn fields_are_not_evaluated_when_the_level_gate_is_closed() {
    let _guard = global_lock();

    gate::close();
    SIDE_EFFECTS.store(0, Ordering::SeqCst);

    log::debug!("never recorded", value = side_effect());
    log::info!("never recorded", value = side_effect());
    log::warn!("never recorded", value = side_effect());
    log::error!("never recorded", value = side_effect());

    assert_eq!(
        SIDE_EFFECTS.load(Ordering::SeqCst),
        0,
        "a gated-out record must not evaluate its arguments, or an expensive debug field would cost in release",
    );
}

#[test]
fn a_record_at_or_above_the_threshold_evaluates_its_fields() {
    let _guard = global_lock();

    gate::open(Level::Warn);
    SIDE_EFFECTS.store(0, Ordering::SeqCst);

    log::debug!("below", value = side_effect());
    log::info!("below", value = side_effect());
    log::warn!("at", value = side_effect());
    log::error!("above", value = side_effect());

    gate::close();

    assert_eq!(SIDE_EFFECTS.load(Ordering::SeqCst), 2);
}

#[test]
fn nothing_passes_the_gate_before_init() {
    let _guard = global_lock();
    gate::close();

    assert!(
        !log::enabled(Level::Error),
        "even an error is discarded until init lowers the threshold"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Spans
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_nested_span_inherits_the_trace_id_and_records_its_parent() {
    let outer = Span::__enter("outer", Vec::new());
    let outer_trace = trace::current().trace_id().unwrap();
    let outer_span = trace::current().span_id().unwrap();

    let inner = Span::__enter("inner", Vec::new());

    assert_eq!(
        trace::current().trace_id().unwrap(),
        outer_trace,
        "a trace spans its whole tree"
    );
    assert_ne!(trace::current().span_id().unwrap(), outer_span);

    drop(inner);
    assert_eq!(
        trace::current().span_id().unwrap(),
        outer_span,
        "closing the inner span restores the outer"
    );

    drop(outer);
    assert_eq!(trace::depth(), 0);
}

#[test]
fn two_root_spans_get_different_traces() {
    let first = Span::__enter("first", Vec::new());
    let first_trace = trace::current().trace_id().unwrap();
    drop(first);

    let second = Span::__enter("second", Vec::new());
    let second_trace = trace::current().trace_id().unwrap();
    drop(second);

    assert_ne!(first_trace, second_trace);
}

#[test]
fn a_span_closes_on_an_early_return() {
    fn fallible() -> Result<()> {
        let _span = Span::__enter("fallible", Vec::new());
        Err(O11yError::AlreadyInitialized)?;
        Ok(())
    }

    assert!(fallible().is_err());
    assert_eq!(trace::depth(), 0, "`?` drops the guard on its way out");
}

#[test]
fn a_span_closes_while_a_panic_unwinds() {
    let result = std::panic::catch_unwind(|| {
        let _span = Span::__enter("doomed", Vec::new());
        panic!("boom");
    });

    assert!(result.is_err());
    assert_eq!(trace::depth(), 0);
}

#[test]
fn guards_dropped_out_of_order_still_leave_the_stack_empty() {
    let outer = Span::__enter("outer", Vec::new());
    let inner = Span::__enter("inner", Vec::new());

    // A guard kept in a struct or a collection can outlive its nesting; removing by identifier rather than by
    // position is what keeps that from corrupting the stack.
    drop(outer);
    assert_eq!(trace::depth(), 1);

    drop(inner);
    assert_eq!(trace::depth(), 0);
}

#[test]
fn current_is_a_silent_no_op_when_no_span_is_open() {
    assert_eq!(trace::depth(), 0);
    assert!(!trace::current().is_recording());
    assert!(trace::current().trace_id().is_none());

    // The point of the test: none of these panic, so library code can annotate unconditionally.
    trace::current().set_attribute("order_id", "ord_8812");
    trace::current().add_event("nothing is listening");
    trace::current().set_error(&O11yError::AlreadyInitialized);
}

#[test]
fn a_disabled_span_records_nothing_and_never_touches_the_stack() {
    let span = Span::__disabled();

    assert!(!span.is_recording());
    assert_eq!(trace::depth(), 0);

    drop(span);
    assert_eq!(trace::depth(), 0);
}

// ---------------------------------------------------------------------------------------------------------------
// Async span propagation
// ---------------------------------------------------------------------------------------------------------------

/// A future that returns `Pending` once before resolving, so a test can observe the gap between two polls.
struct YieldOnce {
    /// Whether the single pending poll has already happened.
    yielded: bool,
    /// The span depth observed during each poll.
    depths: Arc<Mutex<Vec<usize>>>,
}

impl std::future::Future for YieldOnce {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        self.depths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(trace::depth());

        if self.yielded {
            std::task::Poll::Ready(())
        } else {
            self.yielded = true;
            std::task::Poll::Pending
        }
    }
}

#[test]
fn an_instrumented_future_holds_its_span_during_a_poll_and_not_between_polls() {
    use trace::Instrument;

    let depths = Arc::new(Mutex::new(Vec::new()));
    let future = YieldOnce {
        yielded: false,
        depths: Arc::clone(&depths),
    };

    let mut instrumented = Box::pin(future.instrument(Span::__enter("async work", Vec::new())));

    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);

    assert!(instrumented.as_mut().poll(&mut context).is_pending());
    assert_eq!(
        trace::depth(),
        0,
        "between polls the span is off the thread, so it cannot mis-parent another task's work",
    );

    assert!(instrumented.as_mut().poll(&mut context).is_ready());
    assert_eq!(trace::depth(), 0);

    let observed = depths.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(
        observed,
        vec![1, 1],
        "the span is on the stack for the duration of every poll"
    );
}

#[test]
fn an_instrumented_future_dropped_before_it_resolves_still_closes_its_span() {
    use trace::Instrument;

    let future = YieldOnce {
        yielded: false,
        depths: Arc::new(Mutex::new(Vec::new())),
    };

    drop(future.instrument(Span::__enter("abandoned", Vec::new())));

    assert_eq!(trace::depth(), 0);
}

// ---------------------------------------------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_counter_accumulates_across_calls() {
    let _guard = global_lock();
    metric::clear();

    let counter = metric::counter("accumulating");
    counter.increment(2);
    counter.increment(3);

    let snapshot = metric::snapshot()
        .into_iter()
        .find(|s| s.name == "accumulating")
        .unwrap();
    let metric::MetricData::Sum(points) = &snapshot.data else {
        panic!("expected a sum")
    };

    assert_eq!(points.iter().find(|point| point.tags.is_empty()).unwrap().value, 5);
}

#[test]
fn tag_order_does_not_split_one_series_into_two() {
    let _guard = global_lock();
    metric::clear();

    let counter = metric::counter("tagged");
    counter.add_with_tags(1, &[("region", "eu-west-1"), ("tier", "gold")]);
    counter.add_with_tags(1, &[("tier", "gold"), ("region", "eu-west-1")]);

    let snapshot = metric::snapshot().into_iter().find(|s| s.name == "tagged").unwrap();
    let metric::MetricData::Sum(points) = &snapshot.data else {
        panic!("expected a sum")
    };
    let tagged: Vec<_> = points.iter().filter(|point| !point.tags.is_empty()).collect();

    assert_eq!(
        tagged.len(),
        1,
        "the same tags in a different order are the same series"
    );
    assert_eq!(tagged[0].value, 2);
}

#[test]
fn a_counter_used_only_with_tags_reports_no_untagged_point() {
    let _guard = global_lock();
    metric::clear();

    let counter = metric::counter("tagged_only");
    counter.add_with_tags(4, &[("route", "/health")]);

    let snapshot = metric::snapshot()
        .into_iter()
        .find(|s| s.name == "tagged_only")
        .unwrap();
    let metric::MetricData::Sum(points) = &snapshot.data else {
        panic!("expected a sum")
    };

    assert!(
        points.iter().all(|point| !point.tags.is_empty()),
        "a counter that was never incremented untagged must not export an untagged zero"
    );
}

#[test]
fn an_untouched_gauge_reports_no_data_point() {
    let _guard = global_lock();
    metric::clear();

    let _gauge = metric::gauge("never_set");
    let snapshot = metric::snapshot().into_iter().find(|s| s.name == "never_set").unwrap();
    let metric::MetricData::Gauge(points) = &snapshot.data else {
        panic!("expected a gauge")
    };

    assert!(
        points.is_empty(),
        "a gauge that was never set must not be reported as zero"
    );
}

#[test]
fn a_gauge_reports_the_value_it_was_last_set_to() {
    let _guard = global_lock();
    metric::clear();

    let gauge = metric::gauge("queue_depth");
    gauge.set(12);
    gauge.set(7);

    let snapshot = metric::snapshot()
        .into_iter()
        .find(|s| s.name == "queue_depth")
        .unwrap();
    let metric::MetricData::Gauge(points) = &snapshot.data else {
        panic!("expected a gauge")
    };

    assert_eq!(points[0].value, 7.0);
}

#[test]
fn a_histogram_files_values_into_the_bucket_they_fall_in() {
    let _guard = global_lock();
    metric::clear();

    let histogram = metric::histogram("values").with_buckets(&[10.0, 100.0]);
    histogram.record(5.0);
    histogram.record(50.0);
    histogram.record(500.0);

    let snapshot = metric::snapshot().into_iter().find(|s| s.name == "values").unwrap();
    let metric::MetricData::Histogram { points, .. } = &snapshot.data else {
        panic!("expected a histogram")
    };

    assert_eq!(points[0].bucket_counts, vec![1, 1, 1]);
    assert_eq!(points[0].count, 3);
    assert_eq!(points[0].sum, 555.0);
}

#[test]
fn with_buckets_given_out_of_order_bounds_sorts_them() {
    let _guard = global_lock();
    metric::clear();

    let histogram = metric::histogram("unsorted").with_buckets(&[100.0, 10.0]);
    histogram.record(50.0);

    let snapshot = metric::snapshot().into_iter().find(|s| s.name == "unsorted").unwrap();
    let metric::MetricData::Histogram { bounds, points } = &snapshot.data else {
        panic!("expected a histogram")
    };

    assert_eq!(**bounds, [10.0, 100.0]);
    assert_eq!(points[0].bucket_counts, vec![0, 1, 0]);
}

#[test]
fn a_dropped_instrument_leaves_the_registry() {
    let _guard = global_lock();
    metric::clear();

    drop(metric::counter("transient"));

    assert!(
        !metric::snapshot().iter().any(|s| s.name == "transient"),
        "the registry holds weak references, so it never keeps a dropped instrument alive",
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Enrichment
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_machine_id_is_scoped_to_the_service() {
    let first = Enrichment::new("service-one").snapshot();
    let second = Enrichment::new("service-two").snapshot();

    match (first.get("machine.id"), second.get("machine.id")) {
        (Some(one), Some(two)) => assert_ne!(one, two, "two services on one machine must not share an id"),
        // A host with no readable machine id simply omits the attribute, which is a supported outcome.
        _ => assert!(!first.contains_key("machine.id") && !second.contains_key("machine.id")),
    }
}

#[test]
fn every_record_carries_the_machine_and_session_attributes() {
    let snapshot = Enrichment::new("my-app").snapshot();

    assert_eq!(
        snapshot.get("machine.os").map(String::as_str),
        Some(std::env::consts::OS)
    );
    assert_eq!(
        snapshot.get("machine.arch").map(String::as_str),
        Some(std::env::consts::ARCH)
    );
    assert!(snapshot.contains_key("session.id"));
}

#[test]
fn renewing_the_session_replaces_the_id_for_later_records_only() {
    let enrichment = Enrichment::new("my-app");

    let before = enrichment.attributes();
    let before_id = enrichment.session_id();

    enrichment.renew_session();

    assert_ne!(enrichment.session_id(), before_id);
    assert!(
        before
            .iter()
            .any(|(key, value)| key == "session.id" && *value == Value::String(before_id.clone())),
        "a record already emitted keeps the session it was emitted under",
    );
}

/// A lookup result carrying only the country, which is all the enrichment reads here.
fn located(country: &str) -> Geolocation {
    Geolocation {
        country: Some(country.to_string()),
        ..Geolocation::default()
    }
}

#[test]
fn a_second_location_replaces_the_first_rather_than_being_appended() {
    let enrichment = Enrichment::new("my-app");

    enrichment.set_location(&located("NL"));
    enrichment.set_location(&located("BR"));

    let countries = enrichment
        .attributes()
        .iter()
        .filter(|(key, _)| key == "location.country")
        .count();

    assert_eq!(countries, 1);
    assert_eq!(
        enrichment.snapshot().get("location.country").map(String::as_str),
        Some("BR"),
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn an_export_posts_each_signal_to_its_own_otlp_path() {
    let _guard = global_lock();
    metric::clear();

    let (endpoint, captured) = spawn_recording_collector(3, "200 OK");
    let sink = Arc::new(ErrorSink::default());
    let shared = pipeline::test_shared(test_config(&endpoint, &sink));

    shared.logs.push(log_record("order received"));
    shared.spans.push(span_record("charge_card"));
    let exported = metric::counter("exported_total");
    exported.increment(1);

    assert!(worker::export_once(&shared));
    assert!(
        captured_at_least(&captured, 3),
        "logs, traces and metrics are three separate requests"
    );

    let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let paths: Vec<&str> = requests
        .iter()
        .map(super::test_support::CapturedRequest::path)
        .collect();

    assert!(paths.contains(&"/v1/logs"));
    assert!(paths.contains(&"/v1/traces"));
    assert!(paths.contains(&"/v1/metrics"));
    assert!(sink.taken().is_empty());
}

#[test]
fn a_rejected_payload_reaches_the_error_callback() {
    let _guard = global_lock();
    metric::clear();

    let (endpoint, _captured) = spawn_recording_collector(1, "503 Service Unavailable");
    let sink = Arc::new(ErrorSink::default());
    let shared = pipeline::test_shared(test_config(&endpoint, &sink));

    shared.logs.push(log_record("order received"));

    assert!(!worker::export_once(&shared));

    let reported = sink.taken();
    assert_eq!(reported.len(), 1);
    assert!(reported[0].contains("503"), "got {reported:?}");
    assert!(
        reported[0].contains("logs"),
        "the callback says which stream failed: {reported:?}"
    );
}

#[test]
fn an_unreachable_collector_reports_rather_than_blocking_or_panicking() {
    let _guard = global_lock();
    metric::clear();

    let sink = Arc::new(ErrorSink::default());
    // Port 1 on loopback refuses immediately, which is the connect-error path rather than the timeout one.
    let shared = pipeline::test_shared(test_config("http://127.0.0.1:1", &sink));

    shared.logs.push(log_record("never delivered"));

    assert!(!worker::export_once(&shared));
    assert_eq!(sink.taken().len(), 1);
}

#[test]
fn discarded_records_are_reported_through_the_callback() {
    let _guard = global_lock();
    metric::clear();

    let sink = Arc::new(ErrorSink::default());
    let shared = pipeline::test_shared(test_config("http://127.0.0.1:1", &sink));

    // A buffer of its own, sized so the overflow is deterministic rather than depending on the default.
    let buffer = Buffer::new(2);
    for _ in 0..5 {
        buffer.push(log_record("overflowing"));
    }
    let (_kept, dropped) = buffer.take();
    assert_eq!(dropped, 3);

    shared.logs.push(log_record("delivered"));
    worker::export_once(&shared);

    assert!(sink.taken().iter().any(|error| error.contains("logs")));
}

// ---------------------------------------------------------------------------------------------------------------
// The process globals
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn disabled_telemetry_makes_no_network_calls_at_all() {
    let _guard = global_lock();

    let (collector, collector_hits) = spawn_counting_server();
    let (geolocation, geolocation_hits) = spawn_counting_server();

    let result = init(
        Config::builder(&collector, NO_HEADERS)
            .service_name("silent")
            .enabled(false)
            .geolocation(true)
            .geolocation_url(&geolocation)
            .build(),
    );

    // `init` may already have been claimed by the end-to-end test when the whole suite shares one process; either
    // way the assertion below is the point, and it holds in both cases.
    assert!(result.is_ok() || matches!(result, Err(O11yError::AlreadyInitialized)));

    log::info!("discarded", order_id = "ord_8812");
    flush();

    assert!(!wait_until(Duration::from_millis(200), || {
        collector_hits.load(Ordering::SeqCst) > 0 || geolocation_hits.load(Ordering::SeqCst) > 0
    }));
}

#[test]
fn init_rejects_an_endpoint_that_is_not_a_url() {
    let error = init(Config::builder("not a url", NO_HEADERS).build()).unwrap_err();

    assert!(matches!(error, O11yError::InvalidEndpoint { .. }));
}

#[test]
fn init_rejects_a_header_that_cannot_be_sent() {
    let error = init(Config::builder("http://127.0.0.1:1", [("bad header name", "value")]).build()).unwrap_err();

    assert!(matches!(error, O11yError::InvalidHeader { .. }));
}

#[test]
fn shutdown_without_init_does_nothing_and_is_safe_to_repeat() {
    let _guard = global_lock();

    shutdown();
    shutdown();
    flush();
}

#[test]
fn the_exported_log_payload_has_the_shape_a_collector_expects() {
    let _guard = global_lock();
    metric::clear();

    let (endpoint, captured) = spawn_recording_collector(1, "200 OK");
    let sink = Arc::new(ErrorSink::default());
    let shared = pipeline::test_shared(test_config(&endpoint, &sink));

    let mut record = log_record("order received");
    record.enrichment = shared.enrichment.attributes();
    shared.logs.push(record);

    assert!(worker::export_once(&shared));
    assert!(captured_at_least(&captured, 1));

    let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let payload = requests[0].body_json();
    let block = &payload["resourceLogs"][0];
    let record = &block["scopeLogs"][0]["logRecords"][0];

    assert_eq!(block["scopeLogs"][0]["scope"]["name"], "rust-sak/o11y");
    assert_eq!(record["severityNumber"], 9, "9 is INFO");
    assert_eq!(record["severityText"], "INFO");
    assert_eq!(record["body"]["stringValue"], "order received");
    assert_eq!(record["attributes"][0]["key"], "order_id");

    let resource: Vec<&str> = block["resource"]["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap())
        .collect();

    assert!(resource.contains(&"service.name"));
    assert!(resource.contains(&"service.version"));
    assert!(resource.contains(&"deployment.environment.name"));
    assert!(
        resource.contains(&"session.id"),
        "the session travels with the batch, not with each record"
    );
}

#[test]
fn the_export_request_carries_the_configured_headers_and_content_type() {
    let _guard = global_lock();
    metric::clear();

    let (endpoint, captured) = spawn_recording_collector(1, "200 OK");
    let sink = Arc::new(ErrorSink::default());
    let shared = pipeline::test_shared(
        Config::builder(&endpoint, [("Authorization", "Bearer secret")])
            .service_name("test-service")
            .timeout(Duration::from_secs(5))
            .on_export_error(sink.handler())
            .build(),
    );

    shared.logs.push(log_record("order received"));
    assert!(worker::export_once(&shared));
    assert!(captured_at_least(&captured, 1));

    let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let head = requests[0].head.to_lowercase();

    assert!(head.contains("authorization: bearer secret"), "got {head}");
    assert!(head.contains("content-type: application/json"), "got {head}");
}

#[test]
fn a_geolocation_lookup_parses_what_the_service_returns() {
    let url = spawn_json_server(r#"{"ip":"1.2.3.4","city":"Amsterdam","region":"North Holland","country":"NL"}"#);
    let geo = fetch_geolocation_from(&url).unwrap();

    assert_eq!(geo.country.as_deref(), Some("NL"));
    assert_eq!(geo.city.as_deref(), Some("Amsterdam"));
}

#[test]
fn a_resolved_location_reaches_the_enrichment() {
    let enrichment = Enrichment::new("my-app");
    enrichment.set_location(&located("NL"));

    assert_eq!(
        enrichment.snapshot().get("location.country").map(String::as_str),
        Some("NL")
    );
}

#[test]
fn byte_values_are_base64_encoded_with_the_right_padding() {
    // Hand-rolled encoders get the tail wrong; every remainder class is exercised here.
    let cases = [
        (vec![], ""),
        (vec![b'f'], "Zg=="),
        (vec![b'f', b'o'], "Zm8="),
        (vec![b'f', b'o', b'o'], "Zm9v"),
        (vec![b'f', b'o', b'o', b'b'], "Zm9vYg=="),
        (vec![b'f', b'o', b'o', b'b', b'a'], "Zm9vYmE="),
        (vec![b'f', b'o', b'o', b'b', b'a', b'r'], "Zm9vYmFy"),
    ];

    for (bytes, expected) in cases {
        let mut record = log_record("bytes");
        record.fields = vec![(Cow::Borrowed("blob"), Value::Bytes(bytes.clone()))];

        let payload = otlp::logs(vec![record], &[]);
        let encoded = &payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"][0]["value"];

        assert_eq!(encoded["bytesValue"], expected, "encoding {bytes:?}");
    }
}

#[test]
fn the_series_cap_folds_the_overflow_into_a_single_marked_series() {
    let _guard = global_lock();
    metric::clear();

    let counter = metric::counter("high_cardinality");

    // Well past the per-instrument cap, as an unbounded tag domain — a request id, say — would be.
    for index in 0..(super::metric::MAX_SERIES + 50) {
        counter.add_with_tags(1, &[("request_id", &index.to_string())]);
    }

    let snapshot = metric::snapshot()
        .into_iter()
        .find(|s| s.name == "high_cardinality")
        .unwrap();
    let metric::MetricData::Sum(points) = &snapshot.data else {
        panic!("expected a sum")
    };

    let overflow = points
        .iter()
        .find(|point| point.tags.iter().any(|(key, _)| &**key == "o11y.series_overflow"))
        .expect("samples past the cap fold into an overflow series");

    assert_eq!(
        overflow.value, 50,
        "the total is preserved even though the breakdown stops"
    );
    assert!(
        points.len() <= super::metric::MAX_SERIES + 2,
        "memory stays bounded: {} series",
        points.len(),
    );
}

// The explicit `&` on each call is load-bearing, not redundant: it is the borrow `#[instrument]` emits, and it is
// what puts method resolution at the step where `ValueViaInto` is found before `ValueViaDebug`. Clippy sees only
// that the compiler would autoref anyway.
#[allow(clippy::needless_borrow)]
#[test]
fn argument_capture_prefers_a_value_conversion_over_debug() {
    use crate::o11y::{ValueViaDebug, ValueViaInto};

    /// A type with no `Into<Value>`, so it can only reach the `Debug` fallback.
    #[derive(Debug)]
    struct Opaque {
        // Read only by the derived `Debug`, which is the whole point of the type.
        #[allow(dead_code)]
        id: u8,
    }

    // Integers, floats, bools and strings take the cheap path and land in their proper variant rather than being
    // formatted into a string.
    assert_eq!((&7u64).o11y_value(), Value::Int(7));
    assert_eq!((&(-3i32)).o11y_value(), Value::Int(-3));
    assert_eq!((&1.5f64).o11y_value(), Value::Double(1.5));
    assert_eq!((&true).o11y_value(), Value::Bool(true));
    assert_eq!((&"hello").o11y_value(), Value::String("hello".to_string()));

    // Anything else still works, through `Debug`.
    assert_eq!(
        (&Opaque { id: 9 }).o11y_value(),
        Value::String("Opaque { id: 9 }".to_string())
    );

    // A reference argument — the shape `#[instrument] fn f(req: &Request)` produces — also reaches the fallback.
    let opaque = Opaque { id: 4 };
    assert_eq!((&&opaque).o11y_value(), Value::String("Opaque { id: 4 }".to_string()));
}

#[test]
fn instrument_captures_each_argument_in_its_natural_value_variant() {
    let _guard = global_lock();

    /// Returns what the span opened around its own body captured, so the assertions can read it directly.
    #[trace::instrument]
    fn charge(order_id: &str, attempt: u32, amount: f64) -> Option<(Cow<'static, str>, record::Fields)> {
        trace::current_span()
    }

    gate::open(Level::Debug);
    let captured = charge("ord_8812", 2, 12.5);
    gate::close();

    let (name, fields) = captured.expect("the span is open for the whole body");

    assert_eq!(name, "charge", "the span is named after the function");
    assert_eq!(
        fields,
        vec![
            (Cow::Borrowed("order_id"), Value::String("ord_8812".to_string())),
            // The point of the test: an integer and a float keep their own variants rather than arriving as
            // `Value::String("2")` and `Value::String("12.5")`, which is what formatting every argument through
            // `Debug` would produce.
            (Cow::Borrowed("attempt"), Value::Int(2)),
            (Cow::Borrowed("amount"), Value::Double(12.5)),
        ]
    );
}

#[test]
fn instrument_honours_skip_and_leaves_the_rest_captured() {
    let _guard = global_lock();

    #[trace::instrument(name = "sign_in", skip(password))]
    fn sign_in(user: &str, password: &str) -> Option<(Cow<'static, str>, record::Fields)> {
        let _ = password;
        trace::current_span()
    }

    #[trace::instrument(skip_all)]
    fn silent(user: &str) -> Option<(Cow<'static, str>, record::Fields)> {
        let _ = user;
        trace::current_span()
    }

    gate::open(Level::Debug);
    let signed_in = sign_in("ada", "hunter2");
    let quiet = silent("ada");
    gate::close();

    let (name, fields) = signed_in.expect("the span is open for the whole body");
    assert_eq!(name, "sign_in", "`name = \"..\"` overrides the function's own name");
    assert_eq!(fields, vec![(Cow::Borrowed("user"), Value::String("ada".to_string()))]);

    let (_, fields) = quiet.expect("the span is open for the whole body");
    assert!(fields.is_empty(), "`skip_all` captures nothing");
}
