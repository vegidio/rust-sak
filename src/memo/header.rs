use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

/// The stored-value format this build writes and accepts.
///
/// Bump it when the layout below changes. It does not help with a change to a *cached type*, which is the caller's
/// to version: see [`fingerprint`] for what that does and does not catch.
pub(super) const VERSION: u8 = 0x01;

/// The version byte plus the type fingerprint that precede every typed value.
pub(super) const LEN: usize = 1 + 8;

/// A short, stable-within-a-build tag for `T`, used to keep one cache key from serving two different types.
///
/// This is the first 8 bytes of the SHA-256 of [`std::any::type_name`], read big-endian. It is **not** a schema
/// identifier. `type_name` is explicitly not stable across compiler versions or across a type moving between
/// modules, which is harmless — an unrecognised fingerprint is a miss, and a miss costs one recomputation.
///
/// The direction that does matter is the other one: `type_name` does not change when the type's *fields* do. Adding
/// a field still fails to decode, and removing one leaves bytes over, so [`decode`] catches both. Reordering fields
/// of the same type is the case nothing here can see — the name, the length and the parse all stay valid while the
/// meaning changes — so a caller who does that to a type they cache on disk has to version the cache key.
pub(super) fn fingerprint<T: ?Sized>() -> u64 {
    let digest = Sha256::digest(std::any::type_name::<T>().as_bytes());
    u64::from_be_bytes(digest[..8].try_into().expect("a SHA-256 digest is 32 bytes"))
}

/// Encodes `value` behind a [`VERSION`] byte and `fingerprint`.
pub(super) fn encode<T>(fingerprint: u64, value: &T) -> Result<Vec<u8>, postcard::Error>
where
    T: Serialize,
{
    let mut bytes = Vec::with_capacity(LEN);
    bytes.push(VERSION);
    bytes.extend_from_slice(&fingerprint.to_be_bytes());

    postcard::to_extend(value, bytes)
}

/// Decodes a value written by [`encode`], or `None` if these bytes are not one this caller can read.
///
/// Every rejection is a **miss**, never an error: too short, an unrecognised version, a fingerprint belonging to some
/// other type, a payload postcard cannot parse, or one with bytes left over. Postcard is not self-describing, so
/// without these checks a `Vec<GpuInfo>` decoded as a `CpuInfo` would yield garbage rather than fail — turning that
/// into a miss is the whole reason the header exists.
///
/// The leftover-bytes check is not redundant. `postcard::from_bytes` ignores a trailing remainder, so a struct that
/// has *lost* a field since the entry was written would otherwise decode cleanly from the longer old payload and
/// hand back a stale value. Requiring the payload to be consumed exactly turns that into a miss too.
pub(super) fn decode<T>(fingerprint: u64, bytes: &[u8]) -> Option<T>
where
    T: DeserializeOwned,
{
    let (header, payload) = bytes.split_at_checked(LEN)?;
    if header[0] != VERSION || header[1..] != fingerprint.to_be_bytes() {
        return None;
    }

    let (value, rest) = postcard::take_from_bytes(payload).ok()?;
    rest.is_empty().then_some(value)
}
