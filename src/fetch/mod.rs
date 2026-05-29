//! HTTP fetching utilities.
//!
//! This module exposes [`Fetch`], a configurable HTTP request builder. Today it only holds configuration (headers,
//! retries, HTTP/2 toggle); the request-sending logic is added in a later iteration.

use reqwest::header::HeaderMap;

/// A configurable HTTP fetcher, built with a fluent (consuming) builder API.
///
/// ```
/// use rust_sak::fetch::Fetch;
///
/// let fetch = Fetch::new()
///     .header("Accept", "application/json")
///     .retries(3)
///     .disable_http2();
/// ```
#[derive(Debug, Clone, Default)]
pub struct Fetch {
    /// Headers sent with every request.
    headers: HeaderMap,
    /// Number of times a failed request is retried.
    retries: u32,
    /// When `true`, requests are forced over HTTP/1.x instead of HTTP/2.
    disable_http2: bool,
}

impl Fetch {
    /// Creates a new [`Fetch`] with the default configuration: no headers, no retries, and HTTP/2 enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a single header.
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
        let key = key.try_into().expect("invalid header name");
        let value = value.try_into().expect("invalid header value");
        self.headers.insert(key, value);
        self
    }

    /// Replaces the full set of headers.
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Sets the number of retry attempts for failed requests.
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Forces requests over HTTP/1.x, disabling HTTP/2.
    pub fn disable_http2(mut self) -> Self {
        self.disable_http2 = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_defaults() {
        let fetch = Fetch::new();
        assert!(fetch.headers.is_empty());
        assert_eq!(fetch.retries, 0);
        assert!(!fetch.disable_http2);
    }

    #[test]
    fn builder_sets_fields() {
        let fetch = Fetch::new()
            .header("Accept", "application/json")
            .retries(3)
            .disable_http2();

        assert_eq!(fetch.headers.get("Accept").unwrap(), "application/json");
        assert_eq!(fetch.retries, 3);
        assert!(fetch.disable_http2);
    }

    #[test]
    fn headers_replaces_map() {
        let mut map = HeaderMap::new();
        map.insert("X-Test", "1".parse().unwrap());

        let fetch = Fetch::new().headers(map);
        assert_eq!(fetch.headers.get("X-Test").unwrap(), "1");
    }
}
