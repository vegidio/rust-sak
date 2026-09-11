//! Opt-in content hashing of a transfer's bytes, computed as they are written.
//!
//! An artifact published with a checksum has to be hashed before it is trusted, and the download path is the one place
//! where its bytes are already in hand — so hashing them there costs nothing, while hashing afterwards is a second full
//! read of a file that may be hundreds of megabytes.
//!
//! A resumed transfer hashes the prefix already sitting in `<path>.part` before it appends, so the digest always
//! describes the whole artifact rather than the tail this attempt happened to carry.

use std::io;
use std::path::Path;

use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;

/// How many bytes are read at a time when hashing a partial's existing prefix. Matches the chunk size
/// [`crypto::sha256_file`](crate::crypto::sha256_file) reads with, so a resume and a re-read cost the same.
const PREFIX_CHUNK: usize = 64 * 1024;

/// The hash function a transfer computes over its bytes, selected with
/// [`RequestOptions::digest`](super::RequestOptions::digest) and reported by [`Download::digest`](super::Download::digest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DigestAlgorithm {
    /// SHA-256, reported as a lowercase hex string — the same digest
    /// [`crypto::sha256_file`](crate::crypto::sha256_file) produces for the finished file.
    Sha256,
}

/// A transfer's running hash. Built fresh for each attempt, since a retry re-derives its own offset and may restart
/// from byte zero.
#[derive(Debug)]
pub(super) struct Hasher {
    algorithm: DigestAlgorithm,
    sha256: Sha256,
}

impl Hasher {
    /// Starts an empty hash for `algorithm`.
    pub(super) fn new(algorithm: DigestAlgorithm) -> Self {
        Self {
            algorithm,
            sha256: Sha256::new(),
        }
    }

    /// Feeds `bytes` to the hash.
    pub(super) fn update(&mut self, bytes: &[u8]) {
        match self.algorithm {
            DigestAlgorithm::Sha256 => self.sha256.update(bytes),
        }
    }

    /// Feeds the first `len` bytes of the file at `path` to the hash.
    ///
    /// This is the resume case: bytes already in the partial belong to the artifact but never pass through this
    /// attempt's stream. `len` is the resume offset rather than the file's current length, so a partial that is longer
    /// than the offset the transfer actually continued from cannot leak extra bytes into the hash.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the file cannot be read, or if it turns out to be shorter than `len` — which would
    /// otherwise silently produce a digest of fewer bytes than were sent.
    pub(super) async fn update_prefix(&mut self, path: &Path, len: u64) -> io::Result<()> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = vec![0u8; PREFIX_CHUNK];
        let mut remaining = len;

        while remaining > 0 {
            let want = remaining.min(PREFIX_CHUNK as u64) as usize;
            let read = file.read(&mut buffer[..want]).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial file is shorter than the offset the transfer resumed from",
                ));
            }
            self.update(&buffer[..read]);
            remaining -= read as u64;
        }

        Ok(())
    }

    /// Consumes the hash and returns it as a lowercase hex string.
    pub(super) fn finish(self) -> String {
        match self.algorithm {
            DigestAlgorithm::Sha256 => hex::encode(self.sha256.finalize()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of `"0123456789"`, the fixture the prefix tests split in two.
    const DIGEST_0_TO_9: &str = "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882";

    #[test]
    fn hashes_bytes_fed_in_pieces() {
        let mut hasher = Hasher::new(DigestAlgorithm::Sha256);
        hasher.update(b"01234");
        hasher.update(b"56789");
        assert_eq!(hasher.finish(), DIGEST_0_TO_9);
    }

    #[tokio::test]
    async fn a_prefix_read_from_disk_joins_the_bytes_fed_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.bin.part");
        tokio::fs::write(&part, b"0123").await.unwrap();

        let mut hasher = Hasher::new(DigestAlgorithm::Sha256);
        hasher.update_prefix(&part, 4).await.unwrap();
        hasher.update(b"456789");

        assert_eq!(hasher.finish(), DIGEST_0_TO_9);
    }

    #[tokio::test]
    async fn a_prefix_stops_at_the_offset_rather_than_the_file_length() {
        // The partial holds more than the transfer resumed from: only the bytes up to the offset are the ones the
        // stream will continue after, so hashing the rest would describe an artifact nobody received.
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.bin.part");
        tokio::fs::write(&part, b"0123EXTRA").await.unwrap();

        let mut hasher = Hasher::new(DigestAlgorithm::Sha256);
        hasher.update_prefix(&part, 4).await.unwrap();
        hasher.update(b"456789");

        assert_eq!(hasher.finish(), DIGEST_0_TO_9);
    }

    #[tokio::test]
    async fn a_prefix_shorter_than_its_offset_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.bin.part");
        tokio::fs::write(&part, b"012").await.unwrap();

        let mut hasher = Hasher::new(DigestAlgorithm::Sha256);
        let err = hasher.update_prefix(&part, 4).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
