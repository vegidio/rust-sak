use sha1::{Digest, Sha1};

/// Computes the git blob hash of `bytes`, the object id `git hash-object` prints for a file holding them, as a
/// lowercase hex string: SHA-1 over `blob <length>\0` followed by the bytes.
pub fn git_blob_sha1_bytes(bytes: &[u8]) -> String {
    let mut hasher = git_blob_hasher(bytes.len() as u64);
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// A SHA-1 hasher that has taken the git blob header for `len` bytes of content.
pub(super) fn git_blob_hasher(len: u64) -> Sha1 {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {len}\0"));
    hasher
}
