use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// How many bytes are read at a time. Named rather than left as a literal in the loop below; `fetch::digest` reads
/// a partial's prefix in the same size, deliberately, so a resume and a re-read cost the same.
///
/// The buffer it sizes is heap-allocated rather than a stack array: 64 KiB is a large fraction of the stack an
/// embedder may hand a thread, and a hashing helper must not be the reason one overflows.
const CHUNK: usize = 64 * 1024;

/// Streams the file at `path` in chunks, handing each chunk to `consume`.
/// Shared by the `*_file` hashing helpers, so the read loop lives in one place.
pub(super) fn for_each_chunk<P: AsRef<Path>>(path: P, mut consume: impl FnMut(&[u8])) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; CHUNK];

    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            // A signal landing mid-read is not a failure: `read` is allowed to return this having done nothing, and
            // the convention is to go round again rather than fail a hash the caller cannot retry any better.
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };

        consume(&buffer[..read]);
    }

    Ok(())
}
