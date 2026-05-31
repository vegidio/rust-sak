use super::xxh3_bytes;

/// Computes the XXH3 (64-bit) hash of `s` (its UTF-8 bytes), returning it as a lowercase hex string.
pub fn xxh3_string(s: &str) -> String {
    xxh3_bytes(s.as_bytes())
}
