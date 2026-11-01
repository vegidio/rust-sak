//! Loading an installed ONNX Runtime, and the one environment the process runs it under.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use ort::device::Device;
use ort::environment::Environment;
use ort::logging::{LogLevel, LoggerFunction};

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
            let file = entry.path().is_file();
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

/// How [`load_with`] starts the environment.
#[derive(Clone)]
pub struct LoadOptions<'a> {
    /// The environment's name, which the runtime's records carry as their logger id. Name it after the application.
    pub name: &'a str,
    /// Where the runtime's records go. `None` sends them to `tracing`, under the target `ort::logging`, in the shape
    /// `ort`'s own bridge gives them. Either way each record passes through this module first, which counts the
    /// out-of-memory ones a failed [`build`](super::build) is classified by.
    pub logger: Option<LoggerFunction>,
}

impl Default for LoadOptions<'_> {
    /// Named `rust-sak`, its records going to `tracing`.
    fn default() -> Self {
        Self {
            name: "rust-sak",
            logger: None,
        }
    }
}

impl std::fmt::Debug for LoadOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadOptions")
            .field("name", &self.name)
            .field("logger", &self.logger.as_ref().map(|_| "custom"))
            .finish()
    }
}

/// Load the runtime installed in `runtime_dir`, the dependency's folder, with [`LoadOptions::default`]. See
/// [`load_with`].
///
/// # Errors
///
/// As [`load_with`].
pub fn load(runtime_dir: &Path) -> Result<(), LoadError> {
    load_with(runtime_dir, &LoadOptions::default())
}

/// Load the runtime installed in `runtime_dir`, the dependency's folder, and start the process's environment, named
/// and logging as `options` say, at warning level and above, its telemetry off. The environment is then
/// [`environment`].
///
/// The WebGPU plugin beside the runtime library is then registered, when this machine supports WebGPU, and
/// [`Supported::detect`](super::Supported::detect) offers WebGPU when it offered a device. A machine without WebGPU,
/// a missing plugin, a refused one or one with no device leaves WebGPU unoffered, with the reason logged, and never
/// fails the load.
///
/// The runtime's logging is dropped to fatal as the process exits, before the runtime releases its environment: a
/// record it logged while being torn down would otherwise reach a logger whose thread-locals are already gone, and
/// abort the process after its work is done.
///
/// One runtime is loaded per process: a load after a successful one returns at once, whatever folder or options it
/// names. A failed load can be tried again.
///
/// # Errors
///
/// [`LoadError`], naming the path: the folder has no install marker or no runtime library, the library cannot be
/// opened or is older than the runtime this crate's `ort` was built for, or it would not start an environment.
pub fn load_with(runtime_dir: &Path, options: &LoadOptions<'_>) -> Result<(), LoadError> {
    let mut held = lock();
    if held.is_some() {
        return Ok(());
    }
    if !runtime_dir.join(MARKER).is_file() {
        return Err(LoadError::NotInstalled {
            path: runtime_dir.to_path_buf(),
        });
    }
    let path = runtime_library(runtime_dir).ok_or_else(|| LoadError::NotInstalled {
        path: runtime_dir.into(),
    })?;
    let environment = start(&path, options)?;
    WEBGPU.store(super::webgpu::register(&environment, runtime_dir), Ordering::Release);
    *held = Some(environment);
    silence_on_exit();
    Ok(())
}

/// Open the library at `path` and start an environment on it.
fn start(path: &Path, options: &LoadOptions<'_>) -> Result<Environment, LoadError> {
    let (name, value) = TELEMETRY_OPT_OUT;
    if std::env::var_os(name).is_none_or(|set| set != value) {
        tracing::warn!("{name} is not {value}; the ONNX Runtime may send telemetry");
    }
    if !path.is_file() {
        return Err(LoadError::NotInstalled {
            path: path.to_path_buf(),
        });
    }

    let builder = ort::init_from(path)
        .map_err(|cause| LoadError::Open {
            path: path.into(),
            reason: cause.to_string(),
        })?
        .with_name(options.name)
        .with_telemetry(false)
        .with_logger(super::logger::wrap(options.logger.clone()));
    let environment = builder.build().map_err(|cause| LoadError::Start {
        path: path.into(),
        reason: cause.to_string(),
    })?;
    environment.set_log_level(LogLevel::Warning);
    tracing::info!("loaded the ONNX Runtime from {} as {}", path.display(), options.name);
    Ok(environment)
}

/// Drop the environment's log level to fatal at process exit. Registered after the environment exists, so it runs
/// before `ort`'s own release of it: exit handlers run in reverse order of registration, and on Linux and Windows the
/// release runs later still. Registered once, by the load that succeeds.
fn silence_on_exit() {
    unsafe extern "C" {
        fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
    }

    extern "C" fn silence() {
        // `try_lock`: an exit while a thread holds the lock must not hang on it. Nothing here touches a thread-local.
        let held = match ENVIRONMENT.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        if let Some(environment) = held.as_ref() {
            environment.set_log_level(LogLevel::Fatal);
        }
    }

    // SAFETY: `silence` is a plain `extern "C"` function with no arguments, as `atexit` requires.
    if unsafe { atexit(silence) } != 0 {
        tracing::warn!("the runtime's logging could not be silenced at exit");
    }
}

/// Whether the runtime's load registered the WebGPU plugin and it offered at least one device. `false` before a
/// successful load.
pub(super) fn webgpu_offered() -> bool {
    WEBGPU.load(Ordering::Acquire)
}

/// The process's environment, once a load has succeeded: the one every session is built under. It lives until the
/// process exits.
pub fn environment() -> Option<Environment> {
    lock().clone()
}

/// The devices the WebGPU plugin registered with `environment`, in the runtime's order. None unless the load
/// registered the plugin.
pub fn webgpu_devices(environment: &Environment) -> impl Iterator<Item = Device<'_>> {
    super::webgpu::devices(environment)
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
        assert_eq!(
            start(&path, &LoadOptions::default()).err(),
            Some(LoadError::NotInstalled { path })
        );
    }

    #[test]
    fn each_failed_load_reports_its_own_path() {
        // `ort` rc.15 leaves its load global empty after a failed load, so a second attempt really loads again.
        let dir = temp();
        for name in ["first.dylib", "second.dylib"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"not a library").unwrap();
            match start(&path, &LoadOptions::default()) {
                Err(LoadError::Open { path: reported, reason }) => {
                    assert!(reported.ends_with(name), "{} for {name}", reported.display());
                    assert!(reason.contains(name), "the reason names {name}: {reason}");
                }
                other => panic!("expected {name} to fail to open, got {other:?}"),
            }
        }
    }
}
