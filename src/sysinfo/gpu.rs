use super::vendor::{vendor_from_description, vendor_name};

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

impl GpuInfo {
    /// Builds an adapter from what a platform backend decoded, applying the conventions this type documents.
    ///
    /// An adapter whose name is empty or blank is dropped: one that cannot be named is not worth reporting. The
    /// vendor is resolved from the numeric PCI id first and falls back to sniffing the model string, which is how a
    /// machine reporting only `"AMD Radeon Pro 5500M"` still resolves to `"AMD"`. A zero memory figure becomes
    /// `None`, because integrated and unified-memory GPUs own no VRAM rather than owning none.
    ///
    /// The DXGI and IORegistry backends both build adapters this way. The `sysfs` backend does not: on Linux the
    /// `pci.ids` database gives a better vendor string than either source here, so it resolves its own.
    pub(super) fn from_parts(name: &str, vendor_id: Option<u16>, memory: Option<u64>) -> Option<GpuInfo> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }

        let vendor = vendor_id
            .and_then(vendor_name)
            .or_else(|| vendor_from_description(name))
            .map(str::to_string);

        Some(GpuInfo {
            name: name.to_string(),
            vendor,
            memory: memory.filter(|bytes| *bytes > 0),
        })
    }
}
