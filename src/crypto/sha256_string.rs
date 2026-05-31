use super::sha256_bytes;

/// Computes the SHA-256 hash of `s` (its UTF-8 bytes), returning it as a lowercase hex string.
pub fn sha256_string(s: &str) -> String {
    sha256_bytes(s.as_bytes())
}
