//! Turning hardware identifiers into names.
//!
//! Two problems live here. PCI devices report a numeric vendor ID that has to become a readable name, and non-PCI
//! GPUs — the ones soldered into an ARM system-on-chip — report no PCI identity at all and can only be recognised
//! by which kernel driver claimed them.

/// The PCI vendor IDs worth naming without consulting a database.
///
/// These names are deliberately shorter than the ones `pci.ids` records: that file calls AMD
/// `"Advanced Micro Devices, Inc. [AMD/ATI]"`, which is accurate and useless in a UI. `pci.ids` is still consulted
/// for *device* names, where its detail is exactly what is wanted.
const VENDORS: &[(u16, &str)] = &[
    (0x1002, "AMD"),
    (0x106b, "Apple"),
    (0x10de, "NVIDIA"),
    (0x1234, "QEMU"),
    (0x13b5, "ARM"),
    (0x1414, "Microsoft"),
    (0x14e4, "Broadcom"),
    (0x15ad, "VMware"),
    (0x17cb, "Qualcomm"),
    (0x1af4, "Red Hat"),
    (0x1b36, "Red Hat"),
    (0x1de4, "Raspberry Pi"),
    (0x5143, "Qualcomm"),
    (0x8086, "Intel"),
];

/// The DRM drivers that claim a real, non-PCI GPU, paired with the vendor behind each.
///
/// This is an allowlist rather than a denylist on purpose. A `card` entry with no PCI identity is not necessarily a
/// GPU: `simpledrm` (the EFI framebuffer stub, present on nearly every UEFI Linux boot and permanently on many
/// servers), `vkms` (a virtual test device) and the USB display drivers `udl` and `evdi` all appear the same way. A
/// denylist would let each newly added one through as a phantom GPU; this cannot.
const SOC_DRIVERS: &[(&str, &str, &str)] = &[
    ("asahi", "Apple GPU", "Apple"),
    ("lima", "Mali GPU", "ARM"),
    ("msm", "Adreno GPU", "Qualcomm"),
    ("panfrost", "Mali GPU", "ARM"),
    ("panthor", "Mali GPU", "ARM"),
    ("v3d", "VideoCore GPU", "Broadcom"),
    ("vc4", "VideoCore GPU", "Broadcom"),
];

/// The marketing names that identify a vendor when only a description string is available.
///
/// Ordered so that the more specific match wins: `"ATI"` and `"Radeon"` both mean AMD, and a description that says
/// `"AMD Radeon"` must not be matched twice with different answers.
const DESCRIPTIONS: &[(&str, &str)] = &[
    ("nvidia", "NVIDIA"),
    ("geforce", "NVIDIA"),
    ("quadro", "NVIDIA"),
    ("amd", "AMD"),
    ("radeon", "AMD"),
    ("ati ", "AMD"),
    ("intel", "Intel"),
    ("apple", "Apple"),
    ("microsoft", "Microsoft"),
    ("vmware", "VMware"),
    ("virtio", "Red Hat"),
    ("qemu", "QEMU"),
    ("qualcomm", "Qualcomm"),
    ("adreno", "Qualcomm"),
    ("mali", "ARM"),
    ("videocore", "Broadcom"),
];

/// The canonical short name for a PCI vendor ID, or `None` if it is not one this module knows.
pub(super) fn vendor_name(id: u16) -> Option<&'static str> {
    VENDORS.iter().find(|(known, _)| *known == id).map(|(_, name)| *name)
}

/// Infers a vendor from a marketing name, for the platforms that report a description but no usable vendor ID.
///
/// Matching is case-insensitive and substring-based, so `"NVIDIA GeForce RTX 3090"` and
/// `"Intel(R) Arc(TM) A770"` both resolve.
pub(super) fn vendor_from_description(description: &str) -> Option<&'static str> {
    let haystack = description.to_ascii_lowercase();
    DESCRIPTIONS
        .iter()
        .find(|(needle, _)| haystack.contains(needle))
        .map(|(_, vendor)| *vendor)
}

/// The name and vendor of a non-PCI SoC GPU, identified by the DRM driver bound to it.
///
/// Returns `None` for every driver not in [`SOC_DRIVERS`], which is what excludes framebuffer stubs and virtual
/// devices from the results.
pub(super) fn gpu_from_driver(driver: &str) -> Option<(&'static str, &'static str)> {
    SOC_DRIVERS
        .iter()
        .find(|(known, _, _)| *known == driver)
        .map(|(_, name, vendor)| (*name, *vendor))
}
