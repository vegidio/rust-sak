//! Loading an installed ONNX Runtime, and the one environment the process runs it under.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ort::environment::Environment;
use ort::logging::LogLevel;

use super::LoadError;
use super::install::MARKER;

/// The variable ONNX Runtime reads its full telemetry opt-out from, and the value that sets it.
const TELEMETRY_OPT_OUT: (&str, &str) = ("ORT_DISABLE_TELEMETRY", "1");

/// The process's environment, once loaded. `ort` keeps only a weak reference to it, and builds one per process.
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);

/// Whether the load registered the WebGPU plugin and it offered a device. Written once, by the load that succeeds.
static WEBGPU: AtomicBool = AtomicBool::new(false);

// ONNX Runtime 1.30 starts Microsoft's 1DS telemetry uploader on macOS and Linux when the environment is created, and
// `with_telemetry(false)` does not stop it: that only governs Windows' ETW. Open Photo AI traced a SIGABRT at exit to
// that uploader. The variable is the only full opt-out, and it must be set before the environment is created. Setting
// it here covers every process that links `ai`.
/// Opts the process out of ONNX Runtime's telemetry before anything in it can start the runtime.
#[ctor::ctor(unsafe)]
fn opt_out_of_runtime_telemetry() {
    let (name, value) = TELEMETRY_OPT_OUT;
    // SAFETY: a constructor runs while the process is still being loaded, before `main` and before any thread the
    // program starts, so nothing can read the environment concurrently with this write.
    unsafe { std::env::set_var(name, value) };
}

/// The ONNX Runtime library in `dir`, found by name for this platform: `libonnxruntime*.dylib` on macOS,
/// `libonnxruntime.so*` on Linux, `onnxruntime.dll` on Windows, never one of its `*_providers_*` plugins. `None` when
/// there is none.
pub fn runtime_library(dir: &Path) -> Option<PathBuf> {
    library_in(dir, std::env::consts::OS)
}

fn library_in(dir: &Path, os: &str) -> Option<PathBuf> {
    let mut found: Vec<(bool, String)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let file = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_file());
            let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
            (file && is_runtime_library(&name, os)).then_some((link, name))
        })
        .collect();
    // A real file over a link to it, and the most specific name, as `libonnxruntime.1.30.0.dylib` over
    // `libonnxruntime.dylib`.
    found.sort_by(|(a_link, a), (b_link, b)| a_link.cmp(b_link).then(b.len().cmp(&a.len())).then(a.cmp(b)));
    found.into_iter().next().map(|(_, name)| dir.join(name))
}

/// Whether a file named `name` is the runtime library on `os`, as `std::env::consts::OS` spells it.
fn is_runtime_library(name: &str, os: &str) -> bool {
    if name.contains("_providers_") {
        return false;
    }
    match os {
        "macos" => name.starts_with("libonnxruntime") && name.ends_with(".dylib"),
        "windows" => name.eq_ignore_ascii_case("onnxruntime.dll"),
        _ => name.starts_with("libonnxruntime.so"),
    }
}

/// Load the runtime installed in `runtime_dir`, the dependency's folder, and start the process's environment, its logs
/// going to `tracing` at warning level and above, its telemetry off.
///
/// The WebGPU plugin beside the runtime library is then registered, when this machine supports WebGPU, and
/// [`Supported::detect`](super::Supported::detect) offers WebGPU when it offered a device. A machine without WebGPU,
/// a missing plugin, a refused one or one with no device leaves WebGPU unoffered, with the reason logged, and never
/// fails the load.
///
/// One runtime is loaded per process: a load after a successful one returns at once, whatever folder it names. A
/// failed load can be tried again.
///
/// # Errors
///
/// [`LoadError`], naming the path: the folder has no install marker or no runtime library, the library cannot be
/// opened or is older than the runtime this crate's `ort` was built for, or it would not start an environment.
pub fn load(runtime_dir: &Path) -> Result<(), LoadError> {
    let mut held = lock();
    if held.is_some() {
        return Ok(());
    }
    if !std::fs::metadata(runtime_dir.join(MARKER)).is_ok_and(|meta| meta.is_file()) {
        return Err(LoadError::NotInstalled {
            path: runtime_dir.to_path_buf(),
        });
    }
    let path = runtime_library(runtime_dir).ok_or_else(|| LoadError::NotInstalled {
        path: runtime_dir.into(),
    })?;
    let environment = start(&path)?;
    WEBGPU.store(super::webgpu::register(&environment, runtime_dir), Ordering::Release);
    *held = Some(environment);
    Ok(())
}

/// Open the library at `path` and start an environment on it.
fn start(path: &Path) -> Result<Environment, LoadError> {
    let (name, value) = TELEMETRY_OPT_OUT;
    if std::env::var_os(name).is_none_or(|set| set != value) {
        tracing::warn!("{name} is not {value}; the ONNX Runtime may send telemetry");
    }
    if !std::fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Err(LoadError::NotInstalled {
            path: path.to_path_buf(),
        });
    }

    let builder = ort::init_from(path).map_err(|cause| LoadError::Open {
        path: path.into(),
        reason: cause.to_string(),
    })?;
    let environment = builder
        .with_name("rust-sak")
        .with_telemetry(false)
        .build()
        .map_err(|cause| LoadError::Start {
            path: path.into(),
            reason: cause.to_string(),
        })?;
    environment.set_log_level(LogLevel::Warning);
    tracing::info!("loaded the ONNX Runtime from {}", path.display());
    Ok(environment)
}

/// Whether the runtime's load registered the WebGPU plugin and it offered at least one device. `false` before a
/// successful load.
pub(super) fn webgpu_offered() -> bool {
    WEBGPU.load(Ordering::Acquire)
}

/// The process's environment, once a load has succeeded.
pub(super) fn environment() -> Option<Environment> {
    lock().clone()
}

/// A load that panicked while holding the lock left either an environment or none, both still valid.
fn lock() -> MutexGuard<'static, Option<Environment>> {
    ENVIRONMENT.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> crate::fs::TempDir {
        crate::fs::mk_temp_dir("rust-sak-ai-load").unwrap()
    }

    #[test]
    fn the_telemetry_opt_out_is_set_before_main() {
        assert_eq!(
            std::env::var("ORT_DISABLE_TELEMETRY").as_deref(),
            Ok("1"),
            "the constructor survived linking"
        );
    }

    #[test]
    fn ort_is_built_for_the_runtimes_api() {
        assert_eq!(
            ort::MINOR_VERSION,
            30,
            "the `api-30` feature matches the 1.30.0 runtime"
        );
    }

    #[test]
    fn the_runtime_library_is_found_by_name_beside_its_plugins() {
        let dir = temp();
        for name in [
            "libonnxruntime.1.30.0.dylib",
            "libonnxruntime_providers_webgpu.dylib",
            "README.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            library_in(dir.path(), "macos"),
            Some(dir.path().join("libonnxruntime.1.30.0.dylib"))
        );
        assert_eq!(library_in(dir.path(), "linux"), None);
        assert_eq!(library_in(&dir.path().join("missing"), "macos"), None);
    }

    #[test]
    fn each_platforms_name_is_recognised() {
        assert!(is_runtime_library("libonnxruntime.1.30.0.dylib", "macos"));
        assert!(is_runtime_library("libonnxruntime.dylib", "macos"));
        assert!(!is_runtime_library("libonnxruntime_providers_webgpu.dylib", "macos"));
        assert!(is_runtime_library("libonnxruntime.so.1.30.0", "linux"));
        assert!(is_runtime_library("libonnxruntime.so", "linux"));
        assert!(!is_runtime_library("libonnxruntime_providers_cuda.so", "linux"));
        assert!(!is_runtime_library("libonnxruntime.1.30.0.dylib", "linux"));
        assert!(is_runtime_library("onnxruntime.dll", "windows"));
        assert!(is_runtime_library("OnnxRuntime.DLL", "windows"));
        assert!(!is_runtime_library("onnxruntime_providers_shared.dll", "windows"));
    }

    #[cfg(unix)]
    #[test]
    fn the_versioned_file_wins_over_a_link_to_it() {
        let dir = temp();
        std::fs::write(dir.path().join("libonnxruntime.so.1.30.0"), b"x").unwrap();
        std::os::unix::fs::symlink("libonnxruntime.so.1.30.0", dir.path().join("libonnxruntime.so.1")).unwrap();
        std::os::unix::fs::symlink("libonnxruntime.so.1", dir.path().join("libonnxruntime.so")).unwrap();
        assert_eq!(
            library_in(dir.path(), "linux"),
            Some(dir.path().join("libonnxruntime.so.1.30.0"))
        );
    }

    #[test]
    fn a_folder_without_a_marker_or_library_is_not_installed() {
        let dir = temp();
        assert_eq!(
            load(dir.path()),
            Err(LoadError::NotInstalled {
                path: dir.path().into()
            }),
            "no marker"
        );
        std::fs::write(dir.path().join(MARKER), "abc").unwrap();
        assert_eq!(
            load(dir.path()),
            Err(LoadError::NotInstalled {
                path: dir.path().into()
            }),
            "no library"
        );
    }

    #[test]
    fn a_missing_library_fails_with_its_path() {
        let dir = temp();
        let path = dir.path().join("libonnxruntime.1.30.0.dylib");
        assert_eq!(start(&path).err(), Some(LoadError::NotInstalled { path }));
    }

    #[test]
    fn each_failed_load_reports_its_own_path() {
        // `ort` rc.15 leaves its load global empty after a failed load, so a second attempt really loads again.
        let dir = temp();
        for name in ["first.dylib", "second.dylib"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"not a library").unwrap();
            match start(&path) {
                Err(LoadError::Open { path: reported, reason }) => {
                    assert!(reported.ends_with(name), "{} for {name}", reported.display());
                    assert!(reason.contains(name), "the reason names {name}: {reason}");
                }
                other => panic!("expected {name} to fail to open, got {other:?}"),
            }
        }
    }
}
