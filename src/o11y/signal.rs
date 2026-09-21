use std::fmt;

/// Which of the three telemetry streams a payload belongs to.
///
/// Each has its own OTLP path, its own buffer and its own export request, so a failure in one never stops the other
/// two. It appears in [`O11yError`](super::O11yError) so an `on_export_error` callback can tell which stream was
/// affected without parsing the message.
///
/// ```
/// use rust_sak::o11y::Signal;
///
/// assert_eq!(Signal::Logs.to_string(), "logs");
/// assert_eq!(Signal::Traces.path(), "/v1/traces");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// Log records.
    Logs,
    /// Metric data points.
    Metrics,
    /// Trace spans.
    Traces,
}

impl Signal {
    /// The OTLP path this signal is posted to, appended to the configured endpoint.
    pub fn path(self) -> &'static str {
        match self {
            Signal::Logs => "/v1/logs",
            Signal::Metrics => "/v1/metrics",
            Signal::Traces => "/v1/traces",
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::Logs => f.write_str("logs"),
            Signal::Metrics => f.write_str("metrics"),
            Signal::Traces => f.write_str("traces"),
        }
    }
}
