use sha1::{Digest, Sha1};

/// Computes the SHA-1 hash of `bytes`, returning it as a lowercase hex string.
///
/// SHA-1 is broken for collision resistance: use it to match a digest someone else publishes as SHA-1 (as Git does),
/// not to choose one.
pub fn sha1_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}
