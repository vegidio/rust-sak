use std::fs;
use std::process;

use super::*;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

const EMPTY_XXH3: &str = "2d06800538d394c2";
const ABC_XXH3: &str = "78af5f94892f3950";

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
    let path = std::env::temp_dir().join(format!("rust_sak_crypto_{}.bin", process::id()));
    fs::write(&path, content).unwrap();

    let result = sha256_file(&path);
    fs::remove_file(&path).unwrap();

    assert_eq!(result.unwrap(), sha256_bytes(content));
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
    let path = std::env::temp_dir().join(format!("rust_sak_crypto_xxh3_{}.bin", process::id()));
    fs::write(&path, content).unwrap();

    let result = xxh3_file(&path);
    fs::remove_file(&path).unwrap();

    assert_eq!(result.unwrap(), xxh3_bytes(content));
}

#[test]
fn xxh3_file_missing_path_errors() {
    let path = std::env::temp_dir().join("rust_sak_crypto_xxh3_does_not_exist.bin");
    assert!(xxh3_file(&path).is_err());
}
