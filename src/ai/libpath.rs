//! Making the runtime and NVIDIA library folders findable by the platform's loader.
//!
//! Windows' `LoadLibrary` reads `PATH` when it is called, so extending it in the process is enough. glibc reads
//! `LD_LIBRARY_PATH` once, when the process starts, so on Linux the process restarts itself once under the new value.
//! macOS needs nothing: no NVIDIA library exists for it, and CoreML and WebGPU ship inside the runtime's folder.
//!
//! The rules are pure functions and tested everywhere. The Linux and Windows arms are written to be correct by
//! construction, and are not built or tested on those platforms by this crate.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// The Windows driver store as WSL2 mounts it: one folder per driver package, old versions included.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const WSL_DRIVER_STORE: &str = "/usr/lib/wsl/drivers";

/// The search path with `dirs` first, then the entries of `inherited`, the variable's current value.
///
/// The given folders go first so a system-wide install of another version cannot win, and inherited entries are kept,
/// since a user who set the variable meant it. Repeats are dropped keeping the first, and so are empty entries, which
/// mean the current folder to the loader.
pub fn search_path(dirs: &[PathBuf], inherited: Option<&OsStr>) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    let inherited = inherited.map(std::env::split_paths).into_iter().flatten();
    for entry in dirs.iter().cloned().chain(inherited) {
        if !entry.as_os_str().is_empty() && !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    // Only an entry holding the platform's separator fails to join, which `split_paths` cannot produce; one given in
    // `dirs` is dropped rather than allowed to become two entries.
    entries.retain(|entry| std::env::join_paths([entry]).is_ok());
    std::env::join_paths(entries).unwrap_or_default()
}

/// Whether the guard variable says this process is already the restarted one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_restarted(guard: Option<&OsStr>) -> bool {
    guard.is_some_and(|value| value == "1")
}

/// Puts `dirs` ahead of the loader's search path, creating those that are missing, and on Linux restarts the process
/// under it, once.
///
/// - **Linux**: sets `LD_LIBRARY_PATH` to `dirs`, then the active Windows driver's folder under WSL2 with an NVIDIA GPU,
///   then the inherited value, and replaces the process with itself, with the same arguments and `guard` set to `1`.
///   With `guard` already `1` it does nothing. On success it **does not return**. A restart that fails is printed to
///   stderr, since no logging can be set up yet, and the run goes on without the GPU libraries.
/// - **Windows**: prepends `dirs` to `PATH`.
/// - **macOS** and anything else: nothing, and nothing is created.
///
/// The folders are created first, because a loader skips a search-path entry that is missing at start for the rest of
/// the process, so libraries installed later in the same run would not be found. Name `guard` for the app, as
/// `MEDIACAT_REEXEC`, so it does not collide with another program's restart guard.
///
/// # Safety
///
/// Call it as the first statement of `main`, before any other thread exists and before logging is set up. It writes the
/// process environment, which races with any thread reading it, and on Linux it replaces the process image, which keeps
/// open file descriptors.
///
/// # Errors
///
/// The file error that stopped a folder from being created. A failed restart is not an error.
pub unsafe fn prepare(dirs: &[PathBuf], guard: &str) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    for dir in dirs {
        std::fs::create_dir_all(dir)?;
    }

    // SAFETY: the caller's contract is that no other thread exists yet.
    unsafe { apply(dirs, guard) };
    Ok(())
}

/// # Safety
///
/// As [`prepare`].
#[cfg(target_os = "linux")]
unsafe fn apply(dirs: &[PathBuf], guard: &str) {
    use std::os::unix::process::CommandExt;

    if is_restarted(std::env::var_os(guard).as_deref()) {
        return;
    }
    let Ok(program) = std::env::current_exe() else {
        eprintln!("could not find this executable to restart it; the NVIDIA libraries will not be found");
        return;
    };

    let mut ours = dirs.to_vec();
    ours.extend(wsl_cuda_driver_dir());
    let error = std::process::Command::new(program)
        .args(std::env::args_os().skip(1))
        .env(
            "LD_LIBRARY_PATH",
            search_path(&ours, std::env::var_os("LD_LIBRARY_PATH").as_deref()),
        )
        .env(guard, "1")
        .exec();

    // `exec` returns only when it failed.
    eprintln!("could not restart with LD_LIBRARY_PATH set; the NVIDIA libraries will not be found: {error}");
}

/// # Safety
///
/// As [`prepare`]: no other thread may exist, which is what makes [`std::env::set_var`] sound.
#[cfg(target_os = "windows")]
unsafe fn apply(dirs: &[PathBuf], _guard: &str) {
    let path = search_path(dirs, std::env::var_os("PATH").as_deref());
    // SAFETY: the caller's contract is that no other thread exists yet.
    unsafe { std::env::set_var("PATH", path) };
}

/// # Safety
///
/// As [`prepare`], though this touches nothing.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
unsafe fn apply(_dirs: &[PathBuf], _guard: &str) {}

/// Under WSL2 with an NVIDIA GPU, the folder of the Windows driver CUDA actually uses.
///
/// WSL2's `libcuda.so.1` is a stub that loads the real driver, `libcuda.so.1.1`, from the Windows driver store, and
/// that driver loads its PTX JIT compiler by name alone. The store is on no search path, so on a distribution with a
/// native NVIDIA driver package the loader finds that package's compiler, from another driver version, and the first
/// CUDA call crashes in it. Putting the driver's own folder on the path makes its compiler win.
///
/// The folder is asked for, not guessed, because the store keeps old drivers beside the current one: `cuInit` makes
/// the stub load the real driver, and `/proc/self/maps` says where it came from. That costs a CUDA initialisation on
/// WSL2 with an NVIDIA GPU, and nothing anywhere else. The driver is never unloaded, and the process is about to be
/// replaced anyway.
#[cfg(target_os = "linux")]
fn wsl_cuda_driver_dir() -> Option<PathBuf> {
    use std::ffi::{c_int, c_uint};

    const WSL_CUDA_STUB: &str = "/usr/lib/wsl/lib/libcuda.so.1";
    if !Path::new(WSL_CUDA_STUB).is_file() {
        return None;
    }

    // SAFETY: loading the stub runs its initialisers, which it ships for a process to load it.
    let library = unsafe { libloading::Library::new(WSL_CUDA_STUB) }.ok()?;
    // The status is not read: a failed `cuInit` still leaves the driver in the maps if it got that far.
    //
    // SAFETY: `cuInit` has this signature in `cuda.h`, and the symbol does not outlive `library`, which is never
    // unloaded. `0` is the only flag value it accepts.
    unsafe {
        let init: libloading::Symbol<unsafe extern "C" fn(c_uint) -> c_int> = library.get(b"cuInit\0").ok()?;
        init(0);
    }
    std::mem::forget(library);

    driver_dir_in_maps(&std::fs::read_to_string("/proc/self/maps").ok()?)
}

/// The driver-store folder the real CUDA driver was mapped from, read from a `/proc/<pid>/maps` listing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn driver_dir_in_maps(maps: &str) -> Option<PathBuf> {
    maps.lines()
        // The path is the last column and the only one with a `/`, and it may contain spaces.
        .filter_map(|line| line.find('/').map(|start| Path::new(&line[start..])))
        .find(|path| path.starts_with(WSL_DRIVER_STORE) && path.file_name() == Some(OsStr::new("libcuda.so.1.1")))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `entries` joined as the platform spells a search path.
    fn joined(entries: &[&str]) -> OsString {
        std::env::join_paths(entries.iter().map(Path::new)).unwrap()
    }

    fn ours() -> [PathBuf; 2] {
        [PathBuf::from("/ai/onnx/1.30.0"), PathBuf::from("/ai/cuda/13.3.0")]
    }

    #[test]
    fn the_given_folders_come_first_and_inherited_entries_are_kept() {
        let inherited = joined(&["/opt/lib", "/usr/local/lib"]);
        assert_eq!(
            search_path(&ours(), Some(&inherited)),
            joined(&["/ai/onnx/1.30.0", "/ai/cuda/13.3.0", "/opt/lib", "/usr/local/lib"])
        );
    }

    #[test]
    fn repeats_are_dropped_keeping_the_first() {
        let inherited = joined(&["/opt/lib", "/ai/onnx/1.30.0", "/opt/lib"]);
        assert_eq!(
            search_path(&ours(), Some(&inherited)),
            joined(&["/ai/onnx/1.30.0", "/ai/cuda/13.3.0", "/opt/lib"])
        );
        let twice = [PathBuf::from("/ai/onnx/1.30.0"), PathBuf::from("/ai/onnx/1.30.0")];
        assert_eq!(search_path(&twice, None), joined(&["/ai/onnx/1.30.0"]));
    }

    #[test]
    fn empty_entries_are_dropped() {
        let mut inherited = joined(&["", "/opt/lib"]);
        inherited.push(joined(&["", ""]));
        assert_eq!(
            search_path(&[PathBuf::new(), PathBuf::from("/ai/onnx/1.30.0")], Some(&inherited)),
            joined(&["/ai/onnx/1.30.0", "/opt/lib"])
        );
    }

    #[test]
    fn nothing_inherited_gives_the_folders_alone() {
        assert_eq!(
            search_path(&ours(), None),
            joined(&["/ai/onnx/1.30.0", "/ai/cuda/13.3.0"])
        );
        assert_eq!(
            search_path(&ours(), Some(OsStr::new(""))),
            joined(&["/ai/onnx/1.30.0", "/ai/cuda/13.3.0"])
        );
        assert_eq!(search_path(&[], None), OsString::new());
    }

    #[test]
    fn only_a_guard_of_1_means_restarted() {
        assert!(is_restarted(Some(OsStr::new("1"))));
        assert!(!is_restarted(None));
        assert!(!is_restarted(Some(OsStr::new(""))));
        assert!(!is_restarted(Some(OsStr::new("0"))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_nothing_is_created_or_changed() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-libpath").unwrap();
        let folders = [dir.path().join("onnx/1.30.0"), dir.path().join("cuda/13.3.0")];
        let before = std::env::var_os("DYLD_LIBRARY_PATH");
        // SAFETY: on macOS `prepare` touches neither the environment nor the process.
        unsafe { prepare(&folders, "RUST_SAK_TEST_REEXEC") }.unwrap();
        assert!(folders.iter().all(|folder| !folder.exists()));
        assert_eq!(std::env::var_os("DYLD_LIBRARY_PATH"), before);
    }

    /// A `/proc/self/maps` excerpt from a WSL2 process after `cuInit`.
    const WSL_MAPS: &str = "\
7b64646d4000-7b64646d5000 r--p 00000000 00:3f 2533274790988457           /usr/lib/wsl/lib/libcuda.so.1
7b612e6c0000-7b612e6c1000 r--p 00000000 00:40 1125899906950443           /usr/lib/wsl/drivers/nv_dispsi.inf_amd64_d95662815b9b13a8/libcuda.so.1.1
7b612e6c1000-7b612f854000 r-xp 00001000 00:40 1125899906950443           /usr/lib/wsl/drivers/nv_dispsi.inf_amd64_d95662815b9b13a8/libcuda.so.1.1
7b640f773000-7b640f774000 r--p 00000000 00:3f 1970324837567045           /usr/lib/wsl/lib/libdxcore.so
7ffd2c9f1000-7ffd2ca12000 rw-p 00000000 00:00 0                          [stack]
";

    #[test]
    fn the_wsl_driver_folder_is_where_the_real_driver_was_mapped_from() {
        assert_eq!(
            driver_dir_in_maps(WSL_MAPS),
            Some(PathBuf::from(
                "/usr/lib/wsl/drivers/nv_dispsi.inf_amd64_d95662815b9b13a8"
            ))
        );
    }

    #[test]
    fn no_wsl_driver_folder_without_the_real_driver() {
        let stub_only = "7b64646d4000-7b64646d5000 r--p 00000000 00:3f 25332 /usr/lib/wsl/lib/libcuda.so.1\n";
        assert_eq!(driver_dir_in_maps(stub_only), None);
        assert_eq!(driver_dir_in_maps(""), None);
        let native = "7b612e6c0000-7b612e6c1000 r--p 00000000 08:20 4242 /usr/lib/x86_64-linux-gnu/libcuda.so.1.1\n";
        assert_eq!(driver_dir_in_maps(native), None, "a native driver is not in the store");
        let loader = "7b612e6c0000-7b612e6c1000 r--p 00000000 00:40 42 /usr/lib/wsl/drivers/nv_old/libcuda_loader.so\n";
        assert_eq!(driver_dir_in_maps(loader), None);
    }

    #[test]
    fn a_driver_folder_with_spaces_is_read_whole() {
        let maps = "7b61-7b62 r--p 00000000 00:40 42 /usr/lib/wsl/drivers/nv dispsi/libcuda.so.1.1\n";
        assert_eq!(
            driver_dir_in_maps(maps),
            Some(PathBuf::from("/usr/lib/wsl/drivers/nv dispsi"))
        );
    }
}
