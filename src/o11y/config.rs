use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use super::geolocation;
use super::{Environment, Level, O11yError, Value};

/// An empty header set, for a collector that authenticates some other way.
///
/// `Config::builder(url, [])` cannot infer the element type of an empty array, so this names it:
///
/// ```
/// use rust_sak::o11y::{Config, NO_HEADERS};
///
/// let config = Config::builder("https://collector.example.com", NO_HEADERS).build();
/// ```
pub const NO_HEADERS: [(&str, &str); 0] = [];

/// The callback invoked when a batch could not be delivered.
pub(super) type ExportErrorHandler = Arc<dyn Fn(&O11yError) + Send + Sync + 'static>;

/// How long a single export attempt may take, when not overridden.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the worker exports, when not overridden.
const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// How many records force an early export, when not overridden.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;

/// How many records each buffer holds before discarding the oldest, when not overridden.
const DEFAULT_MAX_BUFFERED: usize = 8_192;

/// A finished configuration, ready to be handed to [`init`](super::init).
///
/// Build one with [`Config::builder`].
#[derive(Clone)]
pub struct Config {
    /// Base URL of the collector. The signal paths are appended to it.
    pub(super) endpoint: String,
    /// Headers put on every export request, typically authentication.
    pub(super) headers: HashMap<String, String>,
    /// Reported as `service.name`, and used to scope the machine identifier.
    pub(super) service_name: String,
    /// Reported as `service.version`.
    pub(super) service_version: String,
    /// Reported as `deployment.environment.name`.
    pub(super) environment: Environment,
    /// Resource attributes the caller added, reported after the built-in ones.
    pub(super) resource: Vec<(Cow<'static, str>, Value)>,
    /// The lowest severity that is recorded.
    pub(super) min_level: Level,
    /// How often the worker exports whatever has accumulated.
    pub(super) flush_interval: Duration,
    /// How many buffered records trigger an export before the interval elapses.
    pub(super) max_batch_size: usize,
    /// How many records each buffer holds before discarding the oldest.
    pub(super) max_buffered: usize,
    /// How long a single export attempt may take.
    pub(super) timeout: Duration,
    /// Whether to collect and export anything at all.
    pub(super) enabled: bool,
    /// Whether to enrich records with an IP-based location lookup.
    pub(super) geolocation: bool,
    /// Base URL of the geolocation service.
    pub(super) geolocation_url: String,
    /// How long the geolocation lookup may take.
    pub(super) geolocation_timeout: Duration,
    /// Where export failures are reported.
    pub(super) on_export_error: ExportErrorHandler,
}

impl Config {
    /// Starts configuring telemetry reporting to the collector at `endpoint`.
    ///
    /// `endpoint` is the collector's **base URL**; `/v1/logs`, `/v1/metrics` and `/v1/traces` are appended to it.
    /// `headers` go on every export request — this is where a collector's authentication belongs. Pass
    /// [`NO_HEADERS`] when there is none.
    ///
    /// ```
    /// use rust_sak::o11y::{Config, Environment};
    ///
    /// let config = Config::builder("https://collector.example.com", [("Authorization", "Bearer secret")])
    ///     .service_name("checkout-service")
    ///     .environment(Environment::Production)
    ///     .build();
    /// ```
    pub fn builder<I, K, V>(endpoint: impl Into<String>, headers: I) -> ConfigBuilder
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        ConfigBuilder {
            config: Config {
                endpoint: endpoint.into(),
                headers: headers
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                service_name: String::new(),
                service_version: String::new(),
                environment: Environment::Development,
                resource: Vec::new(),
                min_level: Level::Info,
                flush_interval: DEFAULT_FLUSH_INTERVAL,
                max_batch_size: DEFAULT_MAX_BATCH_SIZE,
                max_buffered: DEFAULT_MAX_BUFFERED,
                timeout: DEFAULT_TIMEOUT,
                enabled: true,
                geolocation: false,
                geolocation_url: geolocation::DEFAULT_URL.to_string(),
                geolocation_timeout: geolocation::DEFAULT_TIMEOUT,
                // Silence by default: a library has no business writing to a host application's stderr uninvited.
                on_export_error: Arc::new(|_| {}),
            },
        }
    }
}

// Hand-written because the error callback is a trait object with no `Debug` bound, and requiring one would rule
// out the closures this is meant to be called with.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("endpoint", &self.endpoint)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("service_name", &self.service_name)
            .field("service_version", &self.service_version)
            .field("environment", &self.environment)
            .field("resource", &self.resource)
            .field("min_level", &self.min_level)
            .field("flush_interval", &self.flush_interval)
            .field("max_batch_size", &self.max_batch_size)
            .field("max_buffered", &self.max_buffered)
            .field("timeout", &self.timeout)
            .field("enabled", &self.enabled)
            .field("geolocation", &self.geolocation)
            .field("geolocation_url", &self.geolocation_url)
            .field("geolocation_timeout", &self.geolocation_timeout)
            .finish_non_exhaustive()
    }
}

/// A fluent, consuming builder for [`Config`], created by [`Config::builder`].
///
/// Only the endpoint is required; everything else has a default. The two privacy switches —
/// [`enabled`](ConfigBuilder::enabled) and [`geolocation`](ConfigBuilder::geolocation) — are independent, and
/// **both** must be on before any location lookup happens.
///
/// ```
/// use std::time::Duration;
/// use rust_sak::o11y::{Config, Environment};
///
/// let config = Config::builder("https://collector.example.com", [("Authorization", "Bearer secret")])
///     .service_name("checkout-service")
///     .service_version(env!("CARGO_PKG_VERSION"))
///     .environment(Environment::Production)
///     .flush_interval(Duration::from_secs(5))
///     .max_batch_size(512)
///     .on_export_error(|e| eprintln!("telemetry export failed: {e}"))
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct ConfigBuilder {
    /// The configuration accumulated so far.
    config: Config,
}

impl ConfigBuilder {
    /// Sets the service name, reported as the `service.name` resource attribute.
    ///
    /// It also scopes the machine identifier, so two services on one machine report different ids.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.config.service_name = name.into();
        self
    }

    /// Sets the application version, reported as `service.version`. Defaults to empty.
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.config.service_version = version.into();
        self
    }

    /// Sets the deployment environment. Defaults to [`Environment::Development`].
    pub fn environment(mut self, environment: Environment) -> Self {
        self.config.environment = environment;
        self
    }

    /// Adds a resource attribute, reported on every batch next to `service.name` and the machine enrichment.
    ///
    /// For facts that hold for the whole process — the hardware it runs on, a build flavour — and so belong once per
    /// batch rather than on every record. A key the module sets itself (`service.*`, `deployment.environment.name`,
    /// `telemetry.sdk.*`, `machine.*`, `session.id`, `location.*`) is ignored rather than allowed to shadow it, and a
    /// key added twice keeps its last value.
    pub fn resource_attribute(mut self, key: impl Into<Cow<'static, str>>, value: impl Into<Value>) -> Self {
        let key = key.into();
        let value = value.into();

        match self.config.resource.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = value,
            None => self.config.resource.push((key, value)),
        }

        self
    }

    /// Sets the lowest severity that is recorded. Defaults to [`Level::Info`].
    ///
    /// Records below it cost one atomic load and a branch, and their field expressions are never evaluated.
    pub fn min_level(mut self, level: Level) -> Self {
        self.config.min_level = level;
        self
    }

    /// Adds one more header to every export request.
    ///
    /// Invalid header names or values are reported by [`init`](super::init), not by this method.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.headers.insert(key.into(), value.into());
        self
    }

    /// Sets how often the worker exports whatever has accumulated. Defaults to five seconds.
    pub fn flush_interval(mut self, interval: Duration) -> Self {
        self.config.flush_interval = interval;
        self
    }

    /// Sets how many buffered records trigger an export before the interval elapses. Defaults to 512.
    pub fn max_batch_size(mut self, size: usize) -> Self {
        self.config.max_batch_size = size;
        self
    }

    /// Sets how many records each buffer holds before discarding the oldest. Defaults to 8192.
    ///
    /// This is the ceiling on what a stalled collector can cost in memory. Raising it buys tolerance for longer
    /// outages at the price of a larger resident set; lowering it does the reverse. Discarded records are reported
    /// through [`on_export_error`](ConfigBuilder::on_export_error) as [`O11yError::Dropped`].
    pub fn max_buffered(mut self, records: usize) -> Self {
        self.config.max_buffered = records;
        self
    }

    /// Sets how long a single export attempt may take. Defaults to ten seconds.
    ///
    /// It also bounds how long [`shutdown`](super::shutdown) can block, since that waits for one final export.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Turns collection on or off. Defaults to `true`.
    ///
    /// When `false`, **nothing leaves the process**: no worker thread is started, no geolocation lookup is made,
    /// and every record is discarded at the level gate. The rest of the API stays callable, so a caller can wire
    /// telemetry in unconditionally and let a single flag decide whether it does anything.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    /// Turns IP-based location enrichment on or off. Defaults to **`false`**.
    ///
    /// This is the one enrichment that discloses information to a third party — the lookup necessarily reveals the
    /// machine's public IP address — so it is opt-in separately from [`enabled`](ConfigBuilder::enabled), and both
    /// must be on before any request is made.
    pub fn geolocation(mut self, geolocation: bool) -> Self {
        self.config.geolocation = geolocation;
        self
    }

    /// Overrides the geolocation service. Defaults to `https://ipinfo.io`.
    pub fn geolocation_url(mut self, base_url: impl Into<String>) -> Self {
        self.config.geolocation_url = base_url.into();
        self
    }

    /// Sets how long the geolocation lookup may take. Defaults to one second.
    pub fn geolocation_timeout(mut self, timeout: Duration) -> Self {
        self.config.geolocation_timeout = timeout;
        self
    }

    /// Sets where export failures are reported. Defaults to discarding them.
    ///
    /// The callback runs on the export thread, long after the call that produced the data returned, and is the only
    /// way to learn that telemetry is not arriving — recording itself never fails. It fires on a failed request, on
    /// a non-2xx response, and when a full buffer discarded records.
    ///
    /// It must not panic: a panic there kills the export thread, after which the module silences itself rather than
    /// letting the buffers fill.
    pub fn on_export_error(mut self, handler: impl Fn(&O11yError) + Send + Sync + 'static) -> Self {
        self.config.on_export_error = Arc::new(handler);
        self
    }

    /// Finishes the configuration.
    ///
    /// This performs no validation and cannot fail; a bad endpoint or an unsendable header is reported by
    /// [`init`](super::init).
    #[must_use = "a Config does nothing until it is passed to init"]
    pub fn build(self) -> Config {
        self.config
    }
}
