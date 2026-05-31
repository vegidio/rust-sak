use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Streams the file at `path` in chunks, handing each chunk to `consume`.
/// Shared by the `*_file` hashing helpers, so the read loop lives in one place.
pub(super) fn for_each_chunk<P: AsRef<Path>>(path: P, mut consume: impl FnMut(&[u8])) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        consume(&buffer[..read]);
    }

    Ok(())
}
