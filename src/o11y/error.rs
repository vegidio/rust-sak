use std::fmt;

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
#[derive(Debug)]
pub enum O11yError {
    /// The OTLP log exporter could not be built from the supplied configuration.
    Exporter(opentelemetry_otlp::ExporterBuildError),
    /// Flushing or shutting the logger provider down failed.
    Sdk(opentelemetry_sdk::error::OTelSdkError),
    /// The geolocation request failed or returned an error status.
    Http(reqwest::Error),
    /// Reading the geolocation response body failed.
    Io(std::io::Error),
    /// The geolocation response was not the expected JSON.
    Json(serde_json::Error),
    /// The collector endpoint could not be parsed as a URL.
    InvalidEndpoint {
        /// The endpoint as supplied by the caller.
        endpoint: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A header name or value supplied to the builder was not valid for an HTTP request.
    InvalidHeader {
        /// The offending header name.
        name: String,
    },
}

impl fmt::Display for O11yError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            O11yError::Exporter(err) => write!(f, "otlp exporter could not be built: {err}"),
            O11yError::Sdk(err) => write!(f, "telemetry sdk error: {err}"),
            O11yError::Http(err) => write!(f, "geolocation request failed: {err}"),
            O11yError::Io(err) => write!(f, "reading the geolocation response failed: {err}"),
            O11yError::Json(err) => write!(f, "geolocation response was not valid json: {err}"),
            O11yError::InvalidEndpoint { endpoint, reason } => {
                write!(f, "invalid collector endpoint {endpoint:?}: {reason}")
            }
            O11yError::InvalidHeader { name } => write!(f, "invalid exporter header {name:?}"),
        }
    }
}

impl std::error::Error for O11yError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            O11yError::Exporter(err) => Some(err),
            O11yError::Sdk(err) => Some(err),
            O11yError::Http(err) => Some(err),
            O11yError::Io(err) => Some(err),
            O11yError::Json(err) => Some(err),
            O11yError::InvalidEndpoint { .. } | O11yError::InvalidHeader { .. } => None,
        }
    }
}

impl From<opentelemetry_otlp::ExporterBuildError> for O11yError {
    fn from(err: opentelemetry_otlp::ExporterBuildError) -> Self {
        O11yError::Exporter(err)
    }
}

impl From<opentelemetry_sdk::error::OTelSdkError> for O11yError {
    fn from(err: opentelemetry_sdk::error::OTelSdkError) -> Self {
        O11yError::Sdk(err)
    }
}

impl From<reqwest::Error> for O11yError {
    fn from(err: reqwest::Error) -> Self {
        O11yError::Http(err)
    }
}

impl From<std::io::Error> for O11yError {
    fn from(err: std::io::Error) -> Self {
        O11yError::Io(err)
    }
}

impl From<serde_json::Error> for O11yError {
    fn from(err: serde_json::Error) -> Self {
        O11yError::Json(err)
    }
}
