# `o11y` module

Ships **logs, metrics and traces** to an OpenTelemetry collector over OTLP/HTTP. One call to `init` at startup, and
every function in the program can record telemetry without holding a handle — the same shape `log` and `tracing` use.

The API is **fully synchronous and needs no async runtime**: export happens on a dedicated thread. It is
**privacy-opt-in twice over**: telemetry is switched off with one flag, and the IP-based location lookup with
another.

## Enabling

```toml
[dependencies]
rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["o11y"] }
```

```rust
use rust_sak::o11y::{self, Config, Environment, Level, log, metric, trace};
```

## What it costs the caller

This module is built around one constraint: **instrumenting a function must not change how long it takes**. A
function that runs in 100 ms still runs in 100 ms, however many records it emits and however unreachable the
collector is.

| Call                            | Cost        | Why                                                                                         |
|---------------------------------|-------------|---------------------------------------------------------------------------------------------|
| `log::debug!` below `min_level` | ~1 ns       | One relaxed atomic load and a branch. **The field expressions are never evaluated.**        |
| `ORDERS.increment(1)`           | ~5 ns       | One `fetch_add` on the instrument's own memory. No buffer, no allocation.                   |
| `ORDERS.add_with_tags(..)`      | ~40 ns      | A read lock and a hash lookup to find the series, then the same `fetch_add`.                |
| `HIST.record(v)`                | ~15 ns      | A short bucket scan, two atomic adds, one compare-and-swap for the running total.           |
| `log::info!` when recorded      | ~150–300 ns | Fields into an owned record, pushed onto a bounded queue under a lock held for nanoseconds. |
| `trace::span!`                  | ~50 ns      | A context pushed onto a thread-local stack; the record is built when the guard drops.       |

Two invariants keep the tail as flat as the median. **The buffer never pushes back** — it is bounded and discards
its oldest entry, so a dead collector costs records and nothing else. And **no lock is ever held across I/O** — the
export thread takes the whole queue in one move, then serialises and sends with nothing held.

## Setting up

```rust,no_run
use std::time::Duration;
use rust_sak::o11y::{self, Config, Environment, Level};

# fn run(user_opted_in: bool) -> Result<(), Box<dyn std::error::Error>> {
o11y::init(
    Config::builder("https://collector.example.com", [("Authorization", "Bearer secret")])
        .service_name("checkout-service")
        .service_version(env!("CARGO_PKG_VERSION"))
        .environment(Environment::Production)
        .min_level(Level::Info)
        .flush_interval(Duration::from_secs(5))
        .max_batch_size(512)
        .enabled(user_opted_in)
        .on_export_error(|e| eprintln!("telemetry export failed: {e}"))
        .build(),
)?;

// ... application runs ...

o11y::shutdown();
# Ok(())
# }
```

> **`shutdown()` is not optional.** There is no handle to drop, so nothing else flushes the buffers — this is the one
> piece of ceremony a process-global API cannot avoid. It blocks for at most one export `timeout`.

### `ConfigBuilder`

`Config::builder(endpoint, headers)` takes the collector's **base URL** — `/v1/logs`, `/v1/metrics` and
`/v1/traces` are appended — and the headers to put on every export request, which is where authentication belongs.
Pass `NO_HEADERS` when there is none. Everything else has a default.

| Method                     | Default             | What it does                                                                             |
|----------------------------|---------------------|------------------------------------------------------------------------------------------|
| `.service_name(..)`        | `""`                | `service.name`, and the scope of the machine identifier.                                 |
| `.service_version(..)`     | omitted             | `service.version`.                                                                       |
| `.environment(..)`         | `Development`       | `deployment.environment.name`.                                                           |
| `.min_level(..)`           | `Level::Info`       | The lowest severity recorded. Below it, a log macro evaluates nothing.                   |
| `.header(k, v)`            | none                | One more header, on top of those passed to `builder`.                                    |
| `.flush_interval(..)`      | `5s`                | How often the worker exports.                                                            |
| `.max_batch_size(..)`      | `512`               | How many buffered records trigger an export early.                                       |
| `.max_buffered(..)`        | `8192`              | Per-signal buffer ceiling. This is what a stalled collector can cost in memory.          |
| `.timeout(..)`             | `10s`               | How long one export attempt may take, and so how long `shutdown` can block.              |
| `.enabled(..)`             | **`true`**          | Master switch. When `false`, **nothing leaves the process**.                             |
| `.geolocation(..)`         | **`false`**         | Opt-in IP-based location enrichment. Ignored unless `.enabled(true)`.                    |
| `.geolocation_url(..)`     | `https://ipinfo.io` | Point the lookup at a self-hosted or proxied endpoint.                                   |
| `.geolocation_timeout(..)` | `1s`                | How long that lookup may take.                                                           |
| `.on_export_error(..)`     | discard             | Where export failures are reported. The **only** way to learn telemetry is not arriving. |
| `.build()`                 | —                   | A `Config`. Validation happens in `init`, not here.                                      |

## Logs

```rust
use rust_sak::o11y::log;

# let order_id = "ord_8812";
# let amount = 129.5;
# let e = std::fmt::Error;
log::debug!("cache lookup", key = "orders:8812", hit = false);
log::info!("order received", order_id = order_id, amount = amount);
log::warn!("large order flagged", order_id = order_id, threshold = 10_000.0);
log::error!("payment failed", order_id = order_id, error = %e);
```

The message is a **constant string, not a format string** — `log::info!("order {id} received")` does not
interpolate. Keeping the message fixed and putting what varies into fields is what lets a backend group every
instance of an event and filter on `order_id`, which a formatted string cannot support.

### Field syntax

| Written as     | Captured as                                   |
|----------------|-----------------------------------------------|
| `key = value`  | `Value::from(value)` — anything `Into<Value>` |
| `key = %value` | its `Display` form, as a string               |
| `key = ?value` | its `Debug` form, as a string                 |
| `key`          | shorthand for `key = key`                     |

A key is normally an identifier. A string literal works too, for the dotted names the semantic conventions use:

```rust
use rust_sak::o11y::log;

log::info!("request finished", "http.response.status_code" = 200);
```

## Metrics

Instruments are created **once** and called many times. The intended shape is a `static` behind a `LazyLock`:

```rust
use std::sync::LazyLock;
use rust_sak::o11y::metric::{self, Counter, Gauge, Histogram};

static ORDERS: LazyLock<Counter> = LazyLock::new(|| metric::counter("orders_total"));
static ORDER_VALUE: LazyLock<Histogram> =
    LazyLock::new(|| metric::histogram("order_value_dollars").with_buckets(&[10.0, 50.0, 100.0, 500.0]));
static QUEUE_DEPTH: LazyLock<Gauge> = LazyLock::new(|| metric::gauge("queue_depth"));

ORDERS.increment(1);
ORDERS.add_with_tags(1, &[("region", "eu-west-1")]);
ORDER_VALUE.record(129.5);
QUEUE_DEPTH.set(12);
```

Metrics differ from logs and spans in three ways worth knowing:

- **They are not gated and not buffered.** An instrument accumulates into its own atomics and the exporter reads
  them on its own schedule. Recording costs the same whether telemetry is running or not, because the atomic
  operation is cheaper than the branch that would skip it.
- **A `static` works before `init`, and without it.** A `static` is first touched by whatever code path reaches it,
  which the module cannot control — so the instrument registry is deliberately independent of initialisation. Touch
  a counter before `init` and its total is exported once telemetry starts.
- **Values are cumulative**, not the change since the last export. A dropped export therefore loses nothing, which
  matters because this module drops rather than retries when a collector is unreachable.

Tag order does not matter: `&[("a", "1"), ("b", "2")]` and `&[("b", "2"), ("a", "1")]` are the same series. An
instrument tracks at most 1024 distinct tag sets; beyond that, samples fold into a single `o11y.series_overflow`
series, so the total stays right while memory stays bounded. Tag values drawn from an unbounded domain — a user id,
a request path, a raw error string — are what that limit is there for.

## Traces

A span opens when `span!` is called and closes when its guard drops, which covers a normal return, an early return
through `?`, and a panic unwinding through the frame:

```rust
use rust_sak::o11y::trace;

# #[derive(Debug)] struct PaymentError;
# impl std::fmt::Display for PaymentError {
#     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("failed") }
# }
# impl std::error::Error for PaymentError {}
# fn validate_card() -> Result<(), PaymentError> { Ok(()) }
fn charge_card(order_id: &str) -> Result<(), PaymentError> {
    let _span = trace::span!("charge_card", order_id = order_id);
    validate_card()?;
    submit_to_processor(order_id)
}

#[trace::instrument]
fn submit_to_processor(order_id: &str) -> Result<(), PaymentError> {
    trace::current().set_attribute("processor", "stripe");
    trace::current().add_event("submitted to processor");
    Ok(())
}
```

Spans nest: one opened inside another inherits its trace id and records it as its parent. **Any log record emitted
while a span is open carries that span's trace and span ids**, which is what lets a backend show a trace and its
logs together. Nothing is needed at the call site for it.

`trace::current()` reaches whichever span is open without a handle being threaded through, so a helper deep in a
call tree can annotate what its caller opened. Every method on it is a **silent no-op when no span is open** — the
alternative is an `Option` that every call site has to unwrap, for something that is not a failure.

### `#[instrument]`

Wraps the whole function body, naming the span after the function and capturing its arguments as fields. An
argument that converts into a `Value` on its own — every integer and float, `bool`, `&str`, `String` — is captured
in that variant directly, costing no formatting; anything else falls back to its `Debug` form, so an argument that
is neither must implement `Debug`.

```rust
use rust_sak::o11y::trace;

#[trace::instrument(skip(password))]
fn sign_in(user: &str, password: &str) -> bool {
    true
}

#[trace::instrument(name = "db.query", skip_all)]
fn query(sql: &str) {}
```

`skip(..)` leaves named arguments out, `skip_all` captures none, and `name = ".."` overrides the span name. A `self`
receiver and destructuring patterns such as `(x, y): (u32, u32)` are skipped, having no single name to record under.

### Async

**A bare guard is sync-only.** Holding one across an `.await` is wrong on a multi-threaded executor: the task can
resume on another thread, where the span is not on the stack, while the span stays open on the original thread and
mis-parents whatever is scheduled there next.

Attach the span to the future instead. Both forms enter and exit it around each poll, so it follows the task
wherever it runs:

```rust
use rust_sak::o11y::trace::{self, Instrument};

#[trace::instrument]
async fn charge(order_id: &str) {
    // `#[instrument]` on an `async fn` wraps the body, so this is already correct.
}

# async fn example() {
async { /* ... */ }
    .instrument(trace::span!("charge_card", order_id = "ord_8812"))
    .await;
# }
```

## Enrichment and privacy

Every batch carries these, as OTLP **resource** attributes — they are identical on every record, so sending them
once per batch rather than once per record is both correct and a large saving on the wire:

| Attribute                     | Source                                                                             |
|-------------------------------|------------------------------------------------------------------------------------|
| `service.name`                | `.service_name(..)`                                                                |
| `service.version`             | `.service_version(..)`, omitted when unset                                         |
| `deployment.environment.name` | `.environment(..)`                                                                 |
| `machine.id`                  | The OS host id, **HMAC-SHA256-scoped to the service name**. Omitted if unreadable. |
| `machine.os` / `machine.arch` | `std::env::consts::OS` / `ARCH` (`macos`, `aarch64`)                               |
| `session.id`                  | A UUID v4, regenerated by `o11y::renew_session()`                                  |
| `location.*`                  | `country`/`region`/`city`, **only** when geolocation was opted into and answered   |

> **The raw host id is never emitted.** It is identical across every program on the machine, so sending it would let
> a backend correlate this application's telemetry with any other's. Keying an HMAC with it instead yields an id
> stable for this machine *and* this service, and uncorrelatable with the id any other service derives. This is the
> same construction as Go's `machineid.ProtectedID`, so a Rust and a Go build of one application report the same id.

> **`enabled(false)` sends nothing at all** — no export thread is started, no geolocation request is made, and every
> record is discarded at the call site. **`geolocation` is off by default** and is a separate decision: it is the one
> enrichment that necessarily discloses the machine's public IP to a third party. Both switches must be on before any
> lookup happens. When on, it runs on a background thread, so `init` never blocks; records emitted before it lands
> simply carry no location.

`renew_session()` is safe to call while other threads are recording. Records already buffered keep the session they
were recorded under, rather than picking up whichever one was current when the exporter reached them.

## Errors

**Recording telemetry never fails.** The log macros, the metric instruments and the span guards all return nothing:
an uninitialised module, a disabled one and an unreachable collector are indistinguishable at the call site, by
design.

`init` returns `InvalidEndpoint`, `InvalidHeader` or `AlreadyInitialized`, and means the module never started.
Everything else reaches the `on_export_error` callback from the export thread: `ExportRejected { signal, status,
body }` for a non-2xx, `Dropped { signal, count }` when a full buffer discarded records, and `Http` / `Io` / `Json`
for a request that failed outright. `Signal` says which of the three streams was affected.

The callback runs on the export thread and **must not panic** — a panic there kills the thread, after which the
module silences itself rather than letting the buffers fill.

## Usage

### Never failing, whatever the configuration says

`init` reports a bad configuration, but telemetry should rarely take an application down:

```rust,no_run
use rust_sak::o11y::{self, Config, NO_HEADERS};

let _ = o11y::init(Config::builder("https://collector.example.com", NO_HEADERS)
    .service_name("checkout-service")
    .build());

// Records either way: a failed `init` leaves every call site a no-op.
rust_sak::o11y::log::info!("app.started");
```

### Wiring a user's consent to the two switches

```rust,no_run
use rust_sak::o11y::{self, Config, NO_HEADERS};

# fn run(user_opted_in: bool, user_shared_region: bool) -> Result<(), Box<dyn std::error::Error>> {
o11y::init(
    Config::builder("https://collector.example.com", NO_HEADERS)
        .service_name("checkout-service")
        .enabled(user_opted_in)
        .geolocation(user_shared_region)
        .build(),
)?;
# Ok(())
# }
```

### Fields assembled at runtime

```rust
use std::borrow::Cow;
use rust_sak::o11y::{Value, log};

let mut fields: Vec<(Cow<'static, str>, Value)> = Vec::new();
fields.push((Cow::Borrowed("codec"), "avif".into()));
fields.push((Cow::Borrowed("threads"), 8u32.into()));

// The macros cover the call-site case; a set built at runtime is what naming `Value` is for.
log::info!("encode.started", count = fields.len());
```
