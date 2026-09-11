//! The resolved per-request bundle produced by [`Fetch::prepare`](super::Fetch) and consumed by the request methods.

use reqwest::header::HeaderMap;

/// Client and resolved per-request settings produced by [`Fetch::prepare`](super::Fetch), consumed by the request
/// methods. The embedded [`reqwest::Client`] is a cheap clone of the reused client (it shares the underlying
/// connection pool).
pub(super) struct PreparedRequest {
    pub(super) client: reqwest::Client,
    pub(super) url: reqwest::Url,
    pub(super) method: reqwest::Method,
    pub(super) query: Vec<(String, String)>,
    /// Per-request headers, applied at the request level so they override the client's default headers per-key.
    pub(super) headers: HeaderMap,
    pub(super) body: Option<serde_json::Value>,
    pub(super) retries: u32,
}

impl PreparedRequest {
    /// Assembles the [`reqwest::RequestBuilder`] for one attempt: method, query parameters, per-request header
    /// overrides, and the optional JSON body. Shared by [`Fetch::text`](super::Fetch::text),
    /// [`Fetch::json`](super::Fetch::json), and the download task so the request is built identically everywhere.
    pub(super) fn request(&self) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(self.method.clone(), self.url.clone())
            .query(&self.query)
            .headers(self.headers.clone());
        if let Some(body) = &self.body {
            request = request.json(body);
        }
        request
    }

    /// The fully resolved URL this request would be sent to, query parameters included.
    ///
    /// Built through [`PreparedRequest::request`] rather than by re-deriving the query string, so what the
    /// partial-file sidecar records as "where these bytes came from" is exactly what the transfer asks for. Falls
    /// back to the bare URL if the builder cannot be finalized, which leaves a weaker identity rather than failing a
    /// download over bookkeeping.
    pub(super) fn effective_url(&self) -> String {
        match self.request().build() {
            Ok(request) => request.url().to_string(),
            Err(_) => self.url.to_string(),
        }
    }
}
