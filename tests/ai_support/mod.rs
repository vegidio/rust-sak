//! The real ONNX Runtime 1.30.0, installed through `ai` from a local copy of its archive, for the test binaries that
//! load it. A process loads one runtime, so each such binary installs its own.
//!
//! The archive is 10 MB and not checked in: point `RUST_SAK_ONNX_ARCHIVE` at a copy of `onnx_macos_arm64.7z` from
//! `deps/onnx/1.30.0` in `vegidio/general`. Without it the tests say so and pass.

// Each binary uses a subset of what is here.
#![allow(dead_code)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;

use rust_sak::ai::{self, Dependency, Source};
use rust_sak::fs::TempDir;

const ASSET: &str = "deps/onnx/1.30.0/onnx_macos_arm64.7z";
pub const COMMIT: &str = "501d7b94e6d22762667e213994b0242ccdb04588";

/// The archive's bytes, or `None` with a note when `RUST_SAK_ONNX_ARCHIVE` is not set.
pub fn archive() -> Option<Vec<u8>> {
    let Some(path) = std::env::var_os("RUST_SAK_ONNX_ARCHIVE") else {
        eprintln!("RUST_SAK_ONNX_ARCHIVE is not set; skipping the tests on the real runtime");
        return None;
    };
    Some(std::fs::read(path).unwrap())
}

/// Serve `archive` as the only file of `test/general` at [`COMMIT`], answering the three requests an install makes.
pub fn serve(archive: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let sha256 = rust_sak::crypto::sha256_bytes(&archive);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                line.clear();
            }
            let body: Vec<u8> = if path.contains("/revision/") {
                format!(r#"{{"sha":"{COMMIT}"}}"#).into_bytes()
            } else if path.contains("/tree/") {
                format!(
                    r#"[{{"type":"file","oid":"x","size":{},"lfs":{{"oid":"{sha256}","size":{}}},"path":"{ASSET}"}}]"#,
                    archive.len(),
                    archive.len()
                )
                .into_bytes()
            } else {
                assert_eq!(path, format!("/test/general/resolve/{COMMIT}/{ASSET}"));
                archive.clone()
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    base
}

/// The runtime's source on the server at `base`.
pub fn source(base: &str) -> Source {
    Source::parse(&format!("{base}/test/general/tree/main/deps/onnx/1.30.0")).unwrap()
}

/// Install the real runtime into a new temporary root, returning the root and the runtime's folder. `None` without
/// the archive.
pub async fn installed() -> Option<(TempDir, PathBuf)> {
    let base = serve(archive()?);
    let root = rust_sak::fs::mk_temp_dir("rust-sak-ai-runtime").unwrap();
    let source = source(&base);
    ai::install(
        root.path(),
        &[Dependency::runtime("ONNX Runtime", source.clone())],
        |_| {},
    )
    .await
    .unwrap();
    let dir = ai::dirs::dir_for(root.path(), &source);
    Some((root, dir))
}
