use std::io;
use std::path::Path;

use sha1::{Digest, Sha1};

use super::chunked_read::for_each_chunk;

/// Computes the SHA-1 hash of the file at `path`, returning it as a lowercase hex string.
///
/// The file is streamed through the hasher in chunks rather than read fully into memory, so it works on arbitrarily
/// large files.
///
/// # Errors
///
/// Returns any [`io::Error`](std::io::Error) encountered while opening or reading the file.
pub fn sha1_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut hasher = Sha1::new();
    for_each_chunk(path, |chunk| hasher.update(chunk))?;
    Ok(hex::encode(hasher.finalize()))
}
