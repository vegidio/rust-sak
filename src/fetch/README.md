# `fetch` module

A configurable, reusable async HTTP client built on `reqwest` + `tokio`, with automatic retries (Fibonacci backoff) and streaming file downloads that report live progress.

## Enabling

Gated behind the `fetch` Cargo feature (requires a Tokio runtime):

```toml
[dependencies]
rust-sak = { version = "1", features = ["fetch"] }
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
| `Fetch::new()` / `Fetch::default()`          | New fetcher. Defaults: no headers, no retries, HTTP/2 on, **30s read (idle) timeout**, `DownloadMode::Resume`.          |
| `.header(key, value)`                        | Add one default header sent with every request. **Panics** on an invalid name/value — use for static headers.           |
| `.headers(HeaderMap)`                        | Replace the entire default header set.                                                                                  |
| `.retries(u32)`                              | Number of retry attempts for failed requests.                                                                           |
| `.disable_http2(bool)`                       | `true` forces HTTP/1.x; `false` keeps HTTP/2.                                                                           |
| `.read_timeout(impl Into<Option<Duration>>)` | Idle timeout per read — resets after each successful read, so it bounds stalls, not total duration. `None` disables it. |
| `.download_mode(DownloadMode)`               | Default behavior when a download target already exists.                                                                 |

## `Fetch` — request methods

All take `&self`. The struct's defaults apply; anything set on `options` overrides them for that one call. Retries use Fibonacci backoff (1s, 2s, 3s, 5s, 8s, … capped at 60s) and apply **only to idempotent methods** (GET/HEAD/PUT/DELETE/OPTIONS/TRACE) unless `RequestOptions::retry_non_idempotent(true)` opts in.

| Method     | Signature                                                                              | What it does                                                                                                                                                                                                                                    |
|------------|----------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `text`     | `async fn text(&self, url, options) -> Result<String, reqwest::Error>`                 | Sends the request, returns the body as a `String`.                                                                                                                                                                                              |
| `json`     | `async fn json<T: DeserializeOwned>(&self, url, options) -> Result<T, reqwest::Error>` | Same as `text` but deserializes the JSON body into `T`.                                                                                                                                                                                         |
| `download` | `fn download(&self, url, path, options) -> Download`                                   | **Non-async, infallible.** Spawns a background task to stream the body to `path` and returns a `Download` handle immediately. Setup errors (bad URL, client build) surface through the handle. **Panics** if not called within a Tokio runtime. |

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

## Downloads

### `DownloadMode` (what to do when the target file already exists)

| Variant              | Behavior                                                                                                                                                                                                                   |
|----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Resume` *(default)* | Continue an incomplete file via an HTTP `Range` request, appending the remaining bytes. Falls back to a full redownload if the server ignores `Range`. Re-reads the on-disk length on each attempt, so retries resume too. |
| `Overwrite`          | Always truncate and download from byte zero.                                                                                                                                                                               |
| `Skip`               | If any file exists at the path, do nothing and report complete **without contacting the server**.                                                                                                                          |

### `Download` (the progress handle)

Dropping the handle does **not** cancel the download.

| Method                                                     | What it does                                                                                                                                                                                                                                           |
|------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `.progress() -> Progress`                                  | Latest snapshot (cheap clone).                                                                                                                                                                                                                         |
| `.completed() -> bool`                                     | `true` once finished (success or failure).                                                                                                                                                                                                             |
| `.failed() -> bool`                                        | `true` if finished with an error.                                                                                                                                                                                                                      |
| `.changed() -> Result<(), RecvError>`                      | Await the next progress update.                                                                                                                                                                                                                        |
| `.track(&mut self, callback) -> Result<(), DownloadError>` | Drive a `FnMut(Option<u64> total, u64 downloaded, Option<f64> fraction)` callback on every update, then resolve with the final result. **Borrows** the handle (stays usable after), but awaits the task **once** — do **not** call `join()` afterward. |
| `.cancel(&self)`                                           | Abort the background task. The transfer then surfaces as `DownloadError::Cancelled`; any partial file is left on disk.                                                                                                                                 |
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

// Plain text body.
let html = fetch.text("https://example.com", RequestOptions::new()).await?;

// JSON into a typed value.
#[derive(serde::Deserialize)]
struct Repo { name: String, stargazers_count: u32 }

let repo: Repo = fetch
    .json("https://api.github.com/repos/rust-lang/rust", RequestOptions::new())
    .await?;

// POST with a JSON body.
let echoed = fetch
    .text(
        "https://httpbin.org/post",
        RequestOptions::new()
            .method(reqwest::Method::POST)
            .body(serde_json::json!({ "hello": "world" })),
    )
    .await?;

// Streaming download with live progress.
let mut dl = fetch.download(
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
