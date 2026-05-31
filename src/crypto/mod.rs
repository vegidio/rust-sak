//! Cryptographic hashing utilities.
//!
//! This module exposes hashing helpers that all return the digest as a lowercase hex [`String`], in two algorithm
//! families:
//!
//! - **SHA-256** — [`sha256_bytes`] (byte slice), [`sha256_string`] (string data), [`sha256_file`] (streams a file's
//! contents through the hasher, chunked, so it never loads the whole file).
//! - **XXH3 (64-bit)** — [`xxh3_bytes`], [`xxh3_string`], [`xxh3_file`]: the same trio backed by the fast,
//! non-cryptographic XXH3 algorithm.
//!
//! ```
//! use rust_sak::crypto::{sha256_string, xxh3_bytes, xxh3_string};
//!
//! assert_eq!(
//!     sha256_string("abc"),
//!     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
//! );
//! assert_eq!(xxh3_string("abc"), "78af5f94892f3950");
//! assert_eq!(xxh3_bytes(b"abc"), xxh3_string("abc"));
//! ```

mod sha256_bytes;
mod sha256_file;
mod sha256_string;
mod xxh3_bytes;
mod xxh3_file;
mod xxh3_string;

pub use sha256_bytes::sha256_bytes;
pub use sha256_file::sha256_file;
pub use sha256_string::sha256_string;
pub use xxh3_bytes::xxh3_bytes;
pub use xxh3_file::xxh3_file;
pub use xxh3_string::xxh3_string;

#[cfg(test)]
mod tests;
