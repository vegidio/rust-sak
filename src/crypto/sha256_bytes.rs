use sha2::{Digest, Sha256};

/// Computes the SHA-256 hash of `bytes`, returning it as a lowercase hex string.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
