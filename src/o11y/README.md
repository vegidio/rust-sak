# `o11y` module

Ships structured **OTLP log records** to an OpenTelemetry collector, enriching every record with a fixed set of attributes describing the machine and the current session. The API is **fully synchronous and needs no async runtime** — export happens on a background thread — and it is **privacy-opt-in twice over**: telemetry is switched off with one flag, and the IP-based location lookup with another.

## Enabling

Gated behind the `o11y` Cargo feature (no Tokio runtime required):

```toml
[dependencies]
rust-sak = { version = "2", features = ["o11y"] }
```

```rust
use rust_sak::o11y::{Telemetry, TelemetryBuilder, Event, Value, Environment, Geolocation, O11yError};
```

## Overview

- **`Telemetry`** — the handle. Build it **once** at startup, share it across threads behind an `Arc`, and emit records through it. Owns its logger provider, so several handles coexist without fighting over a process-wide global.
- **`TelemetryBuilder`** — the consuming configuration builder, created by `Telemetry::builder`.
- **`Event`** — a record under construction, returned by `Telemetry::event`. Nothing is sent until a severity method is called.
- **`Value`** — the field value type, with `From` impls for the everyday Rust types.
- **`Environment`** — the deployment environment reported with every record.
- **`Geolocation` / `fetch_geolocation`** — the optional IP-based location lookup, usable on its own. `fetch_geolocation_from` points it at another endpoint, and `fetch_geolocation_with` also chooses the timeout.
- **`O11yError`** — construction and shutdown failures. **Emitting a record never fails.**

The split between **configuration** (consumes `self`, returns `Self` — set up once) and **emit methods** (take `&self` — call many times) is what makes a single `Telemetry` shareable.

## `TelemetryBuilder` — configuration

These consume `self` and return `Self`, so chain them. Only the endpoint and service name are required.

| Method                                       | Default             | What it does                                                                                                                   |
|----------------------------------------------|---------------------|--------------------------------------------------------------------------------------------------------------------------------|
| `Telemetry::builder(endpoint, service_name)` | —                   | Starts a builder. `endpoint` is the collector's **base URL**; `/v1/logs` is appended to it.                                    |
| `.version(impl Into<String>)`                | `""`                | Application version, reported as the `version` attribute on every record.                                                      |
| `.environment(Environment)`                  | `Development`       | Reported as the `deployment.environment.name` resource attribute.                                                              |
| `.header(key, value)`                        | none                | One header on every OTLP export request, typically authentication. Call repeatedly.                                            |
| `.headers(iter)`                             | none                | Several headers at once, merged over any already set.                                                                          |
| `.enabled(bool)`                             | **`true`**          | Master switch. When `false`, **nothing leaves the process** and every record is discarded — but the handle stays fully usable. |
| `.geolocation(bool)`                         | **`false`**         | Opt-in IP-based location enrichment. Ignored unless `.enabled(true)`.                                                          |
| `.geolocation_url(impl Into<String>)`        | `https://ipinfo.io` | Point the lookup at a self-hosted or proxied endpoint. It requests `<base_url>/json`.                                          |
| `.timeout(Duration)`                         | exporter default    | How long a single **export** attempt may take. Reaches the OTLP exporter only, not the geolocation lookup.                     |
| `.geolocation_timeout(Duration)`             | `1s`                | How long the geolocation lookup may take. Worth raising for a self-hosted endpoint behind a VPN or proxy.                      |
| `.build()`                                   | —                   | `Result<Telemetry>`. Reports a bad endpoint, an unsendable header, or an exporter that could not be constructed.               |

> Because the endpoint is always supplied programmatically, the `OTEL_EXPORTER_OTLP_*` environment variables are ignored.

## `Telemetry` — emitting records

All take `&self`. Each severity has a **short form** for the no-fields case and the fluent `event()` path for everything else.

| Method                    | Signature                                            | What it does                                                                                     |
|---------------------------|------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `event`                   | `fn event(&self, name) -> Event<'_>`                 | Starts a record to which fields can be attached.                                                 |
| `debug` / `info` / `warn` | `fn info(&self, event)`                              | Emits a record with no fields of its own, at that severity.                                      |
| `error`                   | `fn error<E: Error + ?Sized>(&self, event, err: &E)` | Emits at `Error` severity with `err` attached.                                                   |
| `renew_session`           | `fn renew_session(&self)`                            | Assigns a fresh session id to every later record. Safe to call while other threads are emitting. |
| `session_id`              | `fn session_id(&self) -> String`                     | The session id currently attached to records.                                                    |
| `is_enabled`              | `fn is_enabled(&self) -> bool`                       | Whether this handle actually exports.                                                            |
| `flush`                   | `fn flush(&self) -> Result<()>`                      | Exports everything buffered so far, blocking until the batch is sent.                            |
| `shutdown`                | `fn shutdown(&self) -> Result<()>`                   | Flushes and shuts the exporter down. **Idempotent**, and also run by `Drop`.                     |
| `Telemetry::disabled()`   | `fn disabled() -> Self`                              | An infallible handle that discards everything and never touches the network.                     |

## Events and fields

An `Event` is `#[must_use]`: nothing is sent until `.info()`, `.warn()`, `.debug()` or `.error(&err)` is called.

| Method                             | What it does                                                                                          |
|------------------------------------|-------------------------------------------------------------------------------------------------------|
| `.field(key, value)`               | Attaches one field. `value` is anything convertible into a `Value`. Call repeatedly.                  |
| `.fields(iter)`                    | Attaches many at once, for a set assembled at runtime — the equivalent of a dynamic `map[string]any`. |
| `.debug()` / `.info()` / `.warn()` | Emits at that severity.                                                                               |
| `.error(&err)`                     | Emits at `Error` severity, attaching `error` (the `Display` form) and, if present, `error.source`.    |

A field whose name matches an enrichment attribute (`version`, `session.id`, …) **replaces it for that one record**; the enrichment itself is untouched.

### `Value` conversions

| Rust type                                                      | Becomes  |
|----------------------------------------------------------------|----------|
| `bool`                                                         | `Bool`   |
| `i8`/`i16`/`i32`/`i64`/`isize`, `u8`/`u16`/`u32`/`u64`/`usize` | `Int`    |
| `f32` / `f64`                                                  | `Double` |
| `&str` (any lifetime), `String`, `Cow<str>`                    | `String` |
| `&[u8]`                                                        | `Bytes`  |
| `Vec<T: Into<Value>>`                                          | `List`   |
| `HashMap<K: Into<String>, V: Into<Value>>`                     | `Map`    |

> OTLP has no unsigned integer type, so `u64`/`usize` values above `i64::MAX` **saturate** to `i64::MAX` rather than wrapping to a negative number. This type exists because OpenTelemetry's own `AnyValue` converts only from `&'static str` and from integers up to `u32` — neither of which covers a runtime-built path or a byte count.

## Enrichment and privacy

Every record carries these attributes, rendered once and swapped atomically so `renew_session` is safe under concurrency:

| Attribute      | Source                                                                                            |
|----------------|---------------------------------------------------------------------------------------------------|
| `version`      | `.version(..)`                                                                                    |
| `machine.id`   | The OS host id, **HMAC-SHA256-scoped to the service name**. Omitted if the host id is unreadable. |
| `machine.os`   | `std::env::consts::OS` (`macos`, `linux`, `windows`)                                              |
| `machine.arch` | `std::env::consts::ARCH` (`x86_64`, `aarch64`)                                                    |
| `session.id`   | A UUID v4, regenerated by `renew_session`                                                         |
| `location.*`   | `country`/`region`/`city`, **only** when geolocation was opted into and the lookup answered       |

`service.name` and `deployment.environment.name` are sent as OTLP *resource* attributes rather than per-record.

> **The raw host id is never emitted.** It is identical across every program on the machine, so sending it would let a backend correlate this application's telemetry with any other's. Keying an HMAC with it instead yields an id that is stable for this machine *and* this service, and uncorrelatable with the id any other service derives. This is the same construction as Go's `machineid.ProtectedID`, so a Rust and a Go build of the same application report the same id.

> **`enabled(false)` sends nothing at all** — no exporter is installed and no geolocation request is made. **`geolocation` is off by default** and is a separate decision from `enabled`: it is the one enrichment that necessarily discloses the machine's public IP address to a third party. Both switches must be on before any lookup happens. When on, it runs on a background thread, so `build()` never blocks; records emitted before it lands simply carry no location.

## Errors

`O11yError` is `Exporter` / `Sdk` / `Http` / `Io` / `Json` / `InvalidEndpoint { endpoint, reason }` / `InvalidHeader { name }`. Implements `Display`, `Error` (with `source()`), and `From` for each wrapped error type. A `Result<T>` alias is re-exported alongside it.

## Usage

### Wiring it up at startup

```rust
use rust_sak::o11y::{Environment, Telemetry};

# fn run(user_opted_in: bool, user_shared_region: bool) -> Result<(), Box<dyn std::error::Error>> {
let telemetry = Telemetry::builder("https://collector.example.com", "open-photo-ai")
    .version(env!("CARGO_PKG_VERSION"))
    .environment(Environment::Production)
    .header("Authorization", "Bearer secret")
    .enabled(user_opted_in)
    .geolocation(user_shared_region)
    .build()?;

telemetry.info("app.started");

// ... application runs ...

telemetry.shutdown()?; // Drop would do this too
# Ok(())
# }
```

### Never failing, the way the Go version behaves

`build()` reports a bad configuration, but a telemetry failure should rarely take an application down. Fall back to a discarding handle in one line:

```rust
use rust_sak::o11y::Telemetry;

let telemetry = Telemetry::builder("https://collector.example.com", "open-photo-ai")
    .build()
    .unwrap_or_else(|_| Telemetry::disabled());

telemetry.info("app.started"); // works either way
```

### A record with mixed-type fields

```rust
# use rust_sak::o11y::Telemetry;
# fn run(telemetry: &Telemetry, path: &str) {
telemetry
    .event("export.finished")
    .field("format", "avif")
    .field("path", path)          // a borrowed, non-'static &str
    .field("bytes", 1_048_576u64) // a u64
    .field("resized", true)
    .info();
# }
```

### Recording an error

```rust
# use rust_sak::o11y::Telemetry;
# fn run(telemetry: &Telemetry) {
if let Err(err) = std::fs::read("/etc/nope") {
    telemetry.event("config.read_failed").field("path", "/etc/nope").error(&err);
}

// Or the short form, when there are no fields to add:
if let Err(err) = std::fs::read("/etc/nope") {
    telemetry.error("config.read_failed", &err);
}
# }
```

### Fields assembled at runtime

```rust
use std::collections::HashMap;
use rust_sak::o11y::{Telemetry, Value};

# fn run(telemetry: &Telemetry) {
let mut collected: HashMap<String, Value> = HashMap::new();
collected.insert("codec".to_string(), "avif".into());
collected.insert("threads".to_string(), 8u32.into());

telemetry.event("encode.started").fields(collected).info();
# }
```

### Starting a new session

```rust
# use rust_sak::o11y::Telemetry;
# fn run(telemetry: &Telemetry) {
telemetry.renew_session(); // every later record carries a new session.id
# }
```

### Sharing across threads

```rust
use std::sync::Arc;
use rust_sak::o11y::Telemetry;

# fn run() {
let telemetry = Arc::new(Telemetry::disabled());

let worker = Arc::clone(&telemetry);
std::thread::spawn(move || {
    worker.event("worker.started").field("id", 1u32).info();
});
# }
```

For the full API — including `Event`, `Value` and the geolocation helpers — see the module rustdoc.
