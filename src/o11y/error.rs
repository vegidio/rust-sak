/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, O11yError>;

/// An error produced while building or shutting down a [`Telemetry`](super::Telemetry).
///
/// Failures from the OpenTelemetry stack are wrapped per source: the OTLP exporter builder
/// ([`O11yError::Exporter`]) and the SDK's flush/shutdown path ([`O11yError::Sdk`]). The remaining variants cover this
/// module's own configuration validation and the optional geolocation lookup.
///
/// Note that **emitting a record never fails**: the log methods on [`Telemetry`](super::Telemetry) return nothing, and
/// a disabled or misconfigured handle simply discards.
#[derive(Debug, thiserror::Error)]
pub enum O11yError {
    /// The OTLP log exporter could not be built from the supplied configuration.
    #[error("otlp exporter could not be built: {0}")]
    Exporter(#[from] opentelemetry_otlp::ExporterBuildError),
    /// Flushing or shutting the logger provider down failed.
    #[error("telemetry sdk error: {0}")]
    Sdk(#[from] opentelemetry_sdk::error::OTelSdkError),
    /// The geolocation request failed or returned an error status.
    #[error("geolocation request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Reading the geolocation response body failed.
    #[error("reading the geolocation response failed: {0}")]
    Io(#[from] std::io::Error),
    /// The geolocation response was not the expected JSON.
    #[error("geolocation response was not valid json: {0}")]
    Json(#[from] serde_json::Error),
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
}
