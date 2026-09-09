use std::fmt;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, SysinfoError>;

/// An error produced while probing the machine's hardware.
///
/// Only [`gpu_info`](super::gpu_info) can fail. CPU and memory information comes from the
/// [`sysinfo`](https://crates.io/crates/sysinfo) crate, which reports "unknown" in-band rather than failing, so
/// [`cpu_info`](super::cpu_info) and [`memory_info`](super::memory_info) return their values directly.
///
/// **An empty GPU list is not an error.** A headless server, a virtual machine with no display adapter, or a
/// container without `/sys` mounted all yield `Ok(vec![])`. These variants report a probe that *could not run*,
/// never a probe that ran and found nothing.
#[derive(Debug)]
pub enum SysinfoError {
    /// Enumerating the graphics devices in the filesystem failed.
    ///
    /// Linux only: reading `/sys/class/drm` was refused. A *missing* `/sys/class/drm` is not an error — a machine
    /// with no DRM subsystem genuinely has no cards to report.
    Io(std::io::Error),
    /// An operating-system graphics call returned a failure status.
    GpuApi {
        /// The entry point that failed, e.g. `"IOServiceGetMatchingServices"` or `"CreateDXGIFactory1"`.
        call: &'static str,
        /// The status it returned: a `kern_return_t` on macOS, an `HRESULT` on Windows.
        code: i32,
    },
    /// GPU detection has no implementation for this operating system.
    UnsupportedPlatform {
        /// The operating system, as [`std::env::consts::OS`] names it.
        os: &'static str,
    },
}

impl fmt::Display for SysinfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SysinfoError::Io(err) => write!(f, "reading the graphics devices failed: {err}"),
            SysinfoError::GpuApi { call, code } => write!(f, "{call} failed with status {code:#010x}"),
            SysinfoError::UnsupportedPlatform { os } => write!(f, "gpu detection is not supported on {os}"),
        }
    }
}

impl std::error::Error for SysinfoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SysinfoError::Io(err) => Some(err),
            SysinfoError::GpuApi { .. } | SysinfoError::UnsupportedPlatform { .. } => None,
        }
    }
}

impl From<std::io::Error> for SysinfoError {
    fn from(err: std::io::Error) -> Self {
        SysinfoError::Io(err)
    }
}
