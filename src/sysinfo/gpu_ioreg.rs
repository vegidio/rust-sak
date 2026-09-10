//! GPU detection on macOS, through the IORegistry.
//!
//! The kernel's live device tree is queried for `IOAccelerator` services, which is the class every Metal-capable
//! GPU registers under. This is the same data `system_profiler SPDisplaysDataType` prints, read through the API
//! that tool itself uses rather than by parsing its output.
//!
//! **Properties are read from the accelerator entry first, and only then from its parent.** Apple Silicon records
//! `model` and `vendor-id` directly on the accelerator, whose parent is an `IOPlatformExpertDevice` rather than a
//! PCI device; an implementation that insists on a PCI parent finds no GPU at all on those machines. Intel Macs
//! put the same properties on the `IOPCIDevice` parent, so both are consulted.
//!
//! Only [`gpus`] is macOS-specific. The decoding below is compiled everywhere so it stays testable from any host.

use super::gpu::GpuInfo;

/// Decodes an IORegistry `vendor-id`, which is stored as little-endian bytes rather than a number.
///
/// Only the low two bytes carry the PCI vendor; the property is four bytes wide with the rest zeroed. Apple's own
/// GPUs report `6b 10 00 00`, which is `0x106b`.
pub(super) fn vendor_id_from_bytes(bytes: &[u8]) -> Option<u16> {
    let [low, high, ..] = bytes else {
        return None;
    };

    Some(u16::from_le_bytes([*low, *high]))
}

/// Decodes a string stored in an `OSData` rather than an `OSString`.
///
/// Intel Macs record the GPU's `model` this way: a C string padded with trailing NUL bytes. Without this an Intel
/// Mac's discrete GPU has no name and is dropped from the results entirely.
pub(super) fn data_string(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    let text = std::str::from_utf8(&bytes[..end]).ok()?.trim();

    if text.is_empty() { None } else { Some(text.to_string()) }
}

/// Assembles a [`GpuInfo`] from the properties read off an accelerator entry or its parent.
///
/// A GPU with no model name at all is dropped: an entry that cannot be named is not worth reporting. The vendor
/// falls back to the model string when the numeric id is missing or unrecognised, which is how a machine reporting
/// only `"AMD Radeon Pro 5500M"` still resolves to `"AMD"`.
pub(super) fn gpu_from_properties(model: Option<&str>, vendor_id: Option<u16>, vram: Option<u64>) -> Option<GpuInfo> {
    GpuInfo::from_parts(model?, vendor_id, vram)
}

/// Converts a VRAM figure reported in mebibytes into bytes.
///
/// Intel Macs publish `VRAM,totalMB`, whose name is honest about the unit. Apple Silicon publishes neither this nor
/// its byte-valued sibling, because the GPU shares the machine's memory.
pub(super) fn vram_from_megabytes(megabytes: u64) -> u64 {
    megabytes * 1024 * 1024
}

#[cfg(target_os = "macos")]
pub(super) use imp::gpus;

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::{CFData, CFDictionary, CFNumber, CFRetained, CFString, kCFAllocatorDefault};
    use objc2_io_kit::{
        IOIteratorNext, IOObjectRelease, IORegistryEntryCreateCFProperty, IORegistryEntryGetParentEntry,
        IOServiceGetMatchingServices, IOServiceMatching, io_object_t, io_registry_entry_t, kIOMainPortDefault,
        kIOReturnSuccess, kIOServicePlane,
    };

    use super::super::error::{Result, SysinfoError};
    use super::super::gpu::GpuInfo;
    use super::{data_string, gpu_from_properties, vendor_id_from_bytes, vram_from_megabytes};

    /// The IOKit class every Metal-capable GPU registers under.
    const ACCELERATOR_CLASS: &std::ffi::CStr = c"IOAccelerator";

    /// Enumerates the machine's GPUs.
    ///
    /// # Errors
    ///
    /// Returns [`SysinfoError::GpuApi`] if IOKit refuses the service lookup.
    pub(in super::super) fn gpus() -> Result<Vec<GpuInfo>> {
        // SAFETY: `IOServiceMatching` is passed a valid NUL-terminated class name and returns either a matching
        // dictionary or null, which the `else` arm handles.
        let matching = unsafe { IOServiceMatching(ACCELERATOR_CLASS.as_ptr()) };
        let Some(matching) = matching else {
            return Err(SysinfoError::GpuApi {
                call: "IOServiceMatching",
                code: 0,
            });
        };

        // `IOServiceGetMatchingServices` consumes a reference to the dictionary. Converting through `CFRetained`
        // adds the one it will consume, leaving the original to be released when `matching` drops.
        let matching = CFRetained::<CFDictionary>::from(&*matching);

        let mut iterator: io_object_t = 0;
        // SAFETY: `iterator` is a valid out-pointer, and the dictionary reference is the one the call consumes.
        let status = unsafe { IOServiceGetMatchingServices(kIOMainPortDefault, Some(matching), &mut iterator) };
        if status != kIOReturnSuccess {
            return Err(SysinfoError::GpuApi {
                call: "IOServiceGetMatchingServices",
                code: status,
            });
        }

        let mut gpus = Vec::new();
        loop {
            let entry = IOIteratorNext(iterator);
            if entry == 0 {
                break;
            }

            if let Some(gpu) = gpu_from_entry(entry) {
                gpus.push(gpu);
            }

            IOObjectRelease(entry);
        }

        IOObjectRelease(iterator);

        Ok(gpus)
    }

    /// Reads one accelerator entry, consulting its parent for anything it does not carry itself.
    fn gpu_from_entry(entry: io_registry_entry_t) -> Option<GpuInfo> {
        let parent = parent_entry(entry);
        let entries: Vec<io_registry_entry_t> = std::iter::once(entry).chain(parent).collect();

        let model = entries.iter().find_map(|entry| string_property(*entry, "model"));
        let vendor_id = entries
            .iter()
            .find_map(|entry| data_property(*entry, "vendor-id"))
            .and_then(|bytes| vendor_id_from_bytes(&bytes));
        let vram = entries.iter().find_map(|entry| vram_property(*entry));

        let gpu = gpu_from_properties(model.as_deref(), vendor_id, vram);

        if let Some(parent) = parent {
            // `IORegistryEntryGetParentEntry` returns a retained object, so the reference is ours to drop.
            IOObjectRelease(parent);
        }

        gpu
    }

    /// The entry's parent in the service plane, or `None` when it has none.
    fn parent_entry(entry: io_registry_entry_t) -> Option<io_registry_entry_t> {
        let mut parent: io_registry_entry_t = 0;
        // SAFETY: `entry` is live, `kIOServicePlane` is a valid plane name and `parent` is a valid out-pointer.
        let status =
            unsafe { IORegistryEntryGetParentEntry(entry, kIOServicePlane.as_ptr().cast_mut().cast(), &mut parent) };

        (status == kIOReturnSuccess && parent != 0).then_some(parent)
    }

    /// Reads a property as a string, accepting both the `CFString` and NUL-padded `CFData` encodings.
    fn string_property(entry: io_registry_entry_t, key: &str) -> Option<String> {
        let value = property(entry, key)?;

        if let Some(text) = value.downcast_ref::<CFString>() {
            let text = text.to_string();
            let text = text.trim();
            return (!text.is_empty()).then(|| text.to_string());
        }

        data_string(value.downcast_ref::<CFData>()?.to_vec().as_slice())
    }

    /// Reads a property as raw bytes.
    fn data_property(entry: io_registry_entry_t, key: &str) -> Option<Vec<u8>> {
        Some(property(entry, key)?.downcast_ref::<CFData>()?.to_vec())
    }

    /// Reads the VRAM an entry reports, from whichever of the two keys it publishes.
    ///
    /// `VRAM,totalMB` is a number of mebibytes; `VRAM,totalsize` is a byte count that may arrive as either a number
    /// or raw bytes. Apple Silicon publishes neither.
    fn vram_property(entry: io_registry_entry_t) -> Option<u64> {
        if let Some(megabytes) = number_property(entry, "VRAM,totalMB") {
            return Some(vram_from_megabytes(megabytes));
        }

        if let Some(bytes) = number_property(entry, "VRAM,totalsize") {
            return Some(bytes);
        }

        let bytes = data_property(entry, "VRAM,totalsize")?;
        let mut buffer = [0_u8; 8];
        let len = bytes.len().min(8);
        buffer[..len].copy_from_slice(&bytes[..len]);

        Some(u64::from_le_bytes(buffer))
    }

    /// Reads a property as an unsigned number.
    fn number_property(entry: io_registry_entry_t, key: &str) -> Option<u64> {
        let value = property(entry, key)?;
        let number = value.downcast_ref::<CFNumber>()?.as_i64()?;

        u64::try_from(number).ok()
    }

    /// Reads a single property off an IORegistry entry.
    fn property(entry: io_registry_entry_t, key: &str) -> Option<CFRetained<objc2_core_foundation::CFType>> {
        let key = CFString::from_str(key);

        // SAFETY: `entry` is live and `key` is a valid `CFString` for the duration of the call.
        unsafe { IORegistryEntryCreateCFProperty(entry, Some(&key), kCFAllocatorDefault, 0) }
    }
}
