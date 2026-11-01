# `crypto` module

Cryptographic and non-cryptographic hashing helpers. Every function returns the digest as a **lowercase hex `String`** and is **synchronous** (no async runtime is pulled in).

## Enabling

The module is gated behind the `crypto` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "2", features = ["crypto"] }
```

```rust
use rust_sak::crypto::{git_blob_sha1_bytes, git_blob_sha1_file, sha1_bytes, sha1_file, sha256_bytes, sha256_string, sha256_file, xxh3_bytes, xxh3_string, xxh3_file};
```

## Public API

Three algorithm families: **SHA-256** (cryptographic) and **XXH3-64** (fast, non-cryptographic), each with the same `_bytes` / `_string` / `_file` trio, and **SHA-1**, for matching digests others publish as SHA-1, such as Git's object ids.

### SHA-256

| Function        | Signature                                                       | What it does                                                                                                                                                   |
|-----------------|-----------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `sha256_bytes`  | `fn sha256_bytes(bytes: &[u8]) -> String`                       | Hashes an in-memory byte slice.                                                                                                                                |
| `sha256_string` | `fn sha256_string(s: &str) -> String`                           | Hashes a string's UTF-8 bytes.                                                                                                                                 |
| `sha256_file`   | `fn sha256_file<P: AsRef<Path>>(path: P) -> io::Result<String>` | Hashes a file, **streamed in 64 KiB chunks** (never loads the whole file), so it works on arbitrarily large files. Returns any I/O error from opening/reading. |

### SHA-1

| Function              | Signature                                                              | What it does                                                                                                                                                                                                                                                 |
|-----------------------|------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `sha1_bytes`          | `fn sha1_bytes(bytes: &[u8]) -> String`                                | Hashes an in-memory byte slice.                                                                                                                                                                                                                              |
| `sha1_file`           | `fn sha1_file<P: AsRef<Path>>(path: P) -> io::Result<String>`          | Hashes a file, streamed in chunks.                                                                                                                                                                                                                           |
| `git_blob_sha1_bytes` | `fn git_blob_sha1_bytes(bytes: &[u8]) -> String`                       | The **git blob hash** of an in-memory byte slice: SHA-1 over `blob <length>\0` followed by the bytes.                                                                                                                                                        |
| `git_blob_sha1_file`  | `fn git_blob_sha1_file<P: AsRef<Path>>(path: P) -> io::Result<String>` | The file's **git blob hash**, what `git hash-object` prints: SHA-1 over `blob <length>\0` followed by the bytes, streamed. Hugging Face reports it as the `oid` of a file outside LFS. Fails with `InvalidData` if the file changes length while it is read. |

> SHA-1 is broken for collision resistance. Use it to check a digest someone else publishes as SHA-1, not to pick one.

### XXH3 (64-bit)

| Function      | Signature                                                     | What it does                       |
|---------------|---------------------------------------------------------------|------------------------------------|
| `xxh3_bytes`  | `fn xxh3_bytes(bytes: &[u8]) -> String`                       | Hashes an in-memory byte slice.    |
| `xxh3_string` | `fn xxh3_string(s: &str) -> String`                           | Hashes a string's UTF-8 bytes.     |
| `xxh3_file`   | `fn xxh3_file<P: AsRef<Path>>(path: P) -> io::Result<String>` | Hashes a file, streamed in chunks. |

> XXH3-64 output is **big-endian** so it matches the hex produced by the `xxhsum` CLI tool.

## Usage

```rust
use rust_sak::crypto::{sha256_string, xxh3_bytes, xxh3_string, sha256_file};

// In-memory hashing — infallible, returns a String directly.
assert_eq!(
    sha256_string("abc"),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
);
assert_eq!(xxh3_string("abc"), "78af5f94892f3950");
assert_eq!(xxh3_bytes(b"abc"), xxh3_string("abc"));

// File hashing — fallible (I/O), returns io::Result<String>.
let digest = sha256_file("Cargo.toml")?;
println!("{digest}");

// The object id Git gives the file, as `git hash-object Cargo.toml` prints it.
let oid = rust_sak::crypto::git_blob_sha1_file("Cargo.toml")?;
assert_eq!(oid.len(), 40);
# Ok::<(), std::io::Error>(())
```

Pick **SHA-256** when you need a cryptographic digest (integrity, content addressing where collisions must be infeasible). Pick **XXH3-64** when you just need a fast checksum (cache keys, change detection) and don't need cryptographic guarantees.
