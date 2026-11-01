use std::io;
use std::path::Path;

use sha1::Digest;

use super::chunked_read::for_each_chunk;
use super::git_blob_sha1_bytes::git_blob_hasher;

/// Computes the git blob hash of the file at `path`, the object id `git hash-object` prints for it, as a lowercase hex
/// string: SHA-1 over `blob <length>\0` followed by the file's bytes.
///
/// The file is streamed through the hasher in chunks rather than read fully into memory.
///
/// # Errors
///
/// Returns any [`io::Error`](std::io::Error) encountered while opening or reading the file, and
/// [`io::ErrorKind::InvalidData`] when the file's length changed while it was read, since the header would then name
/// the wrong length.
pub fn git_blob_sha1_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let path = path.as_ref();
    let len = std::fs::metadata(path)?.len();
    let mut hasher = git_blob_hasher(len);
    let mut read = 0u64;
    for_each_chunk(path, |chunk| {
        read += chunk.len() as u64;
        hasher.update(chunk);
    })?;
    if read != len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} changed length while it was hashed ({len} bytes, then {read})",
                path.display()
            ),
        ));
    }
    Ok(hex::encode(hasher.finalize()))
}
