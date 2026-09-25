//! NVIDIA GPU detection on Linux, through NVML, for the machines `sysfs` cannot see an NVIDIA card on.
//!
//! The DRM entries under `/sys/class/drm` only exist for a card some DRM driver has claimed, and two common setups
//! have an NVIDIA GPU that no DRM driver claims:
//!
//! - **WSL2**, where the GPU is paravirtualised through `/dev/dxg` and Linux never sees a PCI display device at all.
//! - **The proprietary driver without `nvidia-drm`**, which is how headless and compute-only machines and most
//!   containers run it: `nvidia.ko` drives the card, but nothing registers it with DRM.
//!
//! Both still ship NVML (`libnvidia-ml.so.1`) with the driver — WSL puts it in `/usr/lib/wsl/lib` — so asking it is
//! the one probe that covers every machine CUDA itself can run on. It is loaded at runtime rather than linked, because
//! a machine with no NVIDIA driver has no NVML and must still be able to load this crate.
//!
//! Only [`gpus`] is Linux-specific. The decoding and the merge rule are compiled everywhere so they stay testable
//! from any host.

use super::gpu::GpuInfo;

/// The vendor every adapter NVML reports belongs to, spelled as [`vendor_name`](super::vendor::vendor_name) spells
/// it for PCI id `0x10de`.
pub(super) const NVIDIA: &str = "NVIDIA";

/// Decodes the name `nvmlDeviceGetName` wrote into a fixed buffer.
///
/// The buffer holds a NUL-terminated string, so everything from the terminator onward is padding and must not be
/// decoded. A buffer with no terminator at all is used whole.
pub(super) fn device_name(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());

    String::from_utf8_lossy(&buffer[..end]).trim().to_string()
}

/// Assembles a [`GpuInfo`] from what NVML reported for one device.
///
/// A device with no name is dropped. NVML reports the card's total framebuffer, which is the dedicated VRAM this type
/// documents; a zero becomes `None`, as everywhere else.
pub(super) fn gpu_from_device(name: &str, total_memory: Option<u64>) -> Option<GpuInfo> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    Some(GpuInfo {
        name: name.to_string(),
        vendor: Some(NVIDIA.to_string()),
        memory: total_memory.filter(|bytes| *bytes > 0),
    })
}

/// Adds the NVIDIA adapters `nvml` reports to the ones `sysfs` found, unless `sysfs` already found one.
///
/// `nvml` is only called when it is needed. A machine whose NVIDIA cards are already under `/sys/class/drm` — any
/// desktop running `nvidia-drm` — never loads the library, and so never reports a card twice.
pub(super) fn with_nvml_fallback(mut sysfs: Vec<GpuInfo>, nvml: impl FnOnce() -> Vec<GpuInfo>) -> Vec<GpuInfo> {
    if !sysfs.iter().any(|gpu| gpu.vendor.as_deref() == Some(NVIDIA)) {
        sysfs.extend(nvml());
    }

    sysfs
}

#[cfg(target_os = "linux")]
pub(super) use imp::gpus;

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::{c_char, c_uint, c_void};

    use libloading::{Library, Symbol};

    use super::super::gpu::GpuInfo;
    use super::{device_name, gpu_from_device};

    /// Where NVML may be, in probe order: the soname the dynamic loader resolves, then WSL's driver directory for a
    /// distribution that has turned off WSL's `ldconfig` integration.
    const LIBRARIES: &[&str] = &["libnvidia-ml.so.1", "/usr/lib/wsl/lib/libnvidia-ml.so.1"];

    /// `NVML_SUCCESS`. Every other `nvmlReturn_t` is some failure, and none of them is worth telling apart here.
    const SUCCESS: c_uint = 0;

    /// `NVML_DEVICE_NAME_V2_BUFFER_SIZE`, the largest name `nvmlDeviceGetName` writes, terminator included.
    const NAME_BUFFER_SIZE: usize = 96;

    /// An opaque `nvmlDevice_t`.
    type Device = *mut c_void;

    /// `nvmlMemory_t`, which every NVML version still accepts through the unversioned `nvmlDeviceGetMemoryInfo`.
    #[repr(C)]
    #[derive(Default)]
    struct Memory {
        total: u64,
        free: u64,
        used: u64,
    }

    /// Enumerates the NVIDIA GPUs the driver reports through NVML.
    ///
    /// Every failure — no NVML on this machine, a driver too old for one of the entry points, a device that will not
    /// answer — yields fewer adapters rather than an error. This is a fallback behind `sysfs`, and a machine with no
    /// NVIDIA driver is the normal case rather than a probe that could not run.
    pub(in super::super) fn gpus() -> Vec<GpuInfo> {
        // SAFETY: loading NVML runs its initialisers, which the driver ships precisely for a process to load it.
        let Some(library) = LIBRARIES.iter().find_map(|path| unsafe { Library::new(path) }.ok()) else {
            return Vec::new();
        };

        // SAFETY: each symbol is declared with the signature `nvml.h` gives it, and none outlives `library`.
        unsafe { enumerate(&library) }.unwrap_or_default()
    }

    /// Initialises NVML, reads every device and shuts it down again.
    ///
    /// # Safety
    ///
    /// `library` must be NVML.
    unsafe fn enumerate(library: &Library) -> Option<Vec<GpuInfo>> {
        // SAFETY: the signatures below are the ones `nvml.h` declares for these entry points.
        let (init, shutdown, count, handle, name, memory) = unsafe {
            let init: Symbol<unsafe extern "C" fn() -> c_uint> = library.get(b"nvmlInit_v2\0").ok()?;
            let shutdown: Symbol<unsafe extern "C" fn() -> c_uint> = library.get(b"nvmlShutdown\0").ok()?;
            let count: Symbol<unsafe extern "C" fn(*mut c_uint) -> c_uint> =
                library.get(b"nvmlDeviceGetCount_v2\0").ok()?;
            let handle: Symbol<unsafe extern "C" fn(c_uint, *mut Device) -> c_uint> =
                library.get(b"nvmlDeviceGetHandleByIndex_v2\0").ok()?;
            let name: Symbol<unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_uint> =
                library.get(b"nvmlDeviceGetName\0").ok()?;
            // Optional: an adapter is still worth reporting without its memory.
            let memory: Option<Symbol<unsafe extern "C" fn(Device, *mut Memory) -> c_uint>> =
                library.get(b"nvmlDeviceGetMemoryInfo\0").ok();

            (init, shutdown, count, handle, name, memory)
        };

        // SAFETY: `nvmlInit_v2` takes no arguments; a failure leaves nothing to shut down.
        if unsafe { init() } != SUCCESS {
            return None;
        }

        let mut devices: c_uint = 0;
        // SAFETY: `devices` is a valid out-pointer for the duration of the call.
        let gpus = if unsafe { count(&mut devices) } == SUCCESS {
            (0..devices)
                .filter_map(|index| {
                    let mut device: Device = std::ptr::null_mut();
                    // SAFETY: `device` is a valid out-pointer, and `index` is below the count NVML just reported.
                    if unsafe { handle(index, &mut device) } != SUCCESS {
                        return None;
                    }

                    let mut buffer = [0_u8; NAME_BUFFER_SIZE];
                    // SAFETY: `device` came from NVML, and `buffer` holds the `length` bytes it is told it may write.
                    if unsafe { name(device, buffer.as_mut_ptr().cast(), NAME_BUFFER_SIZE as c_uint) } != SUCCESS {
                        return None;
                    }

                    let total = memory.as_ref().and_then(|memory| {
                        let mut info = Memory::default();
                        // SAFETY: `device` came from NVML, and `info` is a valid `nvmlMemory_t` out-pointer.
                        (unsafe { memory(device, &mut info) } == SUCCESS).then_some(info.total)
                    });

                    gpu_from_device(&device_name(&buffer), total)
                })
                .collect()
        } else {
            Vec::new()
        };

        // SAFETY: balances the successful `nvmlInit_v2` above. Its status changes nothing that was already read.
        unsafe { shutdown() };

        Some(gpus)
    }
}
