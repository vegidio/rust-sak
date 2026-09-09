/// A graphics adapter detected on this machine.
///
/// Returned by [`gpu_info`](super::gpu_info). Both optional fields distinguish *"the platform says this does not
/// apply"* from a zero: an Apple Silicon GPU has no VRAM of its own because memory is unified, which is not the
/// same as having none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    /// The adapter's model name, e.g. `"Apple M2 Max"` or `"NVIDIA GeForce RTX 3090"`.
    pub name: String,
    /// The canonical vendor name, e.g. `"NVIDIA"`, `"AMD"`, `"Intel"` or `"Apple"`.
    ///
    /// `None` when the vendor could not be identified — an unrecognised PCI vendor ID on a machine with no
    /// `pci.ids` database to consult.
    pub vendor: Option<String>,
    /// Dedicated video memory in **bytes**, or `None` when the platform does not expose it.
    ///
    /// `None` is common and expected rather than a failure: Apple Silicon uses unified memory, and Linux `sysfs`
    /// only reports VRAM for `amdgpu` — Intel, `nouveau` and the proprietary NVIDIA driver expose nothing. This is
    /// never `Some(0)`.
    pub memory: Option<u64>,
}
