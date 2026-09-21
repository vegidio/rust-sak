//! The `o11y` pipeline driven the way an application drives it: `init`, record, `shutdown`.
//!
//! Everything else in the suite is an in-crate `#[cfg(test)]` module that reaches past the process globals — it
//! builds the exporter's state directly and runs one export cycle synchronously. That covers the payloads but not
//! the lifecycle: the worker thread starting, the batch threshold waking it early, and `shutdown` draining what is
//! still buffered. Those only happen for real once, in one process, because `init` succeeds once — which is
//! exactly why this is its own integration binary.

#![cfg(feature = "o11y")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rust_sak::o11y::{self, Config, Environment, Level, log, metric, trace};

/// A captured export request: its path and its parsed JSON body.
#[derive(Debug)]
struct Captured {
    /// The OTLP path it was posted to.
    path: String,
    /// The payload.
    body: serde_json::Value,
}

/// Spawns a collector that answers `200 OK` and records every request until the test ends.
fn spawn_collector() -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));

    {
        let captured = Arc::clone(&captured);

        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                if let Some(request) = serve(stream) {
                    captured.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                }
            }
        });
    }

    (format!("http://{address}"), captured)
}

/// Reads one HTTP/1.1 request, answers it, and returns what it carried.
fn serve(mut stream: TcpStream) -> Option<Captured> {
    let mut raw = Vec::new();
    let mut chunk = [0; 4096];

    // Read until the headers are complete, then until the declared body has arrived.
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }

        raw.extend_from_slice(&chunk[..read]);

        let Some(header_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };

        let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
        let length: usize = head
            .lines()
            .find_map(|line| {
                line.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                    .map(str::to_string)
            })
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);

        if raw.len() >= header_end + 4 + length {
            let body = &raw[header_end + 4..header_end + 4 + length];
            let path = head.split_whitespace().nth(1).unwrap_or_default().to_string();

            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.flush();

            return Some(Captured {
                path,
                body: serde_json::from_slice(body).expect("the collector should receive valid json"),
            });
        }
    }

    None
}

/// Blocks until `condition` holds or `timeout` elapses.
fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if condition() {
            return true;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    condition()
}

/// How many times the export-error callback fired. It must not, in this test.
static EXPORT_ERRORS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn the_pipeline_records_and_exports_all_three_signals() {
    let (endpoint, captured) = spawn_collector();

    // A counter touched *before* `init`, which is what a `static LazyLock` does in a real application.
    let orders = metric::counter("orders_total");
    orders.increment(4);

    o11y::init(
        Config::builder(&endpoint, [("Authorization", "Bearer secret")])
            .service_name("checkout-service")
            .service_version("1.2.3")
            .environment(Environment::Production)
            .min_level(Level::Debug)
            // Long enough that nothing exports on the interval: this test drives it through `flush` and
            // `shutdown`, so a timed export would make the assertions racy.
            .flush_interval(Duration::from_secs(30))
            .timeout(Duration::from_secs(5))
            .on_export_error(|_| {
                EXPORT_ERRORS.fetch_add(1, Ordering::SeqCst);
            })
            .build(),
    )
    .unwrap();

    assert!(o11y::is_enabled());
    let session = o11y::session_id().expect("a running pipeline has a session");

    // A log emitted inside a span must come out carrying that span's ids.
    let (trace_id, span_id) = {
        let _span = trace::span!("charge_card", order_id = "ord_8812");
        let ids = (
            trace::current().trace_id().unwrap(),
            trace::current().span_id().unwrap(),
        );

        trace::current().set_attribute("processor", "stripe");
        trace::current().add_event("submitted to processor");
        log::info!("order received", order_id = "ord_8812", amount = 129.5);

        ids
    };

    log::warn!("outside any span", reason = "none open");
    orders.add_with_tags(1, &[("region", "eu-west-1")]);

    o11y::shutdown();

    assert!(
        wait_until(Duration::from_secs(10), || {
            captured.lock().unwrap_or_else(PoisonError::into_inner).len() >= 3
        }),
        "shutdown must drain every signal before it returns",
    );

    let requests = captured.lock().unwrap_or_else(PoisonError::into_inner);
    let find = |path: &str| requests.iter().find(|request| request.path == path).unwrap();

    // --- logs ---
    let logs = find("/v1/logs");
    let resource = &logs.body["resourceLogs"][0]["resource"]["attributes"];
    let attribute = |key: &str| {
        resource
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == key)
            .map(|entry| entry["value"]["stringValue"].as_str().unwrap().to_string())
    };

    assert_eq!(attribute("service.name").as_deref(), Some("checkout-service"));
    assert_eq!(attribute("service.version").as_deref(), Some("1.2.3"));
    assert_eq!(attribute("deployment.environment.name").as_deref(), Some("production"));
    assert_eq!(attribute("session.id").as_deref(), Some(session.as_str()));

    let records = logs.body["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
        .as_array()
        .unwrap();
    assert_eq!(records.len(), 2);

    let inside = records
        .iter()
        .find(|r| r["body"]["stringValue"] == "order received")
        .unwrap();
    assert_eq!(inside["traceId"], trace_id, "a log inside a span carries its trace");
    assert_eq!(inside["spanId"], span_id);
    assert_eq!(inside["severityNumber"], 9);

    let outside = records
        .iter()
        .find(|r| r["body"]["stringValue"] == "outside any span")
        .unwrap();
    assert!(outside.get("traceId").is_none());
    assert_eq!(outside["severityNumber"], 13, "13 is WARN");

    // --- traces ---
    let traces = find("/v1/traces");
    let span = &traces.body["resourceSpans"][0]["scopeSpans"][0]["spans"][0];

    assert_eq!(span["name"], "charge_card");
    assert_eq!(span["traceId"], trace_id);
    assert_eq!(span["spanId"], span_id);
    assert!(span.get("parentSpanId").is_none(), "a root span has no parent");
    assert_eq!(span["events"][0]["name"], "submitted to processor");

    let span_attributes = span["attributes"].as_array().unwrap();
    assert!(span_attributes.iter().any(|entry| entry["key"] == "order_id"));
    assert!(
        span_attributes.iter().any(|entry| entry["key"] == "processor"),
        "an attribute added through `current()` reaches the span it was added to",
    );

    let end: u64 = span["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
    let start: u64 = span["startTimeUnixNano"].as_str().unwrap().parse().unwrap();
    assert!(end >= start, "a span ends no earlier than it started");

    // --- metrics ---
    let metrics = find("/v1/metrics");
    let all = metrics.body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
        .as_array()
        .unwrap();
    let orders_metric = all.iter().find(|metric| metric["name"] == "orders_total").unwrap();
    let points = orders_metric["sum"]["dataPoints"].as_array().unwrap();

    assert_eq!(orders_metric["sum"]["aggregationTemporality"], 2);
    let untagged = points
        .iter()
        .find(|point| point["attributes"].as_array().unwrap().is_empty())
        .unwrap();
    assert_eq!(
        untagged["asInt"], "4",
        "a counter incremented before init is still exported once telemetry starts",
    );

    let tagged = points
        .iter()
        .find(|point| !point["attributes"].as_array().unwrap().is_empty())
        .unwrap();
    assert_eq!(tagged["attributes"][0]["key"], "region");
    assert_eq!(tagged["asInt"], "1");

    assert_eq!(
        EXPORT_ERRORS.load(Ordering::SeqCst),
        0,
        "nothing should have failed to export"
    );

    // Recording after shutdown is discarded rather than panicking.
    log::info!("after shutdown", ignored = true);
    o11y::shutdown();
}
