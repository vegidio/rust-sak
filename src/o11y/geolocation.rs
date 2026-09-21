//! IP-based location enrichment.
//!
//! This is the only part of the module that reaches out to a third party, and it is **opt-in**: a
//! [`Config`](super::Config) performs no lookup unless
//! [`ConfigBuilder::geolocation(true)`](super::ConfigBuilder::geolocation) was set.

use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use serde::Deserialize;

use super::Result;

/// How long the lookup may take before it is abandoned, unless the caller says otherwise. Enrichment is never worth
/// stalling on, but a self-hosted endpoint behind a VPN can legitimately need longer — see
/// [`fetch_geolocation_with`].
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

/// The HTTP client every lookup shares.
///
/// A blocking `reqwest::Client` is expensive to build: it loads the TLS root store and starts a tokio runtime with
/// its own thread, all to make one sub-kilobyte GET. Building one per lookup meant paying that every time, so the
/// process builds one and reuses it. It is module-private, so nothing outside can reconfigure it — which is the
/// objection to `reqwest`'s process-wide default client, not to a static of our own.
///
/// It carries no timeout of its own; each request sets its own, which is what lets callers choose.
static CLIENT: LazyLock<Option<reqwest::blocking::Client>> =
    LazyLock::new(|| reqwest::blocking::Client::builder().build().ok());

/// Caps the response body. The service answers in well under a kilobyte, so anything larger is either a
/// misconfigured endpoint or a hostile one.
const MAX_RESPONSE: u64 = 64 * 1024;

/// The default lookup service.
pub(super) const DEFAULT_URL: &str = "https://ipinfo.io";

/// Location details for the machine's public IP address, as reported by [ipinfo.io](https://ipinfo.io).
///
/// Every field is optional: the service omits what it cannot determine, and a self-hosted endpoint may return less.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Geolocation {
    /// The public IP address the lookup resolved from.
    pub ip: Option<String>,
    /// Reverse-DNS hostname for the IP.
    pub hostname: Option<String>,
    /// City name.
    pub city: Option<String>,
    /// Region or state name.
    pub region: Option<String>,
    /// ISO 3166-1 alpha-2 country code.
    pub country: Option<String>,
    /// `"latitude,longitude"`.
    pub loc: Option<String>,
    /// The owning organization, usually the ISP.
    pub org: Option<String>,
    /// Postal code.
    pub postal: Option<String>,
    /// IANA timezone name.
    pub timezone: Option<String>,
}

/// Looks up the location of this machine's public IP address via [ipinfo.io](https://ipinfo.io).
///
/// > **Privacy:** this call discloses the caller's public IP address to a third party. A
/// > [`Config`](super::Config) never calls it unless
/// > [`ConfigBuilder::geolocation(true)`](super::ConfigBuilder::geolocation) was set — enabling telemetry alone
/// > is not enough.
///
/// The request is given a one-second timeout and the response body is capped at 64 KiB.
///
/// **This blocks the calling thread** for up to the timeout — it goes through `reqwest`'s blocking client, which its
/// own documentation says should not be used inside an async runtime. Call it from a plain thread, or from
/// `tokio::task::spawn_blocking`. A [`Config`](super::Config) that opts into geolocation runs it on a
/// background thread of its own, so this only concerns calling the function directly.
///
/// # Errors
///
/// Returns an [`O11yError`](super::O11yError) if the request cannot be built or sent, the service answers with a
/// non-success status, or the body is not the expected JSON.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::fetch_geolocation;
///
/// let geo = fetch_geolocation()?;
/// println!("{:?}", geo.country);
/// # Ok(())
/// # }
/// ```
pub fn fetch_geolocation() -> Result<Geolocation> {
    fetch_geolocation_from(DEFAULT_URL)
}

/// Like [`fetch_geolocation`], but queries `<base_url>/json` instead of the default service.
///
/// Useful for a self-hosted or proxied lookup endpoint, and for testing against a local stub. Blocks the calling
/// thread, as [`fetch_geolocation`] does.
///
/// # Errors
///
/// Returns an [`O11yError`](super::O11yError) under the same conditions as [`fetch_geolocation`].
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::fetch_geolocation_from;
///
/// let geo = fetch_geolocation_from("https://ipinfo.example.internal")?;
/// # Ok(())
/// # }
/// ```
pub fn fetch_geolocation_from(base_url: &str) -> Result<Geolocation> {
    fetch_geolocation_with(base_url, DEFAULT_TIMEOUT)
}

/// Looks the location up against `base_url`, allowing `timeout` for the whole request.
///
/// The one-second default the other two functions use suits a public service on a healthy connection. A self-hosted
/// endpoint, one reached through a VPN or a proxy, or a deliberately slow test double may need longer.
///
/// Blocks the calling thread for up to `timeout`, as [`fetch_geolocation`] does — so a generous `timeout` here is
/// also a generous amount of time to tie up whichever thread calls it.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use std::time::Duration;
/// use rust_sak::o11y::fetch_geolocation_with;
///
/// let geo = fetch_geolocation_with("https://ipinfo.example.internal", Duration::from_secs(5))?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// As [`fetch_geolocation`].
pub fn fetch_geolocation_with(base_url: &str, timeout: Duration) -> Result<Geolocation> {
    // The shared client, or a throwaway one when it could not be built — only so the failure is reported rather
    // than silently swallowed by the `LazyLock`.
    let fallback;
    let client = match CLIENT.as_ref() {
        Some(client) => client,
        None => {
            fallback = reqwest::blocking::Client::builder().build()?;
            &fallback
        }
    };

    let response = client
        .get(format!("{}/json", base_url.trim_end_matches('/')))
        .timeout(timeout)
        .send()?
        .error_for_status()?;

    // Read through a capped reader before deserializing, so an endless body cannot exhaust memory.
    let mut body = Vec::new();
    response.take(MAX_RESPONSE).read_to_end(&mut body)?;

    Ok(serde_json::from_slice(&body)?)
}
