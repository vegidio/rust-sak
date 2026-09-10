use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::enrichment::{LOCATION_CITY, LOCATION_COUNTRY, MACHINE_ARCH, MACHINE_ID, MACHINE_OS, SESSION_ID, VERSION};
use super::test_support::{spawn_collector, spawn_counting_server, spawn_json_server, wait_until};
use super::*;

/// A sample ipinfo.io payload.
const GEO_JSON: &str = r#"{"ip":"1.2.3.4","city":"Amsterdam","region":"North Holland","country":"NL"}"#;

/// How long to wait on the background geolocation thread before giving up.
const GEO_TIMEOUT: Duration = Duration::from_secs(5);

// --- privacy tests ---
//
// These are the reason the module exists in this shape: opting out has to mean no traffic at all, and the machine
// identifier must not be correlatable across applications.

#[test]
fn disabled_makes_no_network_calls() {
    // Geolocation discloses the caller's public IP. Opting out of telemetry has to mean opting out of that lookup
    // too, not merely out of exporting its result — even when geolocation itself was explicitly requested.
    let (geo_url, geo_hits) = spawn_counting_server();
    let (collector_url, collector_hits) = spawn_counting_server();

    let telemetry = Telemetry::builder(&collector_url, "test-service")
        .version("1.0.0")
        .enabled(false)
        .geolocation(true)
        .geolocation_url(&geo_url)
        .build()
        .unwrap();

    telemetry.info("should.not.be.sent");
    telemetry.flush().unwrap();

    // Give anything that *would* have been sent a chance to arrive before asserting it did not.
    assert!(!wait_until(Duration::from_millis(300), || {
        geo_hits.load(Ordering::SeqCst) > 0 || collector_hits.load(Ordering::SeqCst) > 0
    }));

    assert_eq!(geo_hits.load(Ordering::SeqCst), 0, "no geolocation lookup may be made");
    assert_eq!(collector_hits.load(Ordering::SeqCst), 0, "no record may be exported");
    assert!(!telemetry.is_enabled());
}

#[test]
fn geolocation_is_off_by_default() {
    // Enabling telemetry must not, on its own, disclose the public IP address to a third party.
    let (geo_url, geo_hits) = spawn_counting_server();
    let (collector_url, _) = spawn_counting_server();

    let telemetry = Telemetry::builder(&collector_url, "test-service")
        .geolocation_url(&geo_url)
        .build()
        .unwrap();

    assert!(!wait_until(Duration::from_millis(300), || {
        geo_hits.load(Ordering::SeqCst) > 0
    }));
    assert_eq!(geo_hits.load(Ordering::SeqCst), 0, "geolocation must be opt-in");

    let fields = telemetry.enrichment_snapshot();
    assert!(!fields.contains_key(LOCATION_COUNTRY));
    assert!(!fields.contains_key(LOCATION_CITY));
}

#[test]
fn disabled_keeps_locally_read_enrichment() {
    let telemetry = Telemetry::builder("http://127.0.0.1:1", "test-service")
        .version("1.0.0")
        .enabled(false)
        .build()
        .unwrap();

    let fields = telemetry.enrichment_snapshot();

    // Everything below is read locally and never leaves the process, so it is gathered regardless.
    assert_eq!(fields.get(VERSION).unwrap(), "1.0.0");
    assert_eq!(fields.get(MACHINE_OS).unwrap(), std::env::consts::OS);
    assert_eq!(fields.get(MACHINE_ARCH).unwrap(), std::env::consts::ARCH);
    assert!(fields.contains_key(SESSION_ID));

    // The one enrichment that would have required a request is absent.
    assert!(!fields.contains_key(LOCATION_COUNTRY));
}

#[test]
fn machine_id_is_scoped_to_the_service() {
    let a = Telemetry::builder("http://127.0.0.1:1", "service-a")
        .enabled(false)
        .build()
        .unwrap();
    let b = Telemetry::builder("http://127.0.0.1:1", "service-b")
        .enabled(false)
        .build()
        .unwrap();

    let id_a = a.enrichment_snapshot().get(MACHINE_ID).cloned();
    let id_b = b.enrichment_snapshot().get(MACHINE_ID).cloned();

    // A machine without a readable host id reports no `machine.id` at all; there is nothing to compare then.
    let (Some(id_a), Some(id_b)) = (id_a, id_b) else {
        return;
    };

    assert!(!id_a.is_empty());
    // A raw host id would be identical across applications, letting a backend correlate this machine's telemetry
    // with that of every other program on it.
    assert_ne!(
        id_a, id_b,
        "machine.id must be derived per service, not the raw host id"
    );
}

#[test]
fn machine_id_is_stable_lowercase_hex() {
    let first = Telemetry::builder("http://127.0.0.1:1", "same-service")
        .enabled(false)
        .build()
        .unwrap();
    let second = Telemetry::builder("http://127.0.0.1:1", "same-service")
        .enabled(false)
        .build()
        .unwrap();

    let Some(id) = first.enrichment_snapshot().get(MACHINE_ID).cloned() else {
        return;
    };

    assert_eq!(id.len(), 64, "hmac-sha256 renders as 64 hex characters");
    assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(
        Some(&id),
        second.enrichment_snapshot().get(MACHINE_ID),
        "the same service must derive the same id on the same machine"
    );
}

// --- construction tests ---

#[test]
fn build_surfaces_an_invalid_endpoint() {
    // A space makes the URL unparseable, which must be reported rather than silently producing a dead exporter.
    let error = Telemetry::builder("ht tp://bad endpoint", "test-service")
        .build()
        .unwrap_err();

    assert!(
        matches!(error, O11yError::InvalidEndpoint { .. }),
        "expected InvalidEndpoint, got {error:?}"
    );
    assert!(error.to_string().contains("ht tp://bad endpoint"));
}

#[test]
fn build_surfaces_an_invalid_header() {
    let error = Telemetry::builder("http://127.0.0.1:1", "test-service")
        .header("bad header name", "value")
        .build()
        .unwrap_err();

    assert!(
        matches!(error, O11yError::InvalidHeader { .. }),
        "expected InvalidHeader, got {error:?}"
    );
}

#[test]
fn disabled_handle_is_usable_and_shuts_down_cleanly() {
    let telemetry = Telemetry::disabled();

    assert!(!telemetry.is_enabled());
    telemetry.info("still works");
    telemetry.event("still works").field("n", 1i64).warn();
    telemetry.error("still works", &std::io::Error::other("boom"));

    assert!(telemetry.flush().is_ok());
    assert!(telemetry.shutdown().is_ok());
    // Idempotent: the explicit call above plus the one from `Drop` must not fail or panic.
    assert!(telemetry.shutdown().is_ok());
}

#[test]
fn endpoint_gets_the_logs_signal_path() {
    // The exporter uses a programmatic endpoint verbatim, so the module has to append `/v1/logs` itself. A trailing
    // slash must not produce a doubled one.
    let (url, server) = spawn_collector();

    let telemetry = Telemetry::builder(format!("{url}/"), "test-service").build().unwrap();
    telemetry.info("app.started");
    let _ = telemetry.flush();

    let request = server.join().unwrap();
    assert!(
        request.head.starts_with("POST /v1/logs "),
        "request head was:\n{}",
        request.head
    );
}

// --- session tests ---

#[test]
fn session_id_is_a_uuid_and_changes_on_renewal() {
    let telemetry = Telemetry::disabled();

    let first = telemetry.session_id();
    assert_eq!(first.len(), 36, "uuid format: 8-4-4-4-12");

    telemetry.renew_session();
    assert_ne!(telemetry.session_id(), first);
}

#[test]
fn renew_session_is_concurrency_safe() {
    // Renewing used to be a candidate for tearing the attribute list while other threads read it; the swap must be
    // atomic from a reader's point of view.
    let telemetry = Arc::new(Telemetry::disabled());
    let mut handles = Vec::new();

    for _ in 0..4 {
        let renewer = Arc::clone(&telemetry);
        handles.push(std::thread::spawn(move || {
            for _ in 0..200 {
                renewer.renew_session();
            }
        }));

        let emitter = Arc::clone(&telemetry);
        handles.push(std::thread::spawn(move || {
            for _ in 0..200 {
                emitter.event("concurrent").field("n", 1i64).info();
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(telemetry.session_id().len(), 36);
    assert_eq!(
        telemetry.enrichment_snapshot().get(SESSION_ID),
        Some(&telemetry.session_id())
    );
}

// --- export tests ---
//
// These point the exporter at the throwaway local server in `super::test_support`, so they exercise the real export
// path without reaching the network.

#[test]
fn emits_an_enriched_record_to_the_collector() {
    let (url, server) = spawn_collector();

    let telemetry = Telemetry::builder(&url, "test-service")
        .version("4.5.6")
        .environment(Environment::Production)
        .header("Authorization", "Bearer secret")
        .build()
        .unwrap();

    telemetry
        .event("export.finished")
        .field("format", "avif")
        .field("bytes", 1_048_576u64)
        .info();
    let _ = telemetry.flush();

    let request = server.join().unwrap();

    assert!(
        request.head.to_lowercase().contains("authorization: bearer secret"),
        "request head was:\n{}",
        request.head
    );
    assert!(request.body_contains("export.finished"), "event name is missing");
    assert!(request.body_contains("format"), "record field is missing");
    assert!(request.body_contains("avif"), "record field value is missing");
    assert!(request.body_contains(SESSION_ID), "enrichment is missing");
    assert!(request.body_contains("4.5.6"), "version enrichment is missing");
    assert!(
        request.body_contains("test-service"),
        "service.name resource is missing"
    );
    assert!(request.body_contains("production"), "deployment environment is missing");
    // Severity text is the SDK's own canonical spelling (uppercase), not Go's lowercased variant.
    assert!(request.body_contains("INFO"), "severity text is missing");
}

#[test]
fn record_fields_override_enrichment() {
    let (url, server) = spawn_collector();

    let telemetry = Telemetry::builder(&url, "test-service")
        .version("1.0.0")
        .build()
        .unwrap();

    telemetry.event("versioned").field(VERSION, "overridden").info();
    let _ = telemetry.flush();

    let request = server.join().unwrap();
    assert!(request.body_contains("overridden"), "the record's own value must win");
    assert!(
        !request.body_contains("1.0.0"),
        "the enrichment value must be dropped, not emitted alongside"
    );

    // The enrichment itself is untouched by that one record.
    assert_eq!(telemetry.enrichment_snapshot().get(VERSION).unwrap(), "1.0.0");
}

#[test]
fn error_attaches_the_error_and_its_source() {
    let (url, server) = spawn_collector();

    let telemetry = Telemetry::builder(&url, "test-service").build().unwrap();

    let source = std::io::Error::other("disk is on fire");
    let error = O11yError::Io(source);
    telemetry.event("export.failed").error(&error);
    let _ = telemetry.flush();

    let request = server.join().unwrap();
    assert!(request.body_contains("error"), "the error attribute is missing");
    assert!(request.body_contains("disk is on fire"), "the source cause is missing");
    assert!(request.body_contains("error.source"), "the source attribute is missing");
}

// --- geolocation tests ---

#[test]
fn geolocation_is_merged_when_opted_in() {
    let geo_url = spawn_json_server(GEO_JSON);

    let telemetry = Telemetry::builder("http://127.0.0.1:1", "test-service")
        .geolocation(true)
        .geolocation_url(&geo_url)
        .build()
        .unwrap();

    // The lookup runs on a background thread, so `build` returns before it lands.
    let resolved = wait_until(GEO_TIMEOUT, || {
        telemetry.enrichment_snapshot().contains_key(LOCATION_CITY)
    });
    assert!(resolved, "the location should have been merged in");

    let fields = telemetry.enrichment_snapshot();
    assert_eq!(fields.get(LOCATION_CITY).unwrap(), "Amsterdam");
    assert_eq!(fields.get(LOCATION_COUNTRY).unwrap(), "NL");
}

#[test]
fn a_second_location_replaces_the_first() {
    // Only one lookup is started today, so this guards the invariant rather than a live bug: appending would put two
    // `location.city` attributes on every record, and the attribute list has no notion of a duplicate key.
    let enrichment = Enrichment::new("1.0.0", "test-service");

    let first = Geolocation {
        city: Some("Amsterdam".to_owned()),
        country: Some("NL".to_owned()),
        ..Geolocation::default()
    };
    let second = Geolocation {
        city: Some("Lisbon".to_owned()),
        country: Some("PT".to_owned()),
        ..Geolocation::default()
    };

    enrichment.set_location(&first);
    enrichment.set_location(&second);

    let attributes = enrichment.attributes();
    let cities = attributes
        .iter()
        .filter(|(key, _)| key.as_str() == LOCATION_CITY)
        .count();
    assert_eq!(cities, 1, "the second location should replace the first, not add to it");
    assert_eq!(enrichment.snapshot().get(LOCATION_CITY).unwrap(), "Lisbon");
    assert_eq!(enrichment.snapshot().get(LOCATION_COUNTRY).unwrap(), "PT");
}

#[test]
fn fetch_geolocation_parses_the_response() {
    let url = spawn_json_server(GEO_JSON);

    let geo = fetch_geolocation_from(&url).unwrap();

    assert_eq!(geo.city.as_deref(), Some("Amsterdam"));
    assert_eq!(geo.country.as_deref(), Some("NL"));
    assert_eq!(geo.ip.as_deref(), Some("1.2.3.4"));
    // Fields the service omitted stay `None` rather than becoming empty strings.
    assert_eq!(geo.postal, None);
}

// --- value tests ---

#[test]
fn value_converts_from_the_common_rust_types() {
    let owned = String::from("avif");

    // A borrowed, non-'static string: the case `AnyValue` itself cannot take.
    assert_eq!(Value::from(owned.as_str()), Value::String("avif".to_string()));
    assert_eq!(Value::from(owned.clone()), Value::String("avif".to_string()));
    assert_eq!(Value::from(true), Value::Bool(true));
    assert_eq!(Value::from(7u32), Value::Int(7));
    assert_eq!(Value::from(1_048_576usize), Value::Int(1_048_576));
    assert_eq!(Value::from(-3i8), Value::Int(-3));
    assert_eq!(Value::from(1.5f32), Value::Double(1.5));
    assert_eq!(Value::from(b"ab".as_slice()), Value::Bytes(vec![b'a', b'b']));
}

#[test]
fn unsigned_values_saturate_rather_than_wrap() {
    // OTLP has no unsigned integer type. Clamping keeps a nonsensical-but-large number from becoming a negative one.
    assert_eq!(Value::from(u64::MAX), Value::Int(i64::MAX));
    assert_eq!(Value::from(i64::MAX as u64), Value::Int(i64::MAX));
}

#[test]
fn value_converts_nested_collections() {
    assert_eq!(
        Value::from(vec![1i64, 2]),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );

    let map = HashMap::from([("codec", "avif")]);
    assert_eq!(Value::from(map), Value::Map(vec![("codec".to_string(), "avif".into())]));
}

#[test]
fn events_accept_dynamically_assembled_fields() {
    let (url, server) = spawn_collector();

    let telemetry = Telemetry::builder(&url, "test-service").build().unwrap();

    let collected: Vec<(String, Value)> = vec![
        ("codec".to_string(), "avif".into()),
        ("threads".to_string(), 8u32.into()),
    ];
    telemetry.event("encode.started").fields(collected).info();
    let _ = telemetry.flush();

    let request = server.join().unwrap();
    assert!(request.body_contains("codec"));
    assert!(request.body_contains("threads"));
}

// --- environment tests ---

#[test]
fn environment_renders_its_name() {
    assert_eq!(Environment::Development.to_string(), "development");
    assert_eq!(Environment::Production.to_string(), "production");
    assert_eq!(Environment::Custom("staging".to_string()).to_string(), "staging");
    assert_eq!(Environment::default(), Environment::Development);
}

// --- error tests ---
//
// `O11yError` hand-writes its `Display` and `source` arms, so a transposed line would go unnoticed without these.

#[test]
fn errors_describe_themselves_and_expose_their_source() {
    use std::error::Error;

    let io = O11yError::Io(std::io::Error::other("disk is on fire"));
    assert!(io.to_string().contains("disk is on fire"));
    assert!(io.source().is_some(), "a wrapped error must expose its source");

    let json = O11yError::Json(serde_json::from_str::<Geolocation>("not json").unwrap_err());
    assert!(json.to_string().contains("json"));
    assert!(json.source().is_some());

    let endpoint = O11yError::InvalidEndpoint {
        endpoint: "ht tp://nope".to_string(),
        reason: "relative URL without a base".to_string(),
    };
    assert!(endpoint.to_string().contains("ht tp://nope"));
    assert!(endpoint.to_string().contains("relative URL without a base"));
    assert!(endpoint.source().is_none(), "a self-describing error has no source");

    let header = O11yError::InvalidHeader {
        name: "bad name".to_string(),
    };
    assert!(header.to_string().contains("bad name"));
    assert!(header.source().is_none());
}
