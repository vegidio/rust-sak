//! Turning records into OTLP/HTTP JSON payloads.
//!
//! Three encoding rules govern everything here, and getting any of them wrong makes a collector reject the whole
//! payload with no useful diagnostic:
//!
//! 1. OTLP/JSON follows the **proto3 JSON mapping**, so 64-bit integers are serialised as **decimal strings** —
//!    `"timeUnixNano": "1758412800000000000"`, `"asInt": "5"`, `"count": "7"`. Bare numbers are accepted by some
//!    collectors, but the specification says strings and the reference encoder emits strings.
//! 2. `traceId`, `spanId` and `parentSpanId` are **lowercase hex**, not the base64 that every other `bytes` field
//!    uses.
//! 3. A histogram's `bucketCounts` is always exactly one longer than its `explicitBounds` — the extra entry is the
//!    bucket above the final bound.

use std::borrow::Cow;

use serde_json::{Value as Json, json};

use super::Value;
use super::enrichment::Attributes;
use super::metric::{MetricData, MetricSnapshot};
use super::record::{LogRecord, SpanRecord};

/// The instrumentation scope reported on every payload.
const SCOPE_NAME: &str = "rust-sak/o11y";

/// `SPAN_KIND_INTERNAL` — the only kind this module produces.
const SPAN_KIND_INTERNAL: u8 = 1;

/// `AGGREGATION_TEMPORALITY_CUMULATIVE`.
///
/// Values are reported as running totals rather than as the change since the last export. Under this module's
/// fail-open policy a failed export is dropped rather than retried, and a dropped cumulative point costs nothing —
/// the next successful one carries the full total. A delta would have lost those counts permanently.
const CUMULATIVE: u8 = 2;

/// Encodes a 64-bit integer the way the proto3 JSON mapping requires.
fn big_int(value: u64) -> Json {
    Json::String(value.to_string())
}

/// Encodes one [`Value`] as an OTLP `AnyValue`.
fn any_value(value: &Value) -> Json {
    match value {
        Value::Bool(inner) => json!({ "boolValue": inner }),
        Value::Int(inner) => json!({ "intValue": inner.to_string() }),
        Value::Double(inner) => json!({ "doubleValue": inner }),
        Value::String(inner) => json!({ "stringValue": inner }),
        // The one `bytes` field that really is base64, unlike the trace and span identifiers.
        Value::Bytes(inner) => json!({ "bytesValue": base64(inner) }),
        Value::List(inner) => json!({ "arrayValue": { "values": inner.iter().map(any_value).collect::<Vec<_>>() } }),
        Value::Map(inner) => json!({
            "kvlistValue": {
                "values": inner
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": any_value(value) }))
                    .collect::<Vec<_>>()
            }
        }),
    }
}

/// Standard base64, which is what the proto3 JSON mapping uses for `bytes`.
///
/// Hand-rolled rather than pulling in a dependency: byte-valued attributes are rare, and this is sixteen lines.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let block = chunk.iter().enumerate().fold(0u32, |block, (index, byte)| {
            block | (u32::from(*byte) << (16 - 8 * index))
        });

        for index in 0..=chunk.len() {
            encoded.push(ALPHABET[(block >> (18 - 6 * index)) as usize & 0x3f] as char);
        }

        for _ in chunk.len()..3 {
            encoded.push('=');
        }
    }

    encoded
}

/// Encodes a set of fields as an OTLP `KeyValue` list.
fn key_values(fields: &[(Cow<'static, str>, Value)]) -> Json {
    Json::Array(
        fields
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": any_value(value) }))
            .collect(),
    )
}

/// The resource block: everything that describes the process rather than one record.
///
/// `service.*` and `deployment.environment.name` come from the configuration; the machine, session and location
/// attributes come from the enrichment. All of them are identical across every record in a batch, so carrying them
/// once per batch rather than once per record is both correct OTLP and a large saving on the wire.
fn resource(base: &[(Cow<'static, str>, Value)], enrichment: &Attributes) -> Json {
    let mut attributes = base.to_vec();
    attributes.extend(enrichment.iter().cloned());

    json!({ "attributes": key_values(&attributes) })
}

/// The instrumentation scope block.
fn scope() -> Json {
    json!({ "name": SCOPE_NAME, "version": env!("CARGO_PKG_VERSION") })
}

/// Splits records into runs sharing one enrichment snapshot.
///
/// Records normally all share one, so this is a single group. It stops being one only across a
/// [`renew_session`](super::renew_session), where the records emitted either side genuinely belong to different
/// sessions — and OTLP allows a payload to carry several resource blocks, so each group keeps the session it was
/// actually emitted under rather than inheriting whichever one the exporter happened to see.
fn group_by_enrichment<T>(records: Vec<T>, enrichment_of: impl Fn(&T) -> &Attributes) -> Vec<(Attributes, Vec<T>)> {
    let mut groups: Vec<(Attributes, Vec<T>)> = Vec::new();

    for record in records {
        match groups
            .iter_mut()
            .find(|(attributes, _)| std::sync::Arc::ptr_eq(attributes, enrichment_of(&record)))
        {
            Some((_, members)) => members.push(record),
            None => groups.push((enrichment_of(&record).clone(), vec![record])),
        }
    }

    groups
}

/// Builds the `/v1/logs` payload.
pub(super) fn logs(records: Vec<LogRecord>, base: &[(Cow<'static, str>, Value)]) -> Json {
    let resource_logs: Vec<Json> = group_by_enrichment(records, |record| &record.enrichment)
        .into_iter()
        .map(|(enrichment, records)| {
            let log_records: Vec<Json> = records
                .iter()
                .map(|record| {
                    let mut encoded = json!({
                        "timeUnixNano": big_int(record.time_unix_nano),
                        "observedTimeUnixNano": big_int(record.time_unix_nano),
                        "severityNumber": record.level.severity_number(),
                        "severityText": record.level.severity_text(),
                        "body": { "stringValue": record.body },
                        "attributes": key_values(&record.fields),
                    });

                    // Omitted entirely rather than sent empty when the record was emitted outside a span: an empty
                    // string here is a malformed identifier, not an absent one.
                    if let (Some(trace_id), Some(span_id)) = (record.trace_id, record.span_id) {
                        encoded["traceId"] = Json::String(trace_id.to_hex());
                        encoded["spanId"] = Json::String(span_id.to_hex());
                    }

                    encoded
                })
                .collect();

            json!({
                "resource": resource(base, &enrichment),
                "scopeLogs": [{ "scope": scope(), "logRecords": log_records }],
            })
        })
        .collect();

    json!({ "resourceLogs": resource_logs })
}

/// Builds the `/v1/traces` payload.
pub(super) fn traces(records: Vec<SpanRecord>, base: &[(Cow<'static, str>, Value)]) -> Json {
    let resource_spans: Vec<Json> = group_by_enrichment(records, |record| &record.enrichment)
        .into_iter()
        .map(|(enrichment, records)| {
            let spans: Vec<Json> = records
                .iter()
                .map(|record| {
                    let events: Vec<Json> = record
                        .events
                        .iter()
                        .map(|event| {
                            json!({
                                "timeUnixNano": big_int(event.time_unix_nano),
                                "name": event.name,
                                "attributes": key_values(&event.attributes),
                            })
                        })
                        .collect();

                    let mut encoded = json!({
                        "traceId": record.trace_id.to_hex(),
                        "spanId": record.span_id.to_hex(),
                        "name": record.name,
                        "kind": SPAN_KIND_INTERNAL,
                        "startTimeUnixNano": big_int(record.start_unix_nano),
                        "endTimeUnixNano": big_int(record.end_unix_nano),
                        "attributes": key_values(&record.attributes),
                        "events": events,
                    });

                    if let Some(parent) = record.parent_span_id {
                        encoded["parentSpanId"] = Json::String(parent.to_hex());
                    }

                    encoded
                })
                .collect();

            json!({
                "resource": resource(base, &enrichment),
                "scopeSpans": [{ "scope": scope(), "spans": spans }],
            })
        })
        .collect();

    json!({ "resourceSpans": resource_spans })
}

/// Builds the `/v1/metrics` payload.
///
/// `start_unix_nano` is when the process began recording, which is what tells a backend that a total restarting
/// from zero is a process restart rather than a counter going backwards.
pub(super) fn metrics(
    snapshots: Vec<MetricSnapshot>,
    base: &[(Cow<'static, str>, Value)],
    enrichment: &Attributes,
    start_unix_nano: u64,
    now_unix_nano: u64,
) -> Json {
    let metrics: Vec<Json> = snapshots
        .iter()
        .map(|snapshot| {
            let data = match &snapshot.data {
                MetricData::Sum(points) => json!({
                    "sum": {
                        "aggregationTemporality": CUMULATIVE,
                        "isMonotonic": true,
                        "dataPoints": points.iter().map(|point| json!({
                            "attributes": tag_values(&point.tags),
                            "startTimeUnixNano": big_int(start_unix_nano),
                            "timeUnixNano": big_int(now_unix_nano),
                            "asInt": point.value.to_string(),
                        })).collect::<Vec<_>>(),
                    }
                }),
                // A gauge is a last-value sample, so temporality does not apply to it.
                MetricData::Gauge(points) => json!({
                    "gauge": {
                        "dataPoints": points.iter().map(|point| json!({
                            "attributes": tag_values(&point.tags),
                            "timeUnixNano": big_int(now_unix_nano),
                            "asDouble": point.value,
                        })).collect::<Vec<_>>(),
                    }
                }),
                MetricData::Histogram(points) => json!({
                    "histogram": {
                        "aggregationTemporality": CUMULATIVE,
                        "dataPoints": points.iter().map(|point| {
                            debug_assert_eq!(
                                point.bucket_counts.len(),
                                point.bounds.len() + 1,
                                "a histogram needs one more bucket than it has bounds",
                            );

                            json!({
                                "attributes": tag_values(&point.tags),
                                "startTimeUnixNano": big_int(start_unix_nano),
                                "timeUnixNano": big_int(now_unix_nano),
                                "count": big_int(point.count),
                                "sum": point.sum,
                                "explicitBounds": point.bounds,
                                "bucketCounts": point.bucket_counts.iter().copied().map(big_int).collect::<Vec<_>>(),
                            })
                        }).collect::<Vec<_>>(),
                    }
                }),
            };

            let mut encoded = json!({ "name": snapshot.name });
            if let (Some(object), Some(extra)) = (encoded.as_object_mut(), data.as_object()) {
                object.extend(extra.clone());
            }

            encoded
        })
        .collect();

    json!({
        "resourceMetrics": [{
            "resource": resource(base, enrichment),
            "scopeMetrics": [{ "scope": scope(), "metrics": metrics }],
        }]
    })
}

/// Encodes a metric's tag set as an OTLP `KeyValue` list.
fn tag_values(tags: &[(String, String)]) -> Json {
    Json::Array(
        tags.iter()
            .map(|(key, value)| json!({ "key": key, "value": { "stringValue": value } }))
            .collect(),
    )
}
