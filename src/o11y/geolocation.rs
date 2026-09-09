//! IP-based location enrichment.
//!
//! This is the only part of the module that reaches out to a third party, and it is **opt-in**: a
//! [`Telemetry`](super::Telemetry) performs no lookup unless
//! [`TelemetryBuilder::geolocation(true)`](super::TelemetryBuilder::geolocation) was set.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

use super::Result;

/// How long the lookup may take before it is abandoned. Enrichment is never worth stalling on.
const TIMEOUT: Duration = Duration::from_secs(1);

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
/// > [`Telemetry`](super::Telemetry) never calls it unless
/// > [`TelemetryBuilder::geolocation(true)`](super::TelemetryBuilder::geolocation) was set — enabling telemetry alone
/// > is not enough.
///
/// The request is given a one-second timeout and the response body is capped at 64 KiB.
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
/// Useful for a self-hosted or proxied lookup endpoint, and for testing against a local stub.
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
    // A dedicated client rather than a shared global, which any other code in the process could reconfigure.
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build()?;
    let response = client
        .get(format!("{}/json", base_url.trim_end_matches('/')))
        .send()?
        .error_for_status()?;

    // Read through a capped reader before deserializing, so an endless body cannot exhaust memory.
    let mut body = Vec::new();
    response.take(MAX_RESPONSE).read_to_end(&mut body)?;

    Ok(serde_json::from_slice(&body)?)
}
