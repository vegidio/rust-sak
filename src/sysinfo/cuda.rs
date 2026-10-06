//! What the installed NVIDIA driver offers CUDA: the newest CUDA version it supports, and the compute capability and
//! total memory of every device it can run CUDA on.
//!
//! The answer comes from the CUDA driver API itself — `nvcuda.dll` on Windows, `libcuda.so.1` on Linux — which is the
//! library every CUDA runtime loads to reach the GPU. Asking it rather than NVML or a table of model names means the
//! answer is the one a CUDA program will meet: a device this reports is one `cuInit` can see, under the driver that
//! will actually run it.
//!
//! Only [`cuda_info`] is platform-specific. The decoding is compiled everywhere so it stays testable from any host.

/// What the NVIDIA driver on this machine offers CUDA.
///
/// Returned by [`cuda_info`]. Versions are `(major, minor)` pairs, so they compare the way versions do:
/// `(12, 8) < (13, 0)` and `(6, 1) < (7, 5)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaInfo {
    /// The newest CUDA version the installed driver supports, e.g. `(13, 0)` for a 580-series driver.
    ///
    /// This is the driver's ceiling, not a toolkit that is installed: a CUDA runtime of this major version or older
    /// runs on it, and a newer major version refuses to.
    pub driver_version: (u32, u32),
    /// Every device the driver can run CUDA on, in CUDA's own device order.
    ///
    /// Empty when the driver is installed but cannot initialise — no NVIDIA GPU present, one the driver does not
    /// support, or `CUDA_VISIBLE_DEVICES` hiding every device.
    pub devices: Vec<CudaDevice>,
}

/// One device the CUDA driver can run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaDevice {
    /// The device's name as CUDA reports it, e.g. `"NVIDIA GeForce GTX 1060"`.
    pub name: String,
    /// The device's compute capability, e.g. `(6, 1)` for Pascal or `(8, 9)` for Ada Lovelace.
    ///
    /// This is what a CUDA library build targets, and so what decides whether it can run here at all: CUDA 13, for
    /// one, needs `(7, 5)` or newer.
    pub compute_capability: (u32, u32),
    /// The device's total memory in **bytes**, as `cuDeviceTotalMem` reports it, or `None` when the driver did not
    /// answer.
    ///
    /// This is the whole framebuffer, fixed for the life of the device — not what is free. It is read without creating
    /// a CUDA context, so asking costs no device memory, and it is the figure a CUDA program sizes itself against on
    /// every platform, including Linux, where `GpuInfo::memory` cannot see an NVIDIA card's VRAM.
    pub total_memory: Option<u64>,
}

/// Decodes the integer `cuDriverGetVersion` reports, `1000 × major + 10 × minor`, into `(major, minor)`.
///
/// `12080` is CUDA 12.8 and `13000` is CUDA 13.0. Zero or a negative value is not a version any driver reports.
pub(super) fn version_from_driver(raw: i32) -> Option<(u32, u32)> {
    let raw = u32::try_from(raw).ok().filter(|raw| *raw > 0)?;

    Some((raw / 1000, raw % 1000 / 10))
}

/// Assembles a [`CudaDevice`] from what the driver reported for one device.
///
/// A device with no name, or with a compute capability that is not a pair of non-negative numbers, is dropped: it
/// cannot be told apart from a driver that answered badly. A zero memory figure becomes `None`, as it does on
/// `GpuInfo`: no device CUDA runs on has no memory.
pub(super) fn device_from_parts(name: &str, major: i32, minor: i32, total_memory: Option<u64>) -> Option<CudaDevice> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    Some(CudaDevice {
        name: name.to_string(),
        compute_capability: (u32::try_from(major).ok()?, u32::try_from(minor).ok()?),
        total_memory: total_memory.filter(|bytes| *bytes > 0),
    })
}

/// Asks the NVIDIA driver what it offers CUDA on this machine.
///
/// **`None` is a normal answer, not a failure.** It means there is no CUDA driver to ask: no NVIDIA driver is
/// installed, it is too old to report a version, or this is macOS or another platform CUDA does not run on. A driver
/// that is present but cannot initialise returns `Some` with no [`devices`](CudaInfo::devices), so a caller can tell
/// "no driver" from "a driver that sees no usable GPU".
///
/// Every call initialises the CUDA driver, which costs tens to hundreds of milliseconds the first time, so hold onto
/// the answer rather than asking repeatedly. **The driver stays loaded** for the rest of the process: GPU drivers are
/// not written to be unloaded, and a CUDA program that goes on to use it would load it again anyway.
///
/// ```no_run
/// use rust_sak::sysinfo::cuda_info;
///
/// match cuda_info() {
///     Some(cuda) => {
///         println!("driver supports CUDA {}.{}", cuda.driver_version.0, cuda.driver_version.1);
///         for device in &cuda.devices {
///             let (major, minor) = device.compute_capability;
///             let memory = device.total_memory.unwrap_or(0) >> 20;
///             println!("{} — compute capability {major}.{minor}, {memory} MiB", device.name);
///         }
///     }
///     None => println!("no CUDA driver"),
/// }
/// ```
pub fn cuda_info() -> Option<CudaInfo> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        imp::cuda_info()
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod imp {
    use std::ffi::{c_char, c_int, c_uint};

    use libloading::{Library, Symbol};

    use super::super::gpu_nvml::device_name;
    use super::{CudaDevice, CudaInfo, device_from_parts, version_from_driver};

    /// `CUDA_SUCCESS`. Every other `CUresult` is some failure, and none of them is worth telling apart here.
    const SUCCESS: c_int = 0;

    /// `CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR`.
    const COMPUTE_CAPABILITY_MAJOR: c_int = 75;

    /// `CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR`.
    const COMPUTE_CAPABILITY_MINOR: c_int = 76;

    /// Room for any name `cuDeviceGetName` writes, terminator included. The driver truncates rather than overruns.
    const NAME_BUFFER_SIZE: usize = 256;

    /// `CUdevice`, which `cuda.h` declares as a plain `int` ordinal.
    type Device = c_int;

    pub(super) fn cuda_info() -> Option<CudaInfo> {
        let library = load()?;

        // SAFETY: each symbol is declared with the signature `cuda.h` gives it, and none outlives `library`.
        let info = unsafe { query(&library) };

        // Never unloaded: `cuInit` has brought the driver up, and a driver is not written to be torn down again.
        std::mem::forget(library);

        info
    }

    /// Loads the driver from the system directory only.
    ///
    /// Restricting the search is what keeps a stray `nvcuda.dll` beside the executable or on `PATH` — where a program
    /// shipping its own CUDA libraries puts its directories — from being asked in the real driver's place.
    #[cfg(target_os = "windows")]
    fn load() -> Option<Library> {
        use libloading::os::windows::{LOAD_LIBRARY_SEARCH_SYSTEM32, Library as WindowsLibrary};

        // SAFETY: loading the driver runs its initialisers, which it ships precisely for a process to load it.
        unsafe { WindowsLibrary::load_with_flags("nvcuda.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .ok()
            .map(Library::from)
    }

    /// Loads the driver by soname, then from WSL's driver directory for a distribution that has turned off WSL's
    /// `ldconfig` integration. Under WSL2 the library is a stub that `cuInit` makes load the real Windows driver.
    #[cfg(target_os = "linux")]
    fn load() -> Option<Library> {
        const LIBRARIES: &[&str] = &["libcuda.so.1", "/usr/lib/wsl/lib/libcuda.so.1"];

        // SAFETY: loading the driver runs its initialisers, which it ships precisely for a process to load it.
        LIBRARIES.iter().find_map(|path| unsafe { Library::new(path) }.ok())
    }

    /// Reads the driver's version, initialises it and reads every device.
    ///
    /// `extern "system"` because `cuda.h` declares every entry point `CUDAAPI`, which is `__stdcall` on Windows —
    /// the same as the C convention everywhere this is compiled except 32-bit Windows, where it matters.
    ///
    /// # Safety
    ///
    /// `library` must be the CUDA driver.
    unsafe fn query(library: &Library) -> Option<CudaInfo> {
        // SAFETY: `cuDriverGetVersion` takes an `int *` and returns a `CUresult`, as `cuda.h` declares it.
        let driver_version: Symbol<unsafe extern "system" fn(*mut c_int) -> c_int> =
            unsafe { library.get(b"cuDriverGetVersion\0") }.ok()?;

        let mut raw_version: c_int = 0;
        // SAFETY: `raw_version` is a valid out-pointer for the duration of the call. The version needs no `cuInit`.
        if unsafe { driver_version(&mut raw_version) } != SUCCESS {
            return None;
        }

        Some(CudaInfo {
            driver_version: version_from_driver(raw_version)?,
            // SAFETY: as this function's own contract.
            devices: unsafe { devices(library) }.unwrap_or_default(),
        })
    }

    /// Initialises the driver and reads every device it reports.
    ///
    /// `None` when initialisation fails — the driver is up to date with no GPU it supports, say — which the caller
    /// reports as a driver with no devices rather than no driver.
    ///
    /// # Safety
    ///
    /// `library` must be the CUDA driver.
    unsafe fn devices(library: &Library) -> Option<Vec<CudaDevice>> {
        // SAFETY: the signatures below are the ones `cuda.h` declares for these entry points.
        let (init, count, get, name, attribute, total_memory) = unsafe {
            let init: Symbol<unsafe extern "system" fn(c_uint) -> c_int> = library.get(b"cuInit\0").ok()?;
            let count: Symbol<unsafe extern "system" fn(*mut c_int) -> c_int> =
                library.get(b"cuDeviceGetCount\0").ok()?;
            let get: Symbol<unsafe extern "system" fn(*mut Device, c_int) -> c_int> =
                library.get(b"cuDeviceGet\0").ok()?;
            let name: Symbol<unsafe extern "system" fn(*mut c_char, c_int, Device) -> c_int> =
                library.get(b"cuDeviceGetName\0").ok()?;
            let attribute: Symbol<unsafe extern "system" fn(*mut c_int, c_int, Device) -> c_int> =
                library.get(b"cuDeviceGetAttribute\0").ok()?;
            // `_v2`, which is what `cuda.h` has mapped `cuDeviceTotalMem` to since CUDA 3.2: the unsuffixed export
            // takes an `unsigned int`, which a card past 4 GiB overflows. Optional, so a driver without it still
            // reports its devices.
            let total_memory: Option<Symbol<unsafe extern "system" fn(*mut usize, Device) -> c_int>> =
                library.get(b"cuDeviceTotalMem_v2\0").ok();

            (init, count, get, name, attribute, total_memory)
        };

        // SAFETY: `0` is the only flag value `cuInit` accepts.
        if unsafe { init(0) } != SUCCESS {
            return None;
        }

        let mut devices: c_int = 0;
        // SAFETY: `devices` is a valid out-pointer for the duration of the call.
        if unsafe { count(&mut devices) } != SUCCESS {
            return None;
        }

        Some(
            (0..devices)
                .filter_map(|ordinal| {
                    let mut device: Device = 0;
                    // SAFETY: `device` is a valid out-pointer, and `ordinal` is below the count the driver reported.
                    if unsafe { get(&mut device, ordinal) } != SUCCESS {
                        return None;
                    }

                    let mut buffer = [0_u8; NAME_BUFFER_SIZE];
                    // SAFETY: `device` came from the driver, and `buffer` holds the `len` bytes it is told it may
                    // write.
                    if unsafe { name(buffer.as_mut_ptr().cast(), NAME_BUFFER_SIZE as c_int, device) } != SUCCESS {
                        return None;
                    }

                    let read = |which| {
                        let mut value: c_int = 0;
                        // SAFETY: `value` is a valid out-pointer, and `device` came from the driver.
                        (unsafe { attribute(&mut value, which, device) } == SUCCESS).then_some(value)
                    };

                    let memory = total_memory.as_ref().and_then(|total_memory| {
                        let mut bytes: usize = 0;
                        // SAFETY: `bytes` is a valid out-pointer, and `device` came from the driver.
                        (unsafe { total_memory(&mut bytes, device) } == SUCCESS).then_some(bytes as u64)
                    });

                    device_from_parts(
                        &device_name(&buffer),
                        read(COMPUTE_CAPABILITY_MAJOR)?,
                        read(COMPUTE_CAPABILITY_MINOR)?,
                        memory,
                    )
                })
                .collect(),
        )
    }
}
