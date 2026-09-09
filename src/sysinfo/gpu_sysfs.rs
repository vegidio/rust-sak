//! GPU detection on Linux, through the DRM entries in `sysfs`.
//!
//! Linux has no library call that answers "what graphics hardware is here". The kernel publishes the answer as a
//! virtual filesystem instead, so everything below is ordinary file reading — the same source `lspci` ultimately
//! reads, rather than a parse of its output.
//!
//! Only the [`gpus_from_drm_dir`] entry point is Linux-specific, and only because of the path it defaults to.
//! Every function here is compiled on all targets and takes its root directory as an argument, so the tests
//! exercise this backend against a fabricated tree from any host.

use std::fs;
use std::path::Path;

use super::error::Result;
use super::gpu::GpuInfo;
use super::pci_ids::lookup_pci_ids;
use super::vendor::{gpu_from_driver, vendor_name};

/// The directory in which the kernel publishes one entry per DRM device.
pub(super) const DRM_CLASS_DIR: &str = "/sys/class/drm";

/// The PCI base class that means "display controller".
const PCI_CLASS_DISPLAY: u32 = 0x03;

/// Enumerates every graphics card under a DRM class directory.
///
/// `pci_ids` is the contents of a `pci.ids` database, read once by the caller and shared across every card so that
/// a machine with several GPUs does not read the same 1.5 MB file repeatedly.
///
/// A missing directory yields an empty list rather than an error: a machine with no DRM subsystem — a container
/// without `/sys` mounted, say — genuinely has no cards to report.
///
/// # Errors
///
/// Returns [`SysinfoError::Io`](super::SysinfoError::Io) if the directory exists but cannot be read.
pub(super) fn gpus_from_drm_dir(drm: &Path, pci_ids: Option<&str>) -> Result<Vec<GpuInfo>> {
    let entries = match fs::read_dir(drm) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };

    let mut cards: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| is_card_name(&entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect();

    // `read_dir` order is whatever the filesystem hands back; sort so `card0` reliably precedes `card1`.
    cards.sort();

    Ok(cards
        .iter()
        .filter_map(|card| gpu_from_card_dir(card, pci_ids))
        .collect())
}

/// Reports whether a `/sys/class/drm` entry names a graphics card.
///
/// The directory holds three other kinds of entry that must not be mistaken for one: connectors, named
/// `card0-DP-1` after the card they hang off; render nodes, named `renderD128`; and a plain `version` file. Only
/// `card` followed by digits and nothing else is a card.
///
/// The digits are not range-checked. Minor numbers below 64 are the usual case but the kernel falls back to a much
/// wider range once those are exhausted, so `card200` is legal on a machine with many GPUs.
pub(super) fn is_card_name(name: &str) -> bool {
    let Some(minor) = name.strip_prefix("card") else {
        return false;
    };

    !minor.is_empty() && minor.bytes().all(|byte| byte.is_ascii_digit())
}

/// Builds a [`GpuInfo`] from one `/sys/class/drm/cardN` directory, or `None` if it is not a GPU.
///
/// Two shapes of device arrive here. A PCI GPU carries `vendor`, `device` and `class` files, and is accepted when
/// its class marks it a display controller. A system-on-chip GPU carries none of those, and is accepted only when
/// the driver bound to it is one this module recognises — which is what keeps framebuffer stubs like `simpledrm`
/// out of the results.
pub(super) fn gpu_from_card_dir(card: &Path, pci_ids: Option<&str>) -> Option<GpuInfo> {
    let device = card.join("device");
    let vendor_id = read_attribute(&device, "vendor").as_deref().and_then(parse_hex_id);
    let device_id = read_attribute(&device, "device").as_deref().and_then(parse_hex_id);

    let Some((vendor_id, device_id)) = vendor_id.zip(device_id) else {
        return soc_gpu(&device);
    };

    // A PCI device with no readable class is not trustworthy enough to report as a GPU.
    let class = read_attribute(&device, "class").as_deref().and_then(parse_class)?;
    if !is_display_class(class) {
        return None;
    }

    let names = pci_ids
        .map(|contents| lookup_pci_ids(contents, vendor_id, device_id))
        .unwrap_or_default();
    let vendor = vendor_name(vendor_id).map(str::to_string).or(names.vendor);

    // Without a database there is no model name to be had, so the vendor and the raw device id are the most
    // informative thing that can honestly be said.
    let name = names.device.unwrap_or_else(|| match &vendor {
        Some(vendor) => format!("{vendor} Device {device_id:04x}"),
        None => format!("Device {vendor_id:04x}:{device_id:04x}"),
    });

    Some(GpuInfo {
        name,
        vendor,
        memory: read_attribute(&device, "mem_info_vram_total")
            .as_deref()
            .and_then(parse_vram),
    })
}

/// Builds a [`GpuInfo`] for a GPU with no PCI identity, from the driver bound to it.
fn soc_gpu(device: &Path) -> Option<GpuInfo> {
    let driver = driver_name(device)?;
    let (name, vendor) = gpu_from_driver(&driver)?;

    Some(GpuInfo {
        name: name.to_string(),
        vendor: Some(vendor.to_string()),
        // A SoC GPU shares the machine's memory; it has no VRAM of its own to report.
        memory: None,
    })
}

/// The name of the kernel driver bound to a device, from the `driver` symlink it grows when one claims it.
fn driver_name(device: &Path) -> Option<String> {
    let target = fs::read_link(device.join("driver")).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

/// Reads one sysfs attribute, treating every failure as absence.
///
/// Attributes come and go with the driver — `mem_info_vram_total` disappears when an AMD card's VRAM manager is
/// not in use — so a missing file means "unknown", never an error.
fn read_attribute(device: &Path, name: &str) -> Option<String> {
    fs::read_to_string(device.join(name)).ok()
}

/// Parses a sysfs PCI id file, which the kernel formats as `0x%04x` followed by a newline.
pub(super) fn parse_hex_id(raw: &str) -> Option<u16> {
    let digits = raw.trim().strip_prefix("0x")?;
    u16::from_str_radix(digits, 16).ok()
}

/// Parses a sysfs `class` file, which the kernel formats as `0x%06x` followed by a newline.
///
/// Both the `0x` prefix and all six digits are required. `uevent` exposes the same number as `PCI_CLASS`, but
/// unprefixed and unpadded — a display controller prints `30000` there rather than `0x030000` — so that field
/// cannot be fed to this function by mistake.
pub(super) fn parse_class(raw: &str) -> Option<u32> {
    let digits = raw.trim().strip_prefix("0x")?;
    if digits.len() != 6 {
        return None;
    }

    u32::from_str_radix(digits, 16).ok()
}

/// Reports whether a PCI class code belongs to a display controller.
///
/// The base class is the top byte of the 24-bit code. Every subclass is accepted: `0x0300` (VGA compatible),
/// `0x0302` (3D controller, which is how discrete NVIDIA cards in laptops present) and `0x0380` (other) are all
/// genuinely GPUs, and this is the same filter `lspci` users apply by hand.
pub(super) fn is_display_class(class: u32) -> bool {
    class >> 16 == PCI_CLASS_DISPLAY
}

/// Parses `mem_info_vram_total`, which `amdgpu` writes as a plain decimal byte count.
///
/// Zero becomes `None`. The field is only ever published by `amdgpu`, so a card reporting zero bytes of VRAM is
/// telling us the manager is not up rather than that the card has no memory.
pub(super) fn parse_vram(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|bytes| *bytes > 0)
}
