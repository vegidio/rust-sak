use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::chunked_read::for_each_chunk;

/// Computes the SHA-256 hash of the file at `path`, returning it as a lowercase hex string.
///
/// The file is streamed through the hasher in chunks rather than read fully into memory, so it works on arbitrarily
/// large files. Returns any I/O error encountered while opening or reading the file.
pub fn sha256_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut hasher = Sha256::new();
    for_each_chunk(path, |chunk| hasher.update(chunk))?;
    Ok(hex::encode(hasher.finalize()))
}
