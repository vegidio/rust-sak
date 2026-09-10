//! Reading the `pci.ids` hardware database.
//!
//! Linux reports a GPU as a pair of 16-bit numbers. Turning `10de:2684` into `"GeForce RTX 4090"` needs the
//! `pci.ids` database that distributions ship, a plain text file maintained by the PCI ID Repository. It is not
//! always installed, so every lookup here degrades to `None` rather than failing.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Where a `pci.ids` database may live, in probe order.
///
/// The first two cover essentially every distribution — `hwdata` on Fedora, RHEL, openSUSE, Arch and Alpine, and
/// the `pci.ids` package on Debian and Ubuntu. The third is where `update-pciids` writes, which is often a *newer*
/// copy than the packaged one, and the fourth is NixOS, whose store paths are only reachable through the
/// current-system symlink.
pub(super) const PCI_IDS_PATHS: &[&str] = &[
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/var/lib/pciutils/pci.ids",
    "/run/current-system/sw/share/hwdata/pci.ids",
];

/// The largest `pci.ids` this module will read.
///
/// The real file is around 1.5 MB. The ceiling exists so that a path pointing at something enormous cannot be
/// turned into an allocation, in the same spirit as the response-body cap in [`o11y`](crate::o11y).
const MAX_PCI_IDS_BYTES: u64 = 16 * 1024 * 1024;

/// The names `pci.ids` records for one device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PciNames {
    /// The vendor's name as the database spells it, e.g. `"Advanced Micro Devices, Inc. [AMD/ATI]"`.
    pub(super) vendor: Option<String>,
    /// The device's name, e.g. `"GA102 [GeForce RTX 3090]"`.
    pub(super) device: Option<String>,
}

/// Reads the first `pci.ids` database found among `paths`.
///
/// Taking the candidate paths as an argument is what lets the tests point at a temporary tree instead of the real
/// `/usr/share`. Unreadable and oversized files are skipped rather than reported: a missing database is the normal
/// case on a minimal system, not an error.
pub(super) fn read_pci_ids_from(paths: &[&Path]) -> Option<String> {
    paths.iter().find_map(|path| {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_PCI_IDS_BYTES {
            return None;
        }

        fs::read_to_string(path).ok()
    })
}

/// Reads the first `pci.ids` database found among [`PCI_IDS_PATHS`].
fn read_pci_ids() -> Option<String> {
    let paths: Vec<&Path> = PCI_IDS_PATHS.iter().map(Path::new).collect();
    read_pci_ids_from(&paths)
}

/// The `pci.ids` database for this process, read at most once.
///
/// The file is around 1.5 MB and cannot meaningfully change while the process runs, so reading it afresh on every
/// [`gpu_info`](super::gpu_info) call was pure waste. `None` means no database was found, which is the normal case
/// on a minimal system. Tests call [`read_pci_ids_from`] directly to point at a temporary tree instead.
pub(super) static PCI_IDS: LazyLock<Option<String>> = LazyLock::new(read_pci_ids);

/// Looks a vendor and device pair up in a `pci.ids` database.
///
/// The format is indentation-significant. A vendor sits at column zero, its devices are indented by one tab, and a
/// device's subsystem variants by two — so a device line is only a match while the enclosing vendor is the one
/// being looked for, and subsystem lines are skipped entirely.
///
/// The file's second half is a device *class* database whose entries start with `C `. Those lines look enough like
/// vendor lines to corrupt a lookup, so the scan stops when it reaches them.
pub(super) fn lookup_pci_ids(contents: &str, vendor: u16, device: u16) -> PciNames {
    let mut names = PciNames::default();
    let mut in_vendor = false;

    for line in contents.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // The class database that forms the file's second half. Nothing after this point is a vendor.
        if line.starts_with("C ") {
            break;
        }

        // Two tabs mark a subsystem variant of the current device, which this lookup does not report.
        if line.starts_with("\t\t") {
            continue;
        }

        if let Some(entry) = line.strip_prefix('\t') {
            if !in_vendor {
                continue;
            }

            if let Some((id, name)) = split_entry(entry)
                && id == device
            {
                names.device = Some(name.to_string());
                break;
            }

            continue;
        }

        // Column zero: a vendor. Reaching the next one means the target vendor's devices are behind us.
        if in_vendor {
            break;
        }

        if let Some((id, name)) = split_entry(line)
            && id == vendor
        {
            names.vendor = Some(name.to_string());
            in_vendor = true;
        }
    }

    names
}

/// Splits a `pci.ids` entry into its hexadecimal id and its name.
///
/// Both vendor and device lines are formatted as four hex digits, two spaces, then the name.
fn split_entry(entry: &str) -> Option<(u16, &str)> {
    let (id, name) = entry.split_once("  ")?;
    let id = u16::from_str_radix(id.trim(), 16).ok()?;
    let name = name.trim();

    if name.is_empty() { None } else { Some((id, name)) }
}
