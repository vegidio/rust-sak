//! Installing the real ONNX Runtime 1.30.0 through `ai`, from a local copy of its archive, and loading it. See
//! `ai_support` for the archive.
//!
//! A process loads one runtime, so this is a test binary of its own.

#![cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]

mod ai_support;

use rust_sak::ai::{self, Dependency, LoadError, Supported};

#[tokio::test]
async fn the_real_runtime_installs_and_loads_and_offers_webgpu() {
    let Some(archive) = ai_support::archive() else {
        return;
    };
    let base = ai_support::serve(archive);
    let root = rust_sak::fs::mk_temp_dir("rust-sak-ai-runtime").unwrap();
    let source = ai_support::source(&base);
    let runtime = Dependency::runtime("ONNX Runtime", source.clone());

    // Not installed yet: the load fails naming the folder, and can be tried again.
    let dir = ai::dirs::dir_for(root.path(), &source);
    assert_eq!(ai::load(&dir), Err(LoadError::NotInstalled { path: dir.clone() }));
    assert!(
        !Supported::detect(false, false).webgpu,
        "nothing is registered before a load"
    );

    ai::install(root.path(), std::slice::from_ref(&runtime), |_| {})
        .await
        .unwrap();
    assert!(ai::is_present(root.path(), &runtime));
    assert_eq!(ai::runtime_library(&dir), Some(dir.join("libonnxruntime.1.30.0.dylib")));

    // `ort` refuses a runtime older than the API it was built for (1.30), so this is 1.30.0's library loaded.
    ai::load(&dir).unwrap();
    assert_eq!(ort::MINOR_VERSION, 30);
    // The archive's plugin registered and offered this Mac's GPU.
    assert!(Supported::detect(false, false).webgpu);
    // A second load returns at once, whatever it names.
    assert_eq!(ai::load(&root.path().join("nowhere")), Ok(()));
}
