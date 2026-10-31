//! Installing the real ONNX Runtime 1.30.0 through `ai`, from a local copy of its archive, and loading it.
//!
//! A process loads one runtime, so this is a test binary of its own. The archive is 10 MB and not checked in: point
//! `RUST_SAK_ONNX_ARCHIVE` at a copy of `onnx_macos_arm64.7z` from `deps/onnx/1.30.0` in `vegidio/general`. Without it
//! the test says so and passes.

#![cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;

use rust_sak::ai::{self, Dependency, LoadError, Source};

const ASSET: &str = "deps/onnx/1.30.0/onnx_macos_arm64.7z";
const COMMIT: &str = "501d7b94e6d22762667e213994b0242ccdb04588";

/// Serve `archive` as the only file of `test/general` at [`COMMIT`], answering the three requests an install makes.
fn serve(archive: Vec<u8>) -> String {
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

#[tokio::test]
async fn the_real_runtime_installs_and_loads() {
    let Some(archive) = std::env::var_os("RUST_SAK_ONNX_ARCHIVE") else {
        eprintln!("RUST_SAK_ONNX_ARCHIVE is not set; skipping the real runtime's install and load");
        return;
    };
    let base = serve(std::fs::read(archive).unwrap());
    let root = rust_sak::fs::mk_temp_dir("rust-sak-ai-runtime").unwrap();
    let source = Source::parse(&format!("{base}/test/general/tree/main/deps/onnx/1.30.0")).unwrap();
    let runtime = Dependency::runtime("ONNX Runtime", source.clone());

    // Not installed yet: the load fails naming the folder, and can be tried again.
    let dir = ai::dirs::dir_for(root.path(), &source);
    assert_eq!(ai::load(&dir), Err(LoadError::NotInstalled { path: dir.clone() }));

    ai::install(root.path(), std::slice::from_ref(&runtime), |_| {})
        .await
        .unwrap();
    assert!(ai::is_present(root.path(), &runtime));
    assert_eq!(ai::runtime_library(&dir), Some(dir.join("libonnxruntime.1.30.0.dylib")));

    // `ort` refuses a runtime older than the API it was built for (1.30), so this is 1.30.0's library loaded.
    ai::load(&dir).unwrap();
    assert_eq!(ort::MINOR_VERSION, 30);
    // A second load returns at once, whatever it names.
    assert_eq!(ai::load(&root.path().join("nowhere")), Ok(()));
}
