use super::Signal;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, O11yError>;

/// A configuration or export failure.
///
/// The variants fall into two groups. [`AlreadyInitialized`](O11yError::AlreadyInitialized),
/// [`InvalidEndpoint`](O11yError::InvalidEndpoint) and [`InvalidHeader`](O11yError::InvalidHeader) are returned by
/// [`init`](super::init) and mean the module never started. The rest are handed to the
/// [`on_export_error`](super::ConfigBuilder::on_export_error) callback from the export thread, long after the call
/// that produced the data returned.
///
/// **Recording telemetry never fails.** The log macros, the metric instruments and the span guards all return
/// nothing: an uninitialised module, a disabled one and an unreachable collector are all indistinguishable at the
/// call site, by design.
#[derive(Debug, thiserror::Error)]
pub enum O11yError {
    /// [`init`](super::init) was called more than once in the same process.
    ///
    /// The first call wins and stays in effect; the module is not reconfigured.
    #[error("o11y has already been initialised")]
    AlreadyInitialized,
    /// The collector endpoint could not be parsed as a URL.
    #[error("invalid collector endpoint {endpoint:?}: {reason}")]
    InvalidEndpoint {
        /// The endpoint as supplied by the caller.
        endpoint: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A header name or value supplied to the builder was not valid for an HTTP request.
    #[error("invalid exporter header {name:?}")]
    InvalidHeader {
        /// The offending header name.
        name: String,
    },
    /// The collector answered, but with a status outside the 2xx range.
    #[error("collector rejected the {signal} payload with status {status}")]
    ExportRejected {
        /// Which stream was rejected.
        signal: Signal,
        /// The HTTP status code returned.
        status: u16,
        /// The response body, truncated to something loggable.
        body: String,
    },
    /// Records were discarded because the buffer was full before the next flush.
    ///
    /// The oldest records are the ones dropped, so what survives is the most recent picture. A steady trickle of
    /// these means `max_buffered` is too small for the emit rate, or the collector is too slow.
    #[error("dropped {count} buffered {signal} record(s) on overflow")]
    Dropped {
        /// Which stream overflowed.
        signal: Signal,
        /// How many records were discarded since the last report.
        count: u64,
    },
    /// The export request or the geolocation request failed outright.
    #[error("telemetry request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Reading a response body failed.
    #[error("reading a telemetry response failed: {0}")]
    Io(#[from] std::io::Error),
    /// A payload could not be serialised, or a geolocation response was not the expected JSON.
    #[error("telemetry payload was not valid json: {0}")]
    Json(#[from] serde_json::Error),
}
