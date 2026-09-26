//! The WebGPU probe must leave every library it loaded in place.
//!
//! A file of its own, holding a single test, because what it measures is process-wide: the dynamic loader's count of
//! objects ever unloaded. Any other test running beside it — `gpu_info` closes NVML when it is done — would move the
//! same counter and make the result mean nothing.
//!
//! On a machine without the Vulkan loader the probe loads nothing and the test passes trivially. It only has something
//! to catch where `libvulkan.so.1` and a driver are installed; Mesa's `lavapipe` is enough, no GPU is needed.

#![cfg(all(feature = "sysinfo", target_os = "linux"))]

use std::ffi::c_void;

use rust_sak::sysinfo::is_webgpu_supported;

/// How many shared objects the process has unloaded so far, from glibc's `dlpi_subs`.
fn unloads() -> u64 {
    unsafe extern "C" fn read(info: *mut libc::dl_phdr_info, _size: libc::size_t, data: *mut c_void) -> libc::c_int {
        // SAFETY: `data` is the `u64` passed below, and `info` is valid for the duration of the callback.
        unsafe { *data.cast::<u64>() = (*info).dlpi_subs };

        // The counter is the same for every object, so the first one is enough.
        1
    }

    let mut subs: u64 = 0;
    // SAFETY: `read` only reads `info` and writes into `subs`, which outlives the call.
    unsafe { libc::dl_iterate_phdr(Some(read), (&raw mut subs).cast()) };

    subs
}

#[test]
fn the_probe_unloads_nothing_it_loaded() {
    let before = unloads();

    let _ = is_webgpu_supported();

    // A driver unloaded here leaves its thread-local destructors pointing into unmapped memory; under WSL2 that is a
    // segfault the next time a thread exits.
    assert_eq!(unloads(), before, "the WebGPU probe unloaded a library it had loaded");
}
