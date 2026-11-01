//! Building real sessions through `ai` on the real ONNX Runtime, on a Mac: CoreML, WebGPU and the CPU. See
//! `ai_support` for the archive.
//!
//! A process loads one runtime, so this is a test binary of its own, and every test shares the load.

#![cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]

mod ai_support;

use std::path::{Path, PathBuf};

use rust_sak::ai::ExecutionProvider::{self, Auto, CoreMl, Cpu, WebGpu};
use rust_sak::ai::{self, SessionRequest, Supported, Tuning};
use rust_sak::fs::TempDir;
use tokio::sync::OnceCell;

/// The runtime, installed and loaded once for the whole binary. `None` without the archive.
async fn loaded() -> Option<&'static (TempDir, PathBuf)> {
    static RUNTIME: OnceCell<Option<(TempDir, PathBuf)>> = OnceCell::const_new();
    RUNTIME
        .get_or_init(|| async {
            let installed = ai_support::installed().await?;
            ai::load(&installed.1).unwrap();
            Some(installed)
        })
        .await
        .as_ref()
}

/// A one-node Relu model, float32 [1, 4]. Relu rather than Identity, which CoreML takes no part of, so it would
/// compile nothing.
fn relu() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai/testdata/relu.onnx")
}

/// The providers a session for `model` was built on when `providers` were asked for, with its cache in `cache`.
fn built_on(
    model: &Path,
    providers: &[ExecutionProvider],
    cache: &Path,
) -> Result<Vec<ExecutionProvider>, ai::SessionError> {
    let tuning = Tuning::default();
    let request = SessionRequest {
        model,
        providers,
        supported: Supported::detect(false, false),
        tuning: &tuning,
        cache_dir: cache,
    };
    ai::session(&request).map(|built| built.providers)
}

#[tokio::test]
async fn sessions_build_on_coreml_and_webgpu_for_auto_and_on_what_is_named() {
    let Some((root, _)) = loaded().await else {
        return;
    };
    let cache = root.path().join("cache/relu");
    assert_eq!(built_on(&relu(), &[Auto], &cache).unwrap(), [CoreMl, WebGpu]);
    assert_eq!(built_on(&relu(), &[Cpu], &cache).unwrap(), []);
    assert_eq!(built_on(&relu(), &[WebGpu], &cache).unwrap(), [WebGpu]);
    assert_eq!(built_on(&relu(), &[CoreMl], &cache).unwrap(), [CoreMl]);
}

#[tokio::test]
async fn coreml_compiles_into_the_models_cache_folder() {
    let Some((root, _)) = loaded().await else {
        return;
    };
    let cache = root.path().join("cache/coreml-test");
    assert_eq!(built_on(&relu(), &[CoreMl], &cache).unwrap(), [CoreMl]);
    let written = std::fs::read_dir(cache.join("coreml")).unwrap().count();
    assert!(
        written > 0,
        "CoreML wrote nothing into {}",
        cache.join("coreml").display()
    );
}

#[tokio::test]
async fn a_file_that_is_not_a_model_fails_with_its_path() {
    let Some((root, _)) = loaded().await else {
        return;
    };
    let model = root.path().join("not-a-model.onnx");
    std::fs::write(&model, b"not a model").unwrap();
    let error = built_on(&model, &[Auto], &root.path().join("cache/broken")).unwrap_err();
    assert_eq!(error.model, model);
    assert!(error.to_string().starts_with(&model.display().to_string()), "{error}");
    assert!(!error.reason.is_empty());
}
