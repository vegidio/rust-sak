use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Returns a stable machine identifier, scoped to `service`, as a lowercase hex string.
///
/// The raw host id (from `machine-uid`) is **never** returned: it is identical across every program on the machine,
/// so emitting it would let a backend correlate this application's telemetry with that of any other. Instead the host
/// id keys an HMAC-SHA256 over the service name, which yields an id that is stable for this machine *and* this
/// service, and uncorrelatable with the id any other service derives from the same host.
///
/// This is the same construction as Go's `machineid.ProtectedID`, so a Rust and a Go build of the same application
/// report the same id on the same machine.
///
/// Returns `None` if the host id cannot be read (an unsupported platform, or a locked-down environment), in which
/// case the caller simply omits the attribute rather than failing.
pub(super) fn machine_id(service: &str) -> Option<String> {
    let host_id = machine_uid::get().ok()?;

    let mut mac = Hmac::<Sha256>::new_from_slice(host_id.as_bytes()).ok()?;
    mac.update(service.as_bytes());

    Some(hex::encode(mac.finalize().into_bytes()))
}
