//! Configuration for [`Telemetry`](super::Telemetry).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry_otlp::{LogExporter, Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;

use super::enrichment::Enrichment;
use super::geolocation::{self, fetch_geolocation_from};
use super::{Environment, O11yError, Result, Telemetry};

/// The `deployment.environment.name` attribute, from OpenTelemetry semantic conventions v1.27.
const DEPLOYMENT_ENVIRONMENT: &str = "deployment.environment.name";

/// A fluent, consuming builder for [`Telemetry`](super::Telemetry), created by
/// [`Telemetry::builder`](super::Telemetry::builder).
///
/// Only the collector endpoint and the service name are required; everything else has a default. The two privacy
/// switches — [`enabled`](TelemetryBuilder::enabled) and [`geolocation`](TelemetryBuilder::geolocation) — are
/// independent, and **both** must be on before any location lookup happens.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::{Environment, Telemetry};
///
/// let telemetry = Telemetry::builder("https://collector.example.com", "my-app")
///     .version(env!("CARGO_PKG_VERSION"))
///     .environment(Environment::Production)
///     .header("Authorization", "Bearer secret")
///     .enabled(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct TelemetryBuilder {
    /// Base URL of the OTLP collector. `/v1/logs` is appended to it.
    endpoint: String,
    /// Reported as `service.name`, and used to scope the machine identifier.
    service_name: String,
    /// Reported as the `version` attribute on every record.
    version: String,
    /// Reported as `deployment.environment.name`.
    environment: Environment,
    /// Extra headers for the OTLP exporter, typically authentication.
    headers: HashMap<String, String>,
    /// Whether to collect and export anything at all.
    enabled: bool,
    /// Whether to enrich records with an IP-based location lookup.
    geolocation: bool,
    /// Base URL of the geolocation service.
    geolocation_url: String,
    /// Export timeout. `None` leaves the exporter default in place.
    timeout: Option<Duration>,
}

impl TelemetryBuilder {
    /// Starts a builder for a collector at `endpoint`, reporting as `service_name`.
    pub(super) fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            version: String::new(),
            environment: Environment::Development,
            headers: HashMap::new(),
            enabled: true,
            geolocation: false,
            geolocation_url: geolocation::DEFAULT_URL.to_string(),
            timeout: None,
        }
    }

    /// Sets the application version reported with every record. Defaults to empty.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Sets the deployment environment. Defaults to [`Environment::Development`].
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    /// Adds a single header to every OTLP export request. Call repeatedly to add more.
    ///
    /// Invalid header names or values are reported by [`build`](TelemetryBuilder::build), not by this method.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Adds several headers at once, merged over any already set.
    pub fn headers<I, K, V>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.headers
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Turns collection on or off. Defaults to `true`.
    ///
    /// When `false`, **nothing leaves the process**: no exporter is installed, no geolocation lookup is made, and
    /// every record is discarded. The handle stays fully usable, so a caller can wire telemetry in unconditionally
    /// and let a single flag decide whether it does anything.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Turns IP-based location enrichment on or off. Defaults to **`false`**.
    ///
    /// This is the one enrichment that discloses information to a third party — the lookup necessarily reveals the
    /// machine's public IP address — so it is opt-in separately from [`enabled`](TelemetryBuilder::enabled), and both
    /// must be on before any request is made.
    ///
    /// When on, the lookup runs on a background thread and its `location.country`/`location.region`/`location.city`
    /// attributes are merged in once it answers, so [`build`](TelemetryBuilder::build) never blocks on it. Records
    /// emitted before it lands simply carry no location.
    pub fn geolocation(mut self, geolocation: bool) -> Self {
        self.geolocation = geolocation;
        self
    }

    /// Overrides the geolocation service. Defaults to `https://ipinfo.io`.
    ///
    /// The lookup requests `<base_url>/json`. Useful for a self-hosted or proxied endpoint.
    pub fn geolocation_url(mut self, base_url: impl Into<String>) -> Self {
        self.geolocation_url = base_url.into();
        self
    }

    /// Sets how long a single export attempt may take. Defaults to the OTLP exporter's own timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Builds the [`Telemetry`](super::Telemetry) handle.
    ///
    /// The enrichment that can be read locally — version, service-scoped machine id, OS, architecture and a fresh
    /// session id — is gathered here regardless of whether collection is enabled, since none of it leaves the
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`O11yError::InvalidEndpoint`] if the endpoint is not a valid URL, [`O11yError::InvalidHeader`] if a
    /// header cannot be sent over HTTP, or [`O11yError::Exporter`] if the OTLP exporter cannot be constructed. To
    /// fall back to a discarding handle instead of propagating, use
    /// [`Telemetry::disabled`](super::Telemetry::disabled):
    ///
    /// ```no_run
    /// use rust_sak::o11y::Telemetry;
    ///
    /// let telemetry = Telemetry::builder("https://collector.example.com", "my-app")
    ///     .build()
    ///     .unwrap_or_else(|_| Telemetry::disabled());
    /// ```
    pub fn build(self) -> Result<Telemetry> {
        let enrichment = Arc::new(Enrichment::new(&self.version, &self.service_name));

        if !self.enabled {
            return Ok(Telemetry::from_parts(None, enrichment));
        }

        validate_headers(&self.headers)?;

        let mut exporter = LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(logs_endpoint(&self.endpoint)?);

        if !self.headers.is_empty() {
            exporter = exporter.with_headers(self.headers);
        }
        if let Some(timeout) = self.timeout {
            exporter = exporter.with_timeout(timeout);
        }

        let resource = Resource::builder()
            .with_service_name(self.service_name.clone())
            .with_attribute(KeyValue::new(DEPLOYMENT_ENVIRONMENT, self.environment.to_string()))
            .build();

        let provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter.build()?)
            .build();

        // The single call site for the location lookup: unreachable unless collection is on *and* it was opted into.
        if self.geolocation {
            let enrichment = Arc::clone(&enrichment);
            let url = self.geolocation_url;
            std::thread::spawn(move || {
                if let Ok(geo) = fetch_geolocation_from(&url) {
                    enrichment.set_location(&geo);
                }
            });
        }

        Ok(Telemetry::from_parts(Some(provider), enrichment))
    }
}

/// Appends the OTLP logs signal path to `base` and checks the result is a usable URL.
///
/// The exporter uses a programmatically supplied endpoint **verbatim** — it only appends `/v1/logs` to endpoints
/// taken from `OTEL_EXPORTER_OTLP_*` environment variables — so the path has to be joined here.
fn logs_endpoint(base: &str) -> Result<String> {
    let endpoint = format!("{}/v1/logs", base.trim_end_matches('/'));

    reqwest::Url::parse(&endpoint).map_err(|err| O11yError::InvalidEndpoint {
        endpoint: base.to_string(),
        reason: err.to_string(),
    })?;

    Ok(endpoint)
}

/// Rejects headers that cannot be put on an HTTP request.
///
/// The exporter silently drops such headers, which would leave a misspelled auth header looking like a server-side
/// authentication failure much later; failing at build time points straight at the cause.
fn validate_headers(headers: &HashMap<String, String>) -> Result<()> {
    for (name, value) in headers {
        let valid = reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok()
            && reqwest::header::HeaderValue::from_str(value).is_ok();

        if !valid {
            return Err(O11yError::InvalidHeader { name: name.clone() });
        }
    }

    Ok(())
}
