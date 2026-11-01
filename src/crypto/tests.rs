use std::fs;
use std::io;
use std::path::Path;
use std::process;

use super::*;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

const EMPTY_XXH3: &str = "2d06800538d394c2";
const ABC_XXH3: &str = "78af5f94892f3950";

/// Writes `content` to a uniquely-named temp file, hashes it with `hash`, removes the file, and
/// returns the resulting digest.
fn hash_temp_file(tag: &str, content: &[u8], hash: impl Fn(&Path) -> io::Result<String>) -> String {
    let path = std::env::temp_dir().join(format!("rust_sak_crypto_{}_{}.bin", tag, process::id()));
    fs::write(&path, content).unwrap();
    let result = hash(&path);
    fs::remove_file(&path).unwrap();
    result.unwrap()
}

#[test]
fn sha256_bytes_matches_known_vectors() {
    assert_eq!(sha256_bytes(b""), EMPTY_SHA256);
    assert_eq!(sha256_bytes(b"abc"), ABC_SHA256);
}

#[test]
fn sha256_string_matches_known_vectors() {
    assert_eq!(sha256_string(""), EMPTY_SHA256);
    assert_eq!(sha256_string("abc"), ABC_SHA256);
}

#[test]
fn sha256_string_agrees_with_bytes() {
    let content = "the quick brown fox";
    assert_eq!(sha256_string(content), sha256_bytes(content.as_bytes()));
}

#[test]
fn sha256_file_hashes_contents() {
    let content = b"the quick brown fox";
    assert_eq!(
        hash_temp_file("sha256", content, |p| sha256_file(p)),
        sha256_bytes(content)
    );
}

#[test]
fn sha256_file_missing_path_errors() {
    let path = std::env::temp_dir().join("rust_sak_crypto_does_not_exist.bin");
    assert!(sha256_file(&path).is_err());
}

#[test]
fn xxh3_bytes_matches_known_vectors() {
    assert_eq!(xxh3_bytes(b""), EMPTY_XXH3);
    assert_eq!(xxh3_bytes(b"abc"), ABC_XXH3);
}

#[test]
fn xxh3_string_matches_known_vectors() {
    assert_eq!(xxh3_string(""), EMPTY_XXH3);
    assert_eq!(xxh3_string("abc"), ABC_XXH3);
}

#[test]
fn xxh3_string_agrees_with_bytes() {
    let content = "the quick brown fox";
    assert_eq!(xxh3_string(content), xxh3_bytes(content.as_bytes()));
}

#[test]
fn xxh3_file_hashes_contents() {
    let content = b"the quick brown fox";
    assert_eq!(hash_temp_file("xxh3", content, |p| xxh3_file(p)), xxh3_bytes(content));
}

#[test]
fn xxh3_file_missing_path_errors() {
    let path = std::env::temp_dir().join("rust_sak_crypto_xxh3_does_not_exist.bin");
    assert!(xxh3_file(&path).is_err());
}

const EMPTY_SHA1: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
/// FIPS 180's two-block vector.
const TWO_BLOCK: &str = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const TWO_BLOCK_SHA1: &str = "84983e441c3bd26ebaae4aa1f95129e5e54670f1";

#[test]
fn sha1_bytes_matches_known_vectors() {
    assert_eq!(sha1_bytes(b""), EMPTY_SHA1);
    assert_eq!(sha1_bytes(b"abc"), ABC_SHA1);
    assert_eq!(sha1_bytes(TWO_BLOCK.as_bytes()), TWO_BLOCK_SHA1);
}

#[test]
fn sha1_file_hashes_contents() {
    assert_eq!(hash_temp_file("sha1", b"abc", |p| sha1_file(p)), ABC_SHA1);
    assert_eq!(
        hash_temp_file("sha1_two_block", TWO_BLOCK.as_bytes(), |p| sha1_file(p)),
        TWO_BLOCK_SHA1
    );
}

#[test]
fn sha1_file_missing_path_errors() {
    let path = std::env::temp_dir().join("rust_sak_crypto_sha1_does_not_exist.bin");
    assert!(sha1_file(&path).is_err());
    assert!(git_blob_sha1_file(&path).is_err());
}

#[test]
fn git_blob_sha1_file_matches_git_hash_object() {
    // What `git hash-object` prints for an empty file and for `hello world\n`.
    assert_eq!(
        hash_temp_file("git_blob_empty", b"", |p| git_blob_sha1_file(p)),
        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
    );
    assert_eq!(
        hash_temp_file("git_blob_hello", b"hello world\n", |p| git_blob_sha1_file(p)),
        "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
    );
}

#[test]
fn git_blob_sha1_bytes_matches_git_hash_object() {
    assert_eq!(git_blob_sha1_bytes(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    assert_eq!(
        git_blob_sha1_bytes(b"hello world\n"),
        "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
    );
}

#[test]
fn git_blob_sha1_file_matches_the_object_id_hugging_face_reports() {
    // SigLIP 2's `config.json` from `vegidio/general`, kept outside LFS: the API reports this `oid` for its 559 bytes.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/crypto/testdata/siglip2_config.json");
    assert_eq!(fs::metadata(&path).unwrap().len(), 559);
    assert_eq!(
        git_blob_sha1_file(&path).unwrap(),
        "7c1e0ed1759922fc4eb362d3b405958e829f364d"
    );
}
