use twox_hash::XxHash3_64;

/// Computes the XXH3 (64-bit) hash of `bytes`, returning it as a lowercase hex string.
pub fn xxh3_bytes(bytes: &[u8]) -> String {
    hex::encode(XxHash3_64::oneshot(bytes).to_be_bytes())
}
