use std::hash::Hasher;
use std::io;
use std::path::Path;

use twox_hash::XxHash3_64;

use super::chunked_read::for_each_chunk;

/// Computes the XXH3 (64-bit) hash of the file at `path`, returning it as a lowercase hex string.
///
/// The file is streamed through the hasher in chunks rather than read fully into memory, so it works on arbitrarily
/// large files. Returns any I/O error encountered while opening or reading the file.
pub fn xxh3_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut hasher = XxHash3_64::new();
    for_each_chunk(path, |chunk| hasher.write(chunk))?;
    Ok(hex::encode(hasher.finish().to_be_bytes()))
}
