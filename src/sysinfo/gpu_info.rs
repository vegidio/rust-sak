use super::error::Result;
use super::gpu::GpuInfo;

/// Enumerates the graphics adapters attached to this machine.
///
/// Each platform is asked through its own interface: the IORegistry on macOS, DXGI on Windows, and the DRM entries
/// under `/sys/class/drm` on Linux. Nothing here spawns a process.
///
/// **An empty list is a normal answer, not a failure.** A headless server, a virtual machine with no display
/// adapter and a container without `/sys` mounted all legitimately have no GPU to report.
///
/// How much is known varies by platform and driver, which is why [`GpuInfo::vendor`] and [`GpuInfo::memory`] are
/// optional. In particular [`memory`](GpuInfo::memory) is `None` for every Apple Silicon GPU, which shares the
/// machine's memory rather than owning any, and for every Linux GPU that is not driven by `amdgpu` — Intel,
/// `nouveau` and the proprietary NVIDIA driver publish no VRAM figure in `sysfs`.
///
/// On Linux the `pci.ids` database used to resolve model names is read once per process, not once per call.
///
/// ```no_run
/// use rust_sak::sysinfo::gpu_info;
///
/// for gpu in gpu_info()? {
///     match gpu.memory {
///         Some(bytes) => println!("{} — {} MiB", gpu.name, bytes / (1024 * 1024)),
///         None => println!("{} — shared or unreported memory", gpu.name),
///     }
/// }
/// # Ok::<(), rust_sak::sysinfo::SysinfoError>(())
/// ```
///
/// # Errors
///
/// Returns [`SysinfoError::GpuApi`](super::SysinfoError::GpuApi) if an operating-system graphics call fails,
/// [`SysinfoError::Io`](super::SysinfoError::Io) if `/sys/class/drm` exists but cannot be read, and
/// [`SysinfoError::UnsupportedPlatform`](super::SysinfoError::UnsupportedPlatform) on an operating system this
/// module has no implementation for.
pub fn gpu_info() -> Result<Vec<GpuInfo>> {
    #[cfg(target_os = "linux")]
    {
        use std::path::Path;

        use super::gpu_sysfs::{DRM_CLASS_DIR, gpus_from_drm_dir};
        use super::pci_ids::PCI_IDS;

        gpus_from_drm_dir(Path::new(DRM_CLASS_DIR), PCI_IDS.as_deref())
    }

    #[cfg(target_os = "macos")]
    {
        super::gpu_ioreg::gpus()
    }

    #[cfg(target_os = "windows")]
    {
        super::gpu_dxgi::gpus()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(super::error::SysinfoError::UnsupportedPlatform {
            os: std::env::consts::OS,
        })
    }
}
