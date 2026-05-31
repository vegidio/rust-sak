use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Computes the SHA-256 hash of the file at `path`, returning it as a lowercase hex string.
///
/// The file is streamed through the hasher in chunks rather than read fully into memory, so it works on arbitrarily
/// large files. Returns any I/O error encountered while opening or reading the file.
pub fn sha256_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}
