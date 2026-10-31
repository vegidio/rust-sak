//! HTTP fetching utilities.
//!
//! This module exposes [`Fetch`], a configurable HTTP request builder. It holds the default configuration (headers,
//! retries, HTTP/2 toggle) and sends requests via [`Fetch::text`] (raw body) or [`Fetch::json`] (deserialized into a
//! caller-chosen type), retrying with Fibonacci backoff. The `*_response` forms of both return a [`Response`] instead
//! — the body with its status and headers, for an answer that is not only in the body, such as a `Link` pagination
//! cursor. [`Fetch::download`] instead streams a response body to a file
//! and returns a [`Download`] handle immediately, exposing live progress as a [`Progress`] snapshot (and surfacing
//! failures as a [`DownloadError`]). Individual requests can override the defaults — including attaching a JSON request
//! body — by passing [`RequestOptions`].

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod digest;
mod download;
mod error;
mod partial;
mod prepared;
mod proxy;
mod request;
mod response;
mod retry;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

// Re-exported because this module's public API is stated in `reqwest`'s types: every request failure carries a
// `reqwest::Error`, `Response::status` is a `reqwest::StatusCode` and `Response::headers` a `reqwest::header::HeaderMap`.
// Without this a consumer cannot name what it is handed without depending on `reqwest` itself and keeping the version
// aligned with this crate's — and two versions would make these two unrelated types.
pub use reqwest;

pub use digest::DigestAlgorithm;
pub use download::{Download, DownloadError, DownloadMode, Progress};
pub use error::FetchError;
pub use proxy::ProxySettings;
pub use request::RequestOptions;
pub use response::Response;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::header::HeaderMap;

use prepared::PreparedRequest;
use proxy::ProxyMode;

/// A configurable, reusable HTTP fetcher, built with a fluent (consuming) builder API.
///
/// A `Fetch` holds the default configuration (headers, retries, HTTP/2 toggle, read timeout, download mode) and a
/// lazily built, reusable [`reqwest::Client`]. The client — and its connection pool — is constructed on the first
/// request and reused across later requests, so a `Fetch` is meant to be configured once and shared (the request
/// methods take `&self`). Mutating the configuration via the builder methods resets the cached client so it is rebuilt
/// with the new settings.
///
/// ```
/// use rust_sak::fetch::Fetch;
///
/// let fetch = Fetch::new()
///     .header("Accept", "application/json")
///     .retries(3)
///     .disable_http2(true);
/// ```
#[derive(Debug)]
pub struct Fetch {
    /// Headers sent with every request.
    headers: HeaderMap,
    /// Number of times a failed request is retried.
    retries: u32,
    /// When `true`, requests are forced over HTTP/1.x instead of HTTP/2.
    disable_http2: bool,
    /// Idle timeout applied per read: a request errors if no data arrives within this window (the timer resets on each
    /// successful read). `None` disables it. Defaults to 30 seconds.
    read_timeout: Option<Duration>,
    /// Bound on establishing the connection — DNS, TCP and TLS — before any bytes are exchanged.
    /// `None` leaves it unbounded, which is `reqwest`'s own default. Defaults to `None`.
    connect_timeout: Option<Duration>,
    /// How the proxy is resolved. Defaults to [`ProxyMode::Detect`], which leaves `reqwest`'s own
    /// environment and system-settings detection in place.
    proxy: ProxyMode,
    /// Default [`DownloadMode`] for [`Fetch::download`], overridable per request via
    /// [`RequestOptions::download_mode`]. Defaults to [`DownloadMode::Resume`].
    download_mode: DownloadMode,
    /// The largest response body the `text` and `json` methods will read, overridable per request via
    /// [`RequestOptions::max_body_bytes`]. `None`, the default, reads any size.
    max_body_bytes: Option<u64>,
    /// Lazily built, reused HTTP client. Cleared by the config builders, so the next request rebuilds it.
    client: OnceLock<reqwest::Client>,
}

impl Default for Fetch {
    /// The default configuration: no headers, no retries, HTTP/2 enabled, a 30-second read (idle) timeout, an
    /// unbounded connect timeout, `reqwest`'s own proxy detection, and [`DownloadMode::Resume`] for downloads.
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            retries: 0,
            disable_http2: false,
            read_timeout: Some(Duration::from_secs(30)),
            connect_timeout: None,
            proxy: ProxyMode::Detect,
            download_mode: DownloadMode::Resume,
            max_body_bytes: None,
            client: OnceLock::new(),
        }
    }
}

impl Clone for Fetch {
    /// Clones the configuration; the cloned `Fetch` starts with an empty client cache (a fresh client is built on its
    /// first request).
    fn clone(&self) -> Self {
        Self {
            headers: self.headers.clone(),
            retries: self.retries,
            disable_http2: self.disable_http2,
            read_timeout: self.read_timeout,
            connect_timeout: self.connect_timeout,
            proxy: self.proxy.clone(),
            download_mode: self.download_mode,
            max_body_bytes: self.max_body_bytes,
            client: OnceLock::new(),
        }
    }
}

impl Fetch {
    /// Creates a new [`Fetch`] with the default configuration: no headers, no retries, HTTP/2 enabled, a
    /// 30-second read (idle) timeout, an unbounded connect timeout, and `reqwest`'s own proxy detection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a single header sent with every request.
    ///
    /// Accepts anything convertible into a header name and value (e.g. `&str`).
    ///
    /// # Panics
    ///
    /// Panics if `key` is not a valid header name or `value` is not a valid header value. This is intended for
    /// statically known headers; for headers built from untrusted input, validate them first and use
    /// [`Fetch::headers`].
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: TryInto<reqwest::header::HeaderName>,
        K::Error: std::fmt::Debug,
        V: TryInto<reqwest::header::HeaderValue>,
        V::Error: std::fmt::Debug,
    {
        insert_header(&mut self.headers, key, value);
        self.client = OnceLock::new();
        self
    }

    /// Replaces the full set of headers.
    pub fn headers(mut self, mut headers: HeaderMap) -> Self {
        mark_credentials(&mut headers);
        self.headers = headers;
        self.client = OnceLock::new();
        self
    }

    /// Sets the number of retry attempts for failed requests.
    ///
    /// Only failures that another attempt could fix are retried: connection errors, timeouts, a body cut off
    /// mid-transfer, and `5xx`, `408` and `429` responses. Any other `4xx`, or a body that is not the JSON asked for,
    /// is returned at once, since resending the same request would get the same answer.
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Sets the HTTP/2 toggle: `true` forces requests over HTTP/1.x, `false` keeps HTTP/2 enabled.
    pub fn disable_http2(mut self, disable: bool) -> Self {
        self.disable_http2 = disable;
        self.client = OnceLock::new();
        self
    }

    /// Sets the read (idle) timeout applied to every request.
    ///
    /// The timeout is applied to each read operation and **resets after each successful read**, so it bounds how long a
    /// connection may stall without sending data — not the total duration of a request. A slow but steady transfer
    /// (e.g. a large download) never trips it. Pass a [`Duration`] to set it or `None` to disable it. Defaults to 30
    /// seconds.
    ///
    /// ```
    /// use std::time::Duration;
    /// use rust_sak::fetch::Fetch;
    ///
    /// let fetch = Fetch::new().read_timeout(Duration::from_secs(10)); // tighter idle timeout
    /// let patient = Fetch::new().read_timeout(None); // no idle timeout
    /// ```
    pub fn read_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.read_timeout = timeout.into();
        self.client = OnceLock::new();
        self
    }

    /// Sets the bound on establishing a connection — DNS resolution, the TCP handshake and the TLS
    /// handshake — before any bytes are exchanged.
    ///
    /// This is a different guarantee from [`read_timeout`](Fetch::read_timeout), which bounds how long an
    /// *established* connection may stall. A host that accepts nothing at all — a dropped packet filter, a black-holed
    /// address — never reaches a read, so only this setting bounds it. Pass a [`Duration`] to set it or `None` to
    /// leave it unbounded, which is the default.
    ///
    /// ```
    /// use std::time::Duration;
    /// use rust_sak::fetch::Fetch;
    ///
    /// let fetch = Fetch::new().connect_timeout(Duration::from_secs(30));
    /// ```
    pub fn connect_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.connect_timeout = timeout.into();
        self.client = OnceLock::new();
        self
    }

    /// Routes requests through an explicit proxy, overriding whatever the environment or the system settings say.
    ///
    /// Without this, `reqwest`'s own detection applies: the `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` variables on every
    /// platform, plus the system proxy settings on macOS and Windows. Call this only to override that; call
    /// [`Fetch::no_proxy`] to switch it off entirely.
    ///
    /// An invalid proxy URL is not rejected here — it surfaces as the [`reqwest::Error`] inside the first request's
    /// [`FetchError::Http`] or [`DownloadError::Http`], since that is where the client is built.
    ///
    /// ```
    /// use rust_sak::fetch::{Fetch, ProxySettings};
    ///
    /// let fetch = Fetch::new().proxy(ProxySettings::new("http://proxy.example.com:3128"));
    /// ```
    pub fn proxy(mut self, proxy: ProxySettings) -> Self {
        self.proxy = ProxyMode::Explicit(proxy);
        self.client = OnceLock::new();
        self
    }

    /// Connects directly, ignoring every source of proxy configuration including the environment variables.
    ///
    /// This is stronger than setting no proxy: it disables `reqwest`'s detection rather than declining to override it,
    /// so a `HTTPS_PROXY` in the environment is bypassed rather than honoured.
    ///
    /// ```
    /// use rust_sak::fetch::Fetch;
    ///
    /// let direct = Fetch::new().no_proxy();
    /// ```
    pub fn no_proxy(mut self) -> Self {
        self.proxy = ProxyMode::Disabled;
        self.client = OnceLock::new();
        self
    }

    /// Sets the default [`DownloadMode`] for [`Fetch::download`], controlling what happens when a file already exists at
    /// the target path. Defaults to [`DownloadMode::Resume`]. Individual requests can override this via
    /// [`RequestOptions::download_mode`]. Like [`Fetch::retries`], this is not a client-build setting, so it does not
    /// rebuild the cached client.
    ///
    /// ```
    /// use rust_sak::fetch::{DownloadMode, Fetch};
    ///
    /// let fetch = Fetch::new().download_mode(DownloadMode::Overwrite);
    /// ```
    pub fn download_mode(mut self, mode: DownloadMode) -> Self {
        self.download_mode = mode;
        self
    }

    /// Caps the response body the `text` and `json` methods will read. `None` (the default) reads any size.
    ///
    /// Those methods hold the whole body in memory, so without a cap a server that answers with an endless stream —
    /// hostile, or merely broken — can exhaust it. A body over the limit fails with [`FetchError::BodyTooLarge`]: at
    /// once when its `Content-Length` already says so, otherwise as soon as the bytes read pass the limit. It is not
    /// retried, since the same request would get the same body. Overridable per request via
    /// [`RequestOptions::max_body_bytes`]; [`Fetch::download`] streams to disk and ignores it. Like
    /// [`Fetch::retries`], this does not rebuild the cached client.
    ///
    /// ```
    /// use rust_sak::fetch::Fetch;
    ///
    /// let fetch = Fetch::new().max_body_bytes(16 << 20);
    /// ```
    pub fn max_body_bytes(mut self, limit: impl Into<Option<u64>>) -> Self {
        self.max_body_bytes = limit.into();
        self
    }

    /// Returns the cached HTTP client, building it from the current configuration on first use.
    ///
    /// The struct's headers become the client's default headers; the HTTP/2 toggle, the read and connect timeouts
    /// and the proxy mode are applied at build time; and the result is cached for reuse across requests.
    ///
    /// # Errors
    ///
    /// Returns a [`reqwest::Error`] if the client cannot be built.
    fn client(&self) -> Result<&reqwest::Client, reqwest::Error> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }

        let mut builder = reqwest::Client::builder().default_headers(self.headers.clone());
        if self.disable_http2 {
            builder = builder.http1_only();
        }
        if let Some(timeout) = self.read_timeout {
            builder = builder.read_timeout(timeout);
        }
        if let Some(timeout) = self.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        match &self.proxy {
            ProxyMode::Detect => {}
            ProxyMode::Disabled => builder = builder.no_proxy(),
            ProxyMode::Explicit(settings) => builder = builder.proxy(settings.to_reqwest()?),
        }

        let client = builder.build()?;
        Ok(self.client.get_or_init(|| client))
    }

    /// Sends a prepared request with retries and captures the whole response: its status, its headers, and
    /// whatever `extract` makes of its body.
    ///
    /// The one place the request pipeline is written. Everything that accretes here later — a per-attempt timeout, a
    /// span around each attempt — lands once rather than in the public methods. Which failures are worth retrying is
    /// [`is_retryable`]'s call, and a `Retry-After` on a `429` or `503` sets the wait before the next attempt.
    ///
    /// # Errors
    ///
    /// Returns a [`FetchError`] if the request cannot be prepared, every attempt fails, the response carries a
    /// non-success status, or the body is larger than the resolved `max_body_bytes`.
    async fn send_retrying<T, Fut>(
        &self,
        url: impl reqwest::IntoUrl,
        options: RequestOptions,
        extract: impl Fn(reqwest::Response) -> Fut,
    ) -> Result<Response<T>, FetchError>
    where
        Fut: Future<Output = Result<T, reqwest::Error>>,
    {
        let prepared = self.prepare(url, options)?;
        let retry_after = retry::RetryAfter::default();

        retry::with_fibonacci_backoff(prepared.retries, is_retryable, &retry_after, || async {
            let response = prepared.request().send().await?;
            retry_after.record(&response);
            let mut response = response.error_for_status()?;
            let status = response.status();

            let (headers, body) = match prepared.max_body_bytes {
                // Taken before the body, which consumes the response. Moved rather than cloned: the response is
                // dropped either way, so nothing observes the emptied map, and a paginating caller is not copying a
                // header map per page for no one.
                None => {
                    let headers = std::mem::take(response.headers_mut());
                    (headers, extract(response).await?)
                }
                // Cloned here, because the rebuilt response needs its own copy to decode the body the same way.
                Some(limit) => {
                    let headers = response.headers().clone();
                    (headers, extract(read_capped(response, limit).await?).await?)
                }
            };

            Ok(Response { body, status, headers })
        })
        .await
    }

    /// Resolves the per-request settings shared by [`Fetch::text`], [`Fetch::json`], and [`Fetch::download`] against
    /// the reused client.
    ///
    /// The method defaults to `GET`; per-request headers are carried through to be applied at the request level (where
    /// they override the client's default headers per-key); the retry count falls back to the struct's. Automatic
    /// retries are restricted to idempotent methods unless [`RequestOptions::retry_non_idempotent`] opts in, so the
    /// resolved retry count is forced to zero for a non-idempotent method otherwise.
    ///
    /// # Errors
    ///
    /// Returns a [`reqwest::Error`] if the client cannot be built or `url` is invalid.
    fn prepare(&self, url: impl reqwest::IntoUrl, options: RequestOptions) -> Result<PreparedRequest, reqwest::Error> {
        let client = self.client()?.clone();
        let method = options.method.unwrap_or(reqwest::Method::GET);

        let retries = options.retries.unwrap_or(self.retries);
        let retries = if is_idempotent(&method) || options.retry_non_idempotent {
            retries
        } else {
            0
        };

        Ok(PreparedRequest {
            client,
            url: url.into_url()?,
            method,
            query: options.query,
            headers: options.headers,
            body: options.body,
            retries,
            max_body_bytes: options.max_body_bytes.or(self.max_body_bytes),
        })
    }

    /// Sends a `GET` request to `url` with default per-request options and returns the response body as a `String`.
    ///
    /// The [`Fetch`] struct's own configuration (headers, retry count, HTTP/2 setting) still applies. Use
    /// [`Fetch::text_with_options`] to supply per-request overrides.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body is not
    /// valid UTF-8 text, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::Fetch;
    ///
    /// let body = Fetch::new().text("https://example.com").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn text(&self, url: impl reqwest::IntoUrl) -> Result<String, FetchError> {
        self.text_with_options(url, RequestOptions::default()).await
    }

    /// Sends a request to `url` and returns the response body as a `String`.
    ///
    /// The struct's headers, retry count, and HTTP/2 setting provide the defaults; any field set on `options` takes
    /// priority for this one request. Headers are merged per-key (request values override struct values, other struct
    /// headers are preserved), query parameters from `options` are appended, the method defaults to `GET`, and a JSON
    /// body set via [`RequestOptions::body`] is attached. The request is retried up to the resolved number of
    /// additional times on failure, with Fibonacci backoff between attempts (1s, 2s, 3s, 5s, …).
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body is not
    /// valid UTF-8 text, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::{Fetch, RequestOptions};
    ///
    /// let body = Fetch::new()
    ///     .text_with_options("https://example.com", RequestOptions::new().query("q", "rust"))
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn text_with_options(
        &self,
        url: impl reqwest::IntoUrl,
        options: RequestOptions,
    ) -> Result<String, FetchError> {
        Ok(self.text_response_with_options(url, options).await?.body)
    }

    /// Sends a `GET` request to `url` with default per-request options and deserializes the JSON response body into `T`.
    ///
    /// The [`Fetch`] struct's own configuration (headers, retry count, HTTP/2 setting) still applies. Use
    /// [`Fetch::json_with_options`] to supply per-request overrides.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body cannot be
    /// deserialized into `T`, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::Fetch;
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Repo {
    ///     name: String,
    ///     stargazers_count: u32,
    /// }
    ///
    /// let repo: Repo = Fetch::new()
    ///     .header("Accept", "application/json")
    ///     .json("https://api.github.com/repos/rust-lang/rust")
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn json<T: serde::de::DeserializeOwned>(&self, url: impl reqwest::IntoUrl) -> Result<T, FetchError> {
        self.json_with_options(url, RequestOptions::default()).await
    }

    /// Sends a request to `url` and deserializes the JSON response body into `T`.
    ///
    /// Behaves exactly like [`Fetch::text_with_options`] — same header merging, query parameters, optional
    /// [`RequestOptions::body`], method default, and Fibonacci-backoff retries — but parses the response body as JSON
    /// into any type implementing [`serde::de::DeserializeOwned`] instead of returning the raw text.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body cannot be
    /// deserialized into `T`, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::{Fetch, RequestOptions};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Repo {
    ///     name: String,
    ///     stargazers_count: u32,
    /// }
    ///
    /// let repo: Repo = Fetch::new()
    ///     .header("Accept", "application/json")
    ///     .json_with_options("https://api.github.com/repos/rust-lang/rust", RequestOptions::new())
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn json_with_options<T: serde::de::DeserializeOwned>(
        &self,
        url: impl reqwest::IntoUrl,
        options: RequestOptions,
    ) -> Result<T, FetchError> {
        Ok(self.json_response_with_options(url, options).await?.body)
    }

    /// Sends a `GET` request to `url` with default per-request options and returns the whole [`Response`] — the body
    /// as a `String`, plus its status and headers.
    ///
    /// Use this where the body is not the whole answer: a pagination cursor in a `Link` header, a checksum in an
    /// `ETag`, a rate limit to read before the next request. [`Fetch::text`] is the same request reduced to its body.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body is not
    /// valid UTF-8 text, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::Fetch;
    ///
    /// let fetch = Fetch::new();
    /// let mut url = Some("https://api.example.com/items".to_string());
    ///
    /// // Following the pagination to exhaustion, rather than bounding it with a large `limit`.
    /// while let Some(next) = url {
    ///     let page = fetch.text_response(&next).await?;
    ///     println!("{}", page.body);
    ///     url = page.link("next").map(str::to_string);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn text_response(&self, url: impl reqwest::IntoUrl) -> Result<Response<String>, FetchError> {
        self.text_response_with_options(url, RequestOptions::default()).await
    }

    /// Sends a request to `url` and returns the whole [`Response`] — the body as a `String`, plus its status and
    /// headers.
    ///
    /// Behaves exactly like [`Fetch::text_with_options`] — same header merging, query parameters, optional
    /// [`RequestOptions::body`], method default, and Fibonacci-backoff retries — but hands back the response's
    /// metadata alongside its body.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body is not
    /// valid UTF-8 text, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::{Fetch, RequestOptions};
    ///
    /// let page = Fetch::new()
    ///     .text_response_with_options("https://api.example.com/items", RequestOptions::new().query("limit", "50"))
    ///     .await?;
    ///
    /// assert!(page.status.is_success());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn text_response_with_options(
        &self,
        url: impl reqwest::IntoUrl,
        options: RequestOptions,
    ) -> Result<Response<String>, FetchError> {
        self.send_retrying(url, options, reqwest::Response::text).await
    }

    /// Sends a `GET` request to `url` with default per-request options and returns the whole [`Response`] — the JSON
    /// body deserialized into `T`, plus its status and headers.
    ///
    /// [`Fetch::json`] is the same request reduced to its body; see [`Fetch::text_response`] for when the metadata is
    /// what a caller is after.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body cannot be
    /// deserialized into `T`, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::Fetch;
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Item {
    ///     name: String,
    /// }
    ///
    /// let page = Fetch::new().json_response::<Vec<Item>>("https://api.example.com/items").await?;
    /// println!("{} items, more at {:?}", page.body.len(), page.link("next"));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn json_response<T: serde::de::DeserializeOwned>(
        &self,
        url: impl reqwest::IntoUrl,
    ) -> Result<Response<T>, FetchError> {
        self.json_response_with_options(url, RequestOptions::default()).await
    }

    /// Sends a request to `url` and returns the whole [`Response`] — the JSON body deserialized into `T`, plus its
    /// status and headers.
    ///
    /// Behaves exactly like [`Fetch::json_with_options`], but hands back the response's metadata alongside its body.
    ///
    /// # Errors
    ///
    /// Returns the last [`FetchError`] if the client cannot be built, every attempt fails, the response body cannot be
    /// deserialized into `T`, or it is larger than [`Fetch::max_body_bytes`] allows.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), rust_sak::fetch::FetchError> {
    /// use rust_sak::fetch::{Fetch, RequestOptions};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Item {
    ///     name: String,
    /// }
    ///
    /// let page: rust_sak::fetch::Response<Vec<Item>> = Fetch::new()
    ///     .json_response_with_options("https://api.example.com/items", RequestOptions::new().query("limit", "50"))
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn json_response_with_options<T: serde::de::DeserializeOwned>(
        &self,
        url: impl reqwest::IntoUrl,
        options: RequestOptions,
    ) -> Result<Response<T>, FetchError> {
        self.send_retrying(url, options, reqwest::Response::json::<T>).await
    }

    /// Streams a request to `url`, writing the response body to `path`, and returns a [`Download`] handle immediately.
    ///
    /// Unlike [`Fetch::text`] and [`Fetch::json`], this does **not** await the transfer: the download runs in a
    /// background task while the body is streamed to disk chunk-by-chunk (never buffered whole in memory). The returned
    /// [`Download`] tracks live progress — total size, bytes downloaded, completion fraction — via
    /// [`Download::progress`], and exposes the final outcome via [`Download::completed`], [`Download::failed`], and
    /// [`Download::join`].
    ///
    /// This uses default per-request options; use [`Fetch::download_with_options`] to override the method, headers,
    /// retries, or [`DownloadMode`] for a single transfer. The struct's own configuration still provides the defaults.
    ///
    /// Bytes are written to `<path>.part`, described by a `<path>.part.json` sidecar, and renamed onto `path` only
    /// once the transfer finishes. A file at `path` is therefore complete by construction: an interrupted or cancelled
    /// transfer leaves a partial beside it, never a truncated file at it.
    ///
    /// The resolved [`DownloadMode`] (the struct's [`download_mode`](Fetch::download_mode), overridable via
    /// [`RequestOptions::download_mode`]) decides what already-present bytes mean. [`DownloadMode::Resume`] (the
    /// default) reports an existing file at `path` as complete, and otherwise continues `<path>.part` with an HTTP
    /// `Range` request — but only after its sidecar confirms it belongs to *this* request; a partial recorded against a
    /// different URL or [`resume_key`](RequestOptions::resume_key), or one with no sidecar at all, is discarded rather
    /// than appended to, as is one the server disowns by answering `If-Range` with a `200`.
    /// [`DownloadMode::Overwrite`] discards any partial and downloads from byte zero, replacing whatever is at `path`.
    /// [`DownloadMode::Skip`] reports an existing file at `path` as complete without contacting the server. On a retry
    /// the transfer likewise resumes from whatever bytes are already in the partial rather than restarting.
    ///
    /// All fallible setup is surfaced through the handle rather than from this call: an invalid URL or a client-build
    /// error is captured and reported, alongside a bad HTTP status, a stream error, or a disk-write error, via
    /// [`Download::failed`]/[`Download::join`] as a [`DownloadError`].
    ///
    /// # Panics
    ///
    /// Must be called from within a Tokio runtime (it spawns a task); panics otherwise.
    ///
    /// The simplest way to follow a download to completion is [`Download::track`], which invokes a callback on every
    /// progress update and resolves with the final [`Result`]:
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use rust_sak::fetch::Fetch;
    ///
    /// let mut download = Fetch::new().download("https://example.com/big.bin", "/tmp/big.bin");
    ///
    /// download
    ///     .track(|total, downloaded, progress| {
    ///         match progress {
    ///             Some(fraction) => println!("{:.0}% ({downloaded} bytes)", fraction * 100.0),
    ///             None => println!("{downloaded} bytes (unknown total: {total:?})"),
    ///         }
    ///     })
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// For finer control you can instead poll [`Download::progress`] and await [`Download::changed`] in a loop, then
    /// await the final outcome with [`Download::join`].
    pub fn download(&self, url: impl reqwest::IntoUrl, path: impl AsRef<std::path::Path>) -> Download {
        self.download_with_options(url, path, RequestOptions::default())
    }

    /// Streams a request to `url` into `path` with per-request `options`, returning a [`Download`] handle immediately.
    ///
    /// Behaves exactly like [`Fetch::download`] — same background streaming, progress tracking, and resume semantics —
    /// but applies `options` (method, headers, query, retries, and [`RequestOptions::download_mode`]) on top of the
    /// struct's configuration, per the same override rules as [`Fetch::text_with_options`].
    ///
    /// # Panics
    ///
    /// Must be called from within a Tokio runtime (it spawns a task); panics otherwise.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use rust_sak::fetch::{DownloadMode, Fetch, RequestOptions};
    ///
    /// let download = Fetch::new().download_with_options(
    ///     "https://example.com/big.bin",
    ///     "/tmp/big.bin",
    ///     RequestOptions::new().download_mode(DownloadMode::Overwrite),
    /// );
    /// download.join().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn download_with_options(
        &self,
        url: impl reqwest::IntoUrl,
        path: impl AsRef<std::path::Path>,
        options: RequestOptions,
    ) -> Download {
        let mode = options.download_mode.unwrap_or(self.download_mode);
        let resume = download::Resume {
            key: options.resume_key.clone(),
            across_urls: options.resume_across_urls,
        };
        let algorithm = options.digest;
        let prepared = self.prepare(url, options);
        let path = path.as_ref().to_path_buf();
        let (tx, rx) = tokio::sync::watch::channel(Progress::default());
        // Written by the background task on success and read through the handle, so the digest travels without
        // widening `Progress` — an in-flight snapshot has nothing to say about a hash that is not finished yet.
        let digest = Arc::new(OnceLock::new());
        let handle = tokio::spawn(download::run(
            prepared,
            path,
            tx,
            mode,
            resume,
            algorithm,
            Arc::clone(&digest),
        ));
        Download::from_parts(rx, handle, digest)
    }
}

/// Reports whether a failed `text`/`json` attempt is worth another. A body over the limit will be over it again;
/// everything else is [`retry::is_transient`]'s call.
fn is_retryable(err: &FetchError) -> bool {
    match err {
        FetchError::Http(err) => retry::is_transient(err),
        FetchError::BodyTooLarge { .. } => false,
    }
}

/// Reads `response`'s body, refusing one larger than `limit`, and hands it back as a response of its own.
///
/// The body is read here, under the limit, rather than by `reqwest`'s own `text`/`json`, which read whatever arrives.
/// Rebuilding a `reqwest::Response` around the bytes is what lets those same methods decode them afterwards: the
/// charset from `Content-Type`, the JSON errors, everything a caller already relies on, stays exactly as it was.
///
/// # Errors
///
/// Returns [`FetchError::BodyTooLarge`] when `Content-Length` already exceeds `limit` or the bytes read pass it, and
/// [`FetchError::Http`] if the body stream fails.
async fn read_capped(response: reqwest::Response, limit: u64) -> Result<reqwest::Response, FetchError> {
    use futures_util::StreamExt;

    // Refused before a byte is read when the server has already said how much is coming.
    if response.content_length().is_some_and(|length| length > limit) {
        return Err(FetchError::BodyTooLarge { limit });
    }

    let (status, version, headers) = (response.status(), response.version(), response.headers().clone());

    // The `Content-Length` check is not enough on its own: a chunked body declares nothing, and a decompressed one
    // can be far larger than the length the server declared.
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(FetchError::BodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }

    let mut rebuilt = http::Response::new(body);
    *rebuilt.status_mut() = status;
    *rebuilt.version_mut() = version;
    *rebuilt.headers_mut() = headers;

    Ok(reqwest::Response::from(rebuilt))
}

/// Inserts `key`/`value` into `map`, converting both and panicking on invalid input. Shared by the `header` builder
/// methods on [`Fetch`] and [`RequestOptions`].
///
/// # Panics
///
/// Panics if `key` is not a valid header name or `value` is not a valid header value.
fn insert_header<K, V>(map: &mut HeaderMap, key: K, value: V)
where
    K: TryInto<reqwest::header::HeaderName>,
    K::Error: std::fmt::Debug,
    V: TryInto<reqwest::header::HeaderValue>,
    V::Error: std::fmt::Debug,
{
    let key = key.try_into().expect("invalid header name");
    let mut value: reqwest::header::HeaderValue = value.try_into().expect("invalid header value");
    value.set_sensitive(is_credential(&key));
    map.insert(key, value);
}

/// Marks every credential-bearing value in `map` as sensitive. Applied to a header map handed over whole, where
/// [`insert_header`] never saw the values go in.
///
/// A sensitive value prints as `Sensitive` in a [`HeaderMap`]'s `Debug` output — which [`Fetch`] and
/// [`RequestOptions`] both derive — and is kept out of HTTP/2's header-compression table.
fn mark_credentials(map: &mut HeaderMap) {
    for (name, value) in map.iter_mut() {
        if is_credential(name) {
            value.set_sensitive(true);
        }
    }
}

/// `true` for a header whose value is a credential: the standard authentication headers, plus the conventional
/// names APIs give their keys and tokens.
fn is_credential(name: &reqwest::header::HeaderName) -> bool {
    use reqwest::header::{AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION};

    if [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE].contains(name) {
        return true;
    }

    // Header names are stored lowercase, so the comparison needs no folding.
    let name = name.as_str();
    ["token", "secret", "password", "api-key", "apikey"]
        .iter()
        .any(|word| name.contains(word))
}

/// `true` for HTTP methods that are idempotent per RFC 9110 (so safe to retry automatically): `GET`, `HEAD`, `PUT`,
/// `DELETE`, `OPTIONS`, `TRACE`. `POST` and `PATCH` are not.
fn is_idempotent(method: &reqwest::Method) -> bool {
    use reqwest::Method;
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}
