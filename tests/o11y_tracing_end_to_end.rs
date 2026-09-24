//! The `tracing` bridge driven the way an application drives it: `init`, a global subscriber carrying the layer,
//! records through the `tracing` macros, `shutdown` — and the OTLP payloads that reach a collector.
//!
//! The in-crate tests prove each mapping against the records before encoding. This proves the same records survive
//! the real worker thread and the encoder: parent ids, trace ids on logs, links, status, and fields the consumer
//! mapped out. It is its own binary because `init` succeeds once per process, and so does installing a global
//! subscriber.

#![cfg(feature = "o11y-tracing")]

mod common;

use std::sync::PoisonError;
use std::time::Duration;

use common::{UNBOUNDED, spawn_recording_collector, wait_until};
use serde_json::Value as Json;
use tracing_subscriber::layer::SubscriberExt;

use rust_sak::o11y::{self, Config, Level, NO_HEADERS};

/// Every entry under `key` in every payload posted to `path`, flattened across requests and resource blocks.
fn collect(requests: &[common::CapturedRequest], path: &str, scope: &str, key: &str) -> Vec<Json> {
    requests
        .iter()
        .filter(|request| request.path() == path)
        .flat_map(|request| {
            let payload = request.body_json();
            let blocks = payload
                .as_object()
                .and_then(|object| object.values().next())
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();

            blocks
                .into_iter()
                .flat_map(|block| block[scope].as_array().cloned().unwrap_or_default())
                .flat_map(|scoped| scoped[key].as_array().cloned().unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The attribute `key` of an OTLP record or span, if it has one.
fn attribute<'a>(entry: &'a Json, key: &str) -> Option<&'a Json> {
    entry["attributes"]
        .as_array()?
        .iter()
        .find(|attribute| attribute["key"] == key)
        .map(|attribute| &attribute["value"])
}

/// The one span named `name`.
fn span<'a>(spans: &'a [Json], name: &str) -> &'a Json {
    let matching: Vec<&Json> = spans.iter().filter(|span| span["name"] == name).collect();
    assert_eq!(matching.len(), 1, "expected exactly one span {name:?}");

    matching[0]
}

/// The one log record whose body is `body`.
fn log<'a>(logs: &'a [Json], body: &str) -> &'a Json {
    let matching: Vec<&Json> = logs.iter().filter(|log| log["body"]["stringValue"] == body).collect();
    assert_eq!(matching.len(), 1, "expected exactly one log record {body:?}");

    matching[0]
}

#[test]
fn tracing_records_reach_the_collector_with_their_trace_structure() {
    let (endpoint, captured) = spawn_recording_collector(UNBOUNDED, "200 OK");

    o11y::init(
        Config::builder(&endpoint, NO_HEADERS)
            .service_name("bridge-test")
            .min_level(Level::Debug)
            // Long enough that nothing exports on the interval: `shutdown` drains everything at once.
            .flush_interval(Duration::from_secs(30))
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .unwrap();

    let layer = o11y::tracing::layer()
        .map_field(|name, value| (name != "path").then_some(value))
        .fold_spans(|metadata| metadata.target() == "ort");
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer)).unwrap();

    let cause = tracing::info_span!("model.build");
    drop(cause.enter());

    {
        let _enhance = tracing::info_span!("enhance", model = "osaka", path = "/Users/ada/photo.jpg").entered();

        tracing::error!("level error");
        tracing::warn!("level warn");
        tracing::info!(path = "/Users/ada/photo.jpg", "level info");
        tracing::debug!("level debug");
        tracing::trace!("level trace");

        let session = tracing::info_span!("session.acquire");
        session.follows_from(&cause);
        session.in_scope(|| {
            let record = tracing::trace_span!(target: "ort", "ort", location = "session_state.cc:1136");
            tracing::warn!(target: "ort", parent: &record, "runtime diagnostic");
        });

        // The span goes to another thread, which does the work inside it and closes it.
        let tile = tracing::info_span!("tile");
        std::thread::spawn(move || {
            tile.in_scope(|| tracing::info!("on the worker"));
        })
        .join()
        .unwrap();

        tracing::info_span!("decode").in_scope(|| tracing::error!("decoder refused the file"));
    }

    drop(cause);
    o11y::shutdown();

    assert!(
        wait_until(Duration::from_secs(10), || {
            let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
            requests.iter().any(|request| request.path() == "/v1/logs")
                && requests.iter().any(|request| request.path() == "/v1/traces")
        }),
        "shutdown must drain both signals before it returns",
    );

    let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let spans = collect(&requests, "/v1/traces", "scopeSpans", "spans");
    let logs = collect(&requests, "/v1/logs", "scopeLogs", "logRecords");

    // --- spans and their parents ---
    let enhance = span(&spans, "enhance");
    let session = span(&spans, "session.acquire");
    let tile = span(&spans, "tile");
    let decode = span(&spans, "decode");
    let cause = span(&spans, "model.build");

    assert!(enhance.get("parentSpanId").is_none(), "the outermost span is a root");
    for child in [session, tile, decode] {
        assert_eq!(
            child["traceId"], enhance["traceId"],
            "{} shares the trace",
            child["name"]
        );
        assert_eq!(
            child["parentSpanId"], enhance["spanId"],
            "{} is a child of enhance",
            child["name"]
        );
    }

    assert!(
        spans.iter().all(|span| span["name"] != "ort"),
        "a folded span is never exported"
    );
    assert_eq!(attribute(enhance, "model").unwrap()["stringValue"], "osaka");
    assert!(
        attribute(enhance, "path").is_none(),
        "a mapped-out field never leaves the process"
    );

    // --- the link and the status ---
    assert_eq!(
        session["links"],
        serde_json::json!([{ "traceId": cause["traceId"], "spanId": cause["spanId"] }])
    );
    assert_eq!(decode["status"]["code"], 2, "2 is ERROR");
    assert_eq!(decode["status"]["message"], "decoder refused the file");
    assert!(tile.get("status").is_none(), "a span that did not fail says nothing");

    // --- logs, their levels and their trace ids ---
    for (body, severity) in [
        ("level error", 17),
        ("level warn", 13),
        ("level info", 9),
        ("level debug", 5),
        ("level trace", 5),
    ] {
        let record = log(&logs, body);
        assert_eq!(record["severityNumber"], severity, "{body}");
        assert_eq!(record["traceId"], enhance["traceId"], "{body} carries its trace");
        assert_eq!(record["spanId"], enhance["spanId"], "{body} carries its span");
        assert_eq!(attribute(record, "target").unwrap()["stringValue"], module_path!());
    }

    assert!(attribute(log(&logs, "level info"), "path").is_none());

    let diagnostic = log(&logs, "runtime diagnostic");
    assert_eq!(
        diagnostic["spanId"], session["spanId"],
        "an event under a folded span joins the span around it"
    );
    assert_eq!(
        attribute(diagnostic, "location").unwrap()["stringValue"],
        "session_state.cc:1136"
    );
    assert_eq!(attribute(diagnostic, "target").unwrap()["stringValue"], "ort");

    assert_eq!(
        log(&logs, "on the worker")["spanId"],
        tile["spanId"],
        "the span followed its work across threads"
    );
    assert_eq!(log(&logs, "decoder refused the file")["spanId"], decode["spanId"]);
}
