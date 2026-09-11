# `fetch` module

A configurable, reusable async HTTP client built on `reqwest` + `tokio`, with automatic retries (Fibonacci backoff) and streaming file downloads that report live progress.

## Enabling

Gated behind the `fetch` Cargo feature (requires a Tokio runtime):

```toml
[dependencies]
rust-sak = { version = "2", features = ["fetch"] }
```

```rust
use rust_sak::fetch::{Fetch, RequestOptions, Download, DownloadMode, DownloadError, Progress};
```

## Overview

- **`Fetch`** — the client. Configure it **once** with a fluent builder, then **reuse** it for many requests. It lazily builds and caches an internal `reqwest::Client` (and its connection pool) on the first request.
- **`RequestOptions`** — per-request overrides (method, query, headers, body, retries, download mode) that take priority over the `Fetch` defaults.
- **`Download` / `Progress` / `DownloadMode` / `DownloadError`** — the streaming-download support returned by `Fetch::download`.

The split between **config builders** (consume `self`, return `Self` — set up once) and **request methods** (take `&self` — call many times) is what makes a single `Fetch` shareable.

## `Fetch` — configuration builders

These consume `self` and return `Self`, so chain them. Changing a client-build setting (headers, HTTP/2, timeout) resets the cached client so the next request rebuilds it.

| Method                                       | What it does                                                                                                            |
|----------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| `Fetch::new()` / `Fetch::default()`          | New fetcher. Defaults: no headers, no retries, HTTP/2 on, **30s read (idle) timeout**, no connect timeout, `reqwest` proxy detection, `DownloadMode::Resume`. |
| `.header(key, value)`                        | Add one default header sent with every request. **Panics** on an invalid name/value — use for static headers.           |
| `.headers(HeaderMap)`                        | Replace the entire default header set.                                                                                  |
| `.retries(u32)`                              | Number of retry attempts for failed requests.                                                                           |
| `.disable_http2(bool)`                       | `true` forces HTTP/1.x; `false` keeps HTTP/2.                                                                           |
| `.read_timeout(impl Into<Option<Duration>>)` | Idle timeout per read — resets after each successful read, so it bounds stalls, not total duration. `None` disables it. |
| `.download_mode(DownloadMode)`               | Default behavior when a download target already exists.                                                                 |
| `.connect_timeout(impl Into<Option<Duration>>)` | Bound on establishing the connection (DNS + TCP + TLS), before any bytes move. `None` leaves it unbounded. |
| `.proxy(ProxySettings)`                      | Route through an explicit proxy, overriding the environment and system settings.                                         |
| `.no_proxy()`                                | Connect directly, ignoring **all** proxy configuration including `HTTPS_PROXY`.                                          |


## Proxies and connection bounding

With nothing configured, `reqwest`'s own detection applies: the `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` environment variables on every platform, plus the system proxy settings on macOS and Windows. The two builders below exist to *override* that, not to enable it.

`ProxySettings` carries the proxy URL and, optionally, credentials and a bypass list. Its `Display` prints the URL alone, so it can be named in a log line or an error message without leaking the password.

```rust
use rust_sak::fetch::{Fetch, ProxySettings};

// Override whatever the environment says.
let via_proxy = Fetch::new().proxy(
    ProxySettings::new("http://proxy.example.com:3128")
        .basic_auth("user", "secret")
        .no_proxy("localhost,127.0.0.1,.internal"),
);

// Or opt out of proxying entirely, environment variables included.
let direct = Fetch::new().no_proxy();
```

An invalid proxy URL is not rejected by `.proxy(...)`; it surfaces as the `reqwest::Error` from the first request, which is where the client is built.

`connect_timeout` bounds a different failure from `read_timeout`. `read_timeout` bounds how long an *established* connection may stall; a host that accepts nothing at all never reaches a read, so only `connect_timeout` bounds it.

```rust
use std::time::Duration;
use rust_sak::fetch::Fetch;

let fetch = Fetch::new()
    .connect_timeout(Duration::from_secs(30)) // bounds the handshake
    .read_timeout(Duration::from_secs(30));   // bounds a stall mid-transfer
```

## `Fetch` — request methods

All take `&self`. Each request method comes in two forms: a short form that uses default per-request options, and a `*_with_options` form that takes an explicit `RequestOptions`. The struct's defaults apply either way; anything set on `options` overrides them for that one call. Retries use Fibonacci backoff (1s, 2s, 3s, 5s, 8s, … capped at 60s) and apply **only to idempotent methods** (GET/HEAD/PUT/DELETE/OPTIONS/TRACE) unless `RequestOptions::retry_non_idempotent(true)` opts in.

| Method                  | Signature                                                                                 | What it does                                                                                                                                                                                                                                    |
|-------------------------|-------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `text`                  | `async fn text(&self, url) -> Result<String, reqwest::Error>`                             | Sends a `GET` with default options, returns the body as a `String`.                                                                                                                                                                            |
| `text_with_options`     | `async fn text_with_options(&self, url, options) -> Result<String, reqwest::Error>`       | Same as `text` but applies the per-request `options`.                                                                                                                                                                                           |
| `json`                  | `async fn json<T: DeserializeOwned>(&self, url) -> Result<T, reqwest::Error>`             | Same as `text` but deserializes the JSON body into `T`.                                                                                                                                                                                         |
| `json_with_options`     | `async fn json_with_options<T: DeserializeOwned>(&self, url, options) -> Result<T, …>`    | Same as `json` but applies the per-request `options`.                                                                                                                                                                                           |
| `download`              | `fn download(&self, url, path) -> Download`                                               | **Non-async, infallible.** Spawns a background task to stream the body to `path` and returns a `Download` handle immediately. Setup errors (bad URL, client build) surface through the handle. **Panics** if not called within a Tokio runtime. |
| `download_with_options` | `fn download_with_options(&self, url, path, options) -> Download`                         | Same as `download` but applies the per-request `options` (including `download_mode`).                                                                                                                                                          |

## `RequestOptions` — per-request overrides

A consuming builder. Anything left unset is inherited from the `Fetch` struct. Headers merge per-key (request value wins, other struct headers preserved); query params are appended in insertion order.

| Method                                  | What it does                                                                                                          |
|-----------------------------------------|-----------------------------------------------------------------------------------------------------------------------|
| `RequestOptions::new()`                 | Empty options (everything inherited).                                                                                 |
| `.method(reqwest::Method)`              | HTTP method (default `GET`).                                                                                          |
| `.header(k, v)` / `.headers(HeaderMap)` | Add/replace per-request headers.                                                                                      |
| `.query(k, v)`                          | Append a query parameter (call repeatedly).                                                                           |
| `.retries(u32)`                         | Override retry count for this request.                                                                                |
| `.retry_non_idempotent(bool)`           | Allow retries for `POST`/`PATCH` etc. (off by default — re-sending a non-idempotent request risks a duplicate write). |
| `.body<T: Serialize>(body)`             | Attach a JSON body, sent with `Content-Type: application/json`. **Panics** if not serializable.                       |
| `.download_mode(DownloadMode)`          | Override the download mode (no effect on `text`/`json`).                                                              |
| `.resume_key(impl Into<String>)`        | Identify a download's bytes beyond their URL, so a partial recorded under a different key is discarded rather than resumed. A pinned content hash is the natural value. |

## Downloads

### On-disk layout: `.part` plus a sidecar

A transfer never writes to its target path. Bytes go to `<path>.part`, and a `<path>.part.json` sidecar records what
they are:

```text
  during transfer                      on success
  ---------------                      ----------
  release.7z.part                -->   flush + fsync, remove the sidecar,
  release.7z.part.json           -->   rename release.7z.part -> release.7z
  { "url": "https://.../runtime/1.26.0/release.7z",
    "etag": "\"a1b2\"",
    "total": 174834737,                a file at the target path is now,
    "resume_key": "5cafbaae..." }      by construction, complete
```

Two things follow, and they are why the files exist:

- **A file at the target path is complete, always.** An interrupted, failed or cancelled transfer leaves a `.part`
  beside the target, never a truncated file at it. `Skip` can therefore no longer mistake a half-finished download for
  a finished one.
- **A resume is provably a resume of the right thing.** Versioned release assets change the URL while keeping the same
  file name, so "same local path" says nothing about "same bytes". The sidecar is checked *before* the `Range` request
  is built.

What a `Resume` request does with what it finds:

| On disk                       | Sidecar                          | Action                                                             |
|-------------------------------|----------------------------------|--------------------------------------------------------------------|
| the target path exists        | —                                | complete; nothing is transferred                                   |
| `.part` + matching sidecar    | `url` **and** `resume_key` match | `Range` from the `.part`'s length, plus `If-Range` if an `ETag` was recorded |
| `.part` + sidecar             | either differs                   | delete both, start fresh                                           |
| `.part`, no sidecar           | —                                | delete, start fresh — the bytes' identity is unknowable            |
| nothing                       | —                                | fresh                                                              |

`If-Range` is the third layer: where the sidecar catches a URL that changed, `If-Range` catches bytes that changed at a
URL that did not. A server whose resource no longer matches the recorded `ETag` answers `200` rather than `206`, which
is already handled as a fresh download.

Concurrent transfers to the same target path are not supported, and nothing locks across processes; callers are assumed
to be single-writer per path.

### `DownloadMode` (what already-present bytes mean)

| Variant              | Behavior                                                                                                                                                                                                                                     |
|----------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Resume` *(default)* | A file at the target path is complete, and is reported as such without contacting the server. Otherwise continue `<path>.part` with an HTTP `Range` request — but only if its sidecar says it belongs to this request. Falls back to a full redownload if the partial is not ours or the server ignores `Range`. Re-reads the partial's length on each attempt, so retries resume too. |
| `Overwrite`          | Discard any partial and download from byte zero, replacing whatever is at the target path.                                                                                                                                                    |
| `Skip`               | If a file exists at the target path, do nothing and report complete **without contacting the server**. Otherwise behaves as `Resume`.                                                                                                          |

### `Download` (the progress handle)

Dropping the handle does **not** cancel the download.

| Method                                                     | What it does                                                                                                                                                                                                                                           |
|------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `.progress() -> Progress`                                  | Latest snapshot (cheap clone).                                                                                                                                                                                                                         |
| `.completed() -> bool`                                     | `true` once finished (success or failure).                                                                                                                                                                                                             |
| `.failed() -> bool`                                        | `true` if finished with an error.                                                                                                                                                                                                                      |
| `.changed() -> Result<(), RecvError>`                      | Await the next progress update.                                                                                                                                                                                                                        |
| `.track(&mut self, callback) -> Result<(), DownloadError>` | Drive a `FnMut(Option<u64> total, u64 downloaded, Option<f64> fraction)` callback on every update, then resolve with the final result. **Borrows** the handle (stays usable after), but awaits the task **once** — do **not** call `join()` afterward. |
| `.cancel(&self)`                                           | Abort the background task. The transfer then surfaces as `DownloadError::Cancelled`; `<path>.part` and its sidecar are left on disk (so a later `Resume` picks up where it stopped) but **nothing is left at the target path**.                        |
| `.join(self) -> Result<(), DownloadError>`                 | Consume the handle and await the final result.                                                                                                                                                                                                         |

### `Progress` (public fields)

`total: Option<u64>` · `downloaded: u64` · `progress: Option<f64>` (0.0–1.0, `None` when total unknown) · `completed: bool` · `failed: bool`.

### `DownloadError`

`Http(reqwest::Error)` · `Io(std::io::Error)` · `Cancelled`. Implements `Display`, `Error`, and `From` for the wrapped error types.

## Usage

```rust
use rust_sak::fetch::{Fetch, RequestOptions, DownloadMode};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
// Configure once, reuse everywhere.
let fetch = Fetch::new()
    .header("Accept", "application/json")
    .retries(3);

// Plain text body — no per-request options needed.
let html = fetch.text("https://example.com").await?;

// JSON into a typed value.
#[derive(serde::Deserialize)]
struct Repo { name: String, stargazers_count: u32 }

let repo: Repo = fetch
    .json("https://api.github.com/repos/rust-lang/rust")
    .await?;

// POST with a JSON body — per-request overrides via `*_with_options`.
let echoed = fetch
    .text_with_options(
        "https://httpbin.org/post",
        RequestOptions::new()
            .method(reqwest::Method::POST)
            .body(serde_json::json!({ "hello": "world" })),
    )
    .await?;

// Streaming download with live progress.
let mut dl = fetch.download_with_options(
    "https://example.com/big.bin",
    "/tmp/big.bin",
    RequestOptions::new().download_mode(DownloadMode::Resume),
);

dl.track(|total, downloaded, progress| match progress {
    Some(fraction) => println!("{:.0}% ({downloaded} bytes)", fraction * 100.0),
    None => println!("{downloaded} bytes (unknown total: {total:?})"),
})
.await?;
# Ok(())
# }
```

For finer control, poll `download.progress()` and await `download.changed()` in a loop, then await the outcome with `download.join()`.

Progress updates are **coalesced**: one when the response headers land, then at most one per 256 KiB or per 100 ms, plus a final one carrying the exact byte count. The update rate therefore does not depend on how the server framed the body.
