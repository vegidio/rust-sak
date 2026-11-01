//! Loading the real ONNX Runtime from a folder without its WebGPU plugin. See `ai_support` for the archive.
//!
//! A process loads one runtime, so this is a test binary of its own.

#![cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]

mod ai_support;

use rust_sak::ai::{self, ExecutionProvider, Supported};

#[tokio::test]
async fn a_runtime_without_its_plugin_loads_and_does_not_offer_webgpu() {
    let Some((_root, dir)) = ai_support::installed().await else {
        return;
    };
    std::fs::remove_file(dir.join("libonnxruntime_providers_webgpu.dylib")).unwrap();

    ai::load(&dir).unwrap();
    let supported = Supported::detect(false, false);
    assert!(!supported.webgpu);
    assert_eq!(
        supported.available(),
        [ExecutionProvider::Cpu, ExecutionProvider::CoreMl]
    );
}
