use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::gpu_dxgi::{adapter_name, gpu_from_adapter, is_software_adapter};
use super::gpu_ioreg::{data_string, gpu_from_properties, vendor_id_from_bytes, vram_from_megabytes};
use super::gpu_sysfs::{
    gpu_from_card_dir, gpus_from_drm_dir, is_card_name, is_display_class, parse_class, parse_hex_id, parse_vram,
};
use super::pci_ids::{lookup_pci_ids, read_pci_ids_from};
use super::vendor::{gpu_from_driver, vendor_from_description, vendor_name};
use super::*;

// --- cpu_info tests ---

#[test]
fn cpu_info_reports_a_non_empty_name() {
    assert!(!cpu_info().name.is_empty());
}

#[test]
fn cpu_info_reports_at_least_one_logical_core() {
    assert!(cpu_info().cores >= 1);
}

#[test]
fn cpu_info_reports_no_more_physical_cores_than_logical_ones() {
    let info = cpu_info();

    if let Some(physical) = info.physical_cores {
        assert!(physical >= 1);
        assert!(physical <= info.cores);
    }
}

#[test]
fn cpu_info_reports_the_architecture_the_binary_was_built_for() {
    assert_eq!(cpu_info().arch, std::env::consts::ARCH);
}

// --- memory_info tests ---

#[test]
fn memory_info_reports_a_non_zero_total() {
    assert!(memory_info().total > 0);
}

#[test]
fn memory_info_reports_the_same_total_across_calls() {
    // Total physical RAM cannot change while the process runs, unlike `available`, which is not asserted here.
    assert_eq!(memory_info().total, memory_info().total);
}

// --- vendor tests ---

#[test]
fn vendor_name_resolves_the_well_known_graphics_vendors() {
    assert_eq!(vendor_name(0x10de), Some("NVIDIA"));
    assert_eq!(vendor_name(0x1002), Some("AMD"));
    assert_eq!(vendor_name(0x8086), Some("Intel"));
    assert_eq!(vendor_name(0x106b), Some("Apple"));
}

#[test]
fn vendor_name_does_not_resolve_the_amd_cpu_vendor_id() {
    // 0x1022 is AMD's CPU and chipset id; their GPUs are 0x1002. Confusing the two would label an Intel iGPU's
    // neighbouring devices as graphics vendors.
    assert_eq!(vendor_name(0x1022), None);
}

#[test]
fn vendor_name_returns_none_for_an_unknown_id() {
    assert_eq!(vendor_name(0xffff), None);
}

#[test]
fn vendor_from_description_matches_case_insensitively() {
    assert_eq!(vendor_from_description("NVIDIA GeForce RTX 3090"), Some("NVIDIA"));
    assert_eq!(vendor_from_description("Intel(R) Arc(TM) A770 Graphics"), Some("Intel"));
    assert_eq!(vendor_from_description("AMD Radeon Pro 5500M"), Some("AMD"));
    assert_eq!(vendor_from_description("apple m2 max"), Some("Apple"));
}

#[test]
fn vendor_from_description_returns_none_for_an_unrecognisable_name() {
    assert_eq!(vendor_from_description("Generic Display Adapter"), None);
}

#[test]
fn gpu_from_driver_accepts_the_known_soc_drivers() {
    assert_eq!(gpu_from_driver("panfrost"), Some(("Mali GPU", "ARM")));
    assert_eq!(gpu_from_driver("vc4"), Some(("VideoCore GPU", "Broadcom")));
    assert_eq!(gpu_from_driver("asahi"), Some(("Apple GPU", "Apple")));
}

#[test]
fn gpu_from_driver_rejects_framebuffer_stubs_and_virtual_devices() {
    // These bind to a `cardN` entry that carries no PCI identity, so only the allowlist keeps them from being
    // reported as GPUs. `simpledrm` in particular is present on nearly every UEFI Linux boot.
    for driver in ["simpledrm", "vkms", "udl", "evdi", "ofdrm"] {
        assert_eq!(gpu_from_driver(driver), None, "{driver} must not be reported as a GPU");
    }
}

// --- pci.ids fixture ---

/// An excerpt of the real database, keeping the shapes a parser has to survive: comments between entries, a
/// subsystem block, a vendor whose devices are not the ones being looked for, and the class database that follows
/// the vendor list.
const PCI_IDS_SAMPLE: &str = "\
# Comments at the top of the file.

1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX]
\t\t1002 0e3b  Radeon RX 7900 XTX
\t73df  Navi 22 [Radeon RX 6700/6700 XT/6750 XT]
# A comment sitting between two vendors.
10de  NVIDIA Corporation
\t2204  GA102 [GeForce RTX 3090]
\t2684  AD102 [GeForce RTX 4090]
8086  Intel Corporation
\t56a0  DG2 [Arc A770]

# List of known device classes, subclasses and programming interfaces
C 00  Unclassified device
\t00  Non-VGA unclassified device
C 03  Display controller
\t00  VGA compatible controller
";

// --- pci.ids parser tests ---

#[test]
fn lookup_pci_ids_finds_a_vendor_and_device() {
    let names = lookup_pci_ids(PCI_IDS_SAMPLE, 0x10de, 0x2684);

    assert_eq!(names.vendor.as_deref(), Some("NVIDIA Corporation"));
    assert_eq!(names.device.as_deref(), Some("AD102 [GeForce RTX 4090]"));
}

#[test]
fn lookup_pci_ids_returns_the_vendor_when_the_device_is_unknown() {
    let names = lookup_pci_ids(PCI_IDS_SAMPLE, 0x10de, 0xffff);

    assert_eq!(names.vendor.as_deref(), Some("NVIDIA Corporation"));
    assert_eq!(names.device, None);
}

#[test]
fn lookup_pci_ids_returns_nothing_for_an_unknown_vendor() {
    assert_eq!(lookup_pci_ids(PCI_IDS_SAMPLE, 0xdead, 0x2684), Default::default());
}

#[test]
fn lookup_pci_ids_does_not_match_a_device_belonging_to_another_vendor() {
    // 0x2684 is an NVIDIA device id. Asking for it under AMD must not find it.
    let names = lookup_pci_ids(PCI_IDS_SAMPLE, 0x1002, 0x2684);

    assert_eq!(names.vendor.as_deref(), Some("Advanced Micro Devices, Inc. [AMD/ATI]"));
    assert_eq!(names.device, None);
}

#[test]
fn lookup_pci_ids_ignores_subsystem_lines() {
    // `1002 0e3b  Radeon RX 7900 XTX` is indented two tabs and must never be read as a device of the vendor.
    let names = lookup_pci_ids(PCI_IDS_SAMPLE, 0x1002, 0x1002);

    assert_eq!(names.device, None);
}

#[test]
fn lookup_pci_ids_stops_before_the_device_class_database() {
    // `C 03  Display controller` would otherwise parse as vendor 0x0003, and its indented subclasses as devices.
    assert_eq!(lookup_pci_ids(PCI_IDS_SAMPLE, 0x0003, 0x0000), Default::default());
}

#[test]
fn lookup_pci_ids_tolerates_an_empty_database() {
    assert_eq!(lookup_pci_ids("", 0x10de, 0x2684), Default::default());
}

#[test]
fn read_pci_ids_from_returns_the_first_readable_database() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("missing.ids");
    let present = dir.path().join("present.ids");
    fs::write(&present, PCI_IDS_SAMPLE).unwrap();

    let found = read_pci_ids_from(&[missing.as_path(), present.as_path()]);

    assert_eq!(found.as_deref(), Some(PCI_IDS_SAMPLE));
}

#[test]
fn read_pci_ids_from_returns_none_when_no_database_exists() {
    let dir = TempDir::new().unwrap();

    assert_eq!(read_pci_ids_from(&[dir.path().join("missing.ids").as_path()]), None);
}

#[test]
fn read_pci_ids_from_skips_a_directory_in_place_of_the_database() {
    let dir = TempDir::new().unwrap();

    assert_eq!(read_pci_ids_from(&[dir.path()]), None);
}

// --- drm sysfs fixture helpers ---

/// Builds a `/sys/class/drm/<name>` card directory with the given `device/` attributes.
///
/// Attributes are written verbatim, newline included, exactly as the kernel formats them, so the parsers are
/// exercised against real text rather than pre-trimmed values.
fn write_card(drm: &Path, name: &str, attributes: &[(&str, &str)]) -> PathBuf {
    let card = drm.join(name);
    let device = card.join("device");
    fs::create_dir_all(&device).unwrap();

    for (attribute, value) in attributes {
        fs::write(device.join(attribute), value).unwrap();
    }

    card
}

/// Points a card's `device/driver` symlink at a driver of the given name, the way the driver core does on bind.
fn bind_driver(card: &Path, driver: &str) {
    let drivers = card.join("bus/pci/drivers");
    fs::create_dir_all(drivers.join(driver)).unwrap();
    unix_fs::symlink(drivers.join(driver), card.join("device/driver")).unwrap();
}

/// The attributes of a discrete NVIDIA card, which reports no VRAM in sysfs.
const NVIDIA_ATTRIBUTES: &[(&str, &str)] = &[("vendor", "0x10de\n"), ("device", "0x2684\n"), ("class", "0x030000\n")];

// --- drm sysfs parser tests ---

#[test]
fn is_card_name_accepts_a_card_and_rejects_everything_else() {
    assert!(is_card_name("card0"));
    assert!(is_card_name("card1"));
    // Minor numbers are not range-checked: the kernel falls back to a much wider range once the low ones run out.
    assert!(is_card_name("card200"));

    assert!(!is_card_name("card0-DP-1"));
    assert!(!is_card_name("card0-HDMI-A-1"));
    assert!(!is_card_name("renderD128"));
    assert!(!is_card_name("version"));
    assert!(!is_card_name("card"));
}

#[test]
fn parse_hex_id_reads_the_kernels_format() {
    assert_eq!(parse_hex_id("0x10de\n"), Some(0x10de));
    assert_eq!(parse_hex_id("0x1002"), Some(0x1002));
}

#[test]
fn parse_hex_id_rejects_a_value_without_the_prefix() {
    assert_eq!(parse_hex_id("10de\n"), None);
}

#[test]
fn parse_class_reads_the_kernels_format() {
    assert_eq!(parse_class("0x030000\n"), Some(0x030000));
    assert_eq!(parse_class("0x030200\n"), Some(0x030200));
}

#[test]
fn parse_class_rejects_the_unpadded_form_from_uevent() {
    // `uevent` exposes the same number as `PCI_CLASS=30000` — no prefix, and unpadded. Accepting it would read a
    // display controller's class as 0x030000 only by luck, and misread others entirely.
    assert_eq!(parse_class("30000\n"), None);
    assert_eq!(parse_class("0x30000\n"), None);
    assert_eq!(parse_class("0x030000A\n"), None);
}

#[test]
fn is_display_class_accepts_every_display_subclass() {
    assert!(is_display_class(0x030000)); // VGA compatible
    assert!(is_display_class(0x030200)); // 3D controller, how laptop NVIDIA cards present
    assert!(is_display_class(0x038000)); // other display controller

    assert!(!is_display_class(0x010802)); // NVMe controller
    assert!(!is_display_class(0x040300)); // audio device
}

#[test]
fn parse_vram_reads_a_decimal_byte_count() {
    assert_eq!(parse_vram("25753026560\n"), Some(25_753_026_560));
}

#[test]
fn parse_vram_treats_zero_as_unknown() {
    // The attribute only exists at all on amdgpu, and disappears when the VRAM manager is not up. Zero means the
    // figure is unavailable, not that the card has no memory.
    assert_eq!(parse_vram("0\n"), None);
}

#[test]
fn parse_vram_rejects_a_non_numeric_value() {
    assert_eq!(parse_vram("unknown\n"), None);
}

// --- drm sysfs tree tests ---

#[test]
fn gpu_from_card_dir_names_a_card_from_the_pci_ids_database() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", NVIDIA_ATTRIBUTES);

    let gpu = gpu_from_card_dir(&card, Some(PCI_IDS_SAMPLE)).unwrap();

    assert_eq!(gpu.name, "AD102 [GeForce RTX 4090]");
    assert_eq!(gpu.vendor.as_deref(), Some("NVIDIA"));
    // NVIDIA publishes no VRAM figure anywhere in sysfs.
    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_card_dir_falls_back_to_the_device_id_without_a_database() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", NVIDIA_ATTRIBUTES);

    let gpu = gpu_from_card_dir(&card, None).unwrap();

    assert_eq!(gpu.name, "NVIDIA Device 2684");
    assert_eq!(gpu.vendor.as_deref(), Some("NVIDIA"));
}

#[test]
fn gpu_from_card_dir_names_an_unknown_vendor_by_both_ids() {
    let dir = TempDir::new().unwrap();
    let card = write_card(
        dir.path(),
        "card0",
        &[("vendor", "0xdead\n"), ("device", "0xbeef\n"), ("class", "0x030000\n")],
    );

    let gpu = gpu_from_card_dir(&card, None).unwrap();

    assert_eq!(gpu.name, "Device dead:beef");
    assert_eq!(gpu.vendor, None);
}

#[test]
fn gpu_from_card_dir_reads_amdgpu_vram_in_bytes() {
    let dir = TempDir::new().unwrap();
    let card = write_card(
        dir.path(),
        "card0",
        &[
            ("vendor", "0x1002\n"),
            ("device", "0x744c\n"),
            ("class", "0x030000\n"),
            ("mem_info_vram_total", "25753026560\n"),
        ],
    );

    let gpu = gpu_from_card_dir(&card, Some(PCI_IDS_SAMPLE)).unwrap();

    assert_eq!(gpu.name, "Navi 31 [Radeon RX 7900 XT/7900 XTX]");
    assert_eq!(gpu.vendor.as_deref(), Some("AMD"));
    assert_eq!(gpu.memory, Some(25_753_026_560));
}

#[test]
fn gpu_from_card_dir_reports_no_memory_when_the_vram_attribute_is_absent() {
    let dir = TempDir::new().unwrap();
    let card = write_card(
        dir.path(),
        "card0",
        &[("vendor", "0x8086\n"), ("device", "0x56a0\n"), ("class", "0x030000\n")],
    );

    let gpu = gpu_from_card_dir(&card, Some(PCI_IDS_SAMPLE)).unwrap();

    assert_eq!(gpu.vendor.as_deref(), Some("Intel"));
    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_card_dir_skips_a_device_that_is_not_a_display_controller() {
    let dir = TempDir::new().unwrap();
    let card = write_card(
        dir.path(),
        "card0",
        &[("vendor", "0x10de\n"), ("device", "0x2684\n"), ("class", "0x010802\n")],
    );

    assert_eq!(gpu_from_card_dir(&card, None), None);
}

#[test]
fn gpu_from_card_dir_skips_a_pci_device_with_no_readable_class() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", &[("vendor", "0x10de\n"), ("device", "0x2684\n")]);

    assert_eq!(gpu_from_card_dir(&card, None), None);
}

#[test]
fn gpu_from_card_dir_identifies_a_soc_gpu_by_its_driver() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", &[]);
    bind_driver(&card, "panfrost");

    let gpu = gpu_from_card_dir(&card, None).unwrap();

    assert_eq!(gpu.name, "Mali GPU");
    assert_eq!(gpu.vendor.as_deref(), Some("ARM"));
    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_card_dir_skips_a_simpledrm_framebuffer() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", &[]);
    bind_driver(&card, "simpledrm");

    assert_eq!(gpu_from_card_dir(&card, None), None);
}

#[test]
fn gpu_from_card_dir_skips_a_card_with_no_identity_at_all() {
    let dir = TempDir::new().unwrap();
    let card = write_card(dir.path(), "card0", &[]);

    assert_eq!(gpu_from_card_dir(&card, None), None);
}

#[test]
fn gpus_from_drm_dir_ignores_connectors_render_nodes_and_the_version_file() {
    let dir = TempDir::new().unwrap();
    write_card(dir.path(), "card0", NVIDIA_ATTRIBUTES);
    write_card(dir.path(), "card0-DP-1", NVIDIA_ATTRIBUTES);
    write_card(dir.path(), "renderD128", NVIDIA_ATTRIBUTES);
    fs::write(dir.path().join("version"), "drm 1.1.0 20060810\n").unwrap();

    let gpus = gpus_from_drm_dir(dir.path(), Some(PCI_IDS_SAMPLE)).unwrap();

    assert_eq!(gpus.len(), 1);
    assert_eq!(gpus[0].name, "AD102 [GeForce RTX 4090]");
}

#[test]
fn gpus_from_drm_dir_returns_cards_in_minor_order() {
    let dir = TempDir::new().unwrap();
    write_card(
        dir.path(),
        "card1",
        &[("vendor", "0x8086\n"), ("device", "0x56a0\n"), ("class", "0x030000\n")],
    );
    write_card(dir.path(), "card0", NVIDIA_ATTRIBUTES);

    let gpus = gpus_from_drm_dir(dir.path(), Some(PCI_IDS_SAMPLE)).unwrap();

    assert_eq!(gpus.len(), 2);
    assert_eq!(gpus[0].vendor.as_deref(), Some("NVIDIA"));
    assert_eq!(gpus[1].vendor.as_deref(), Some("Intel"));
}

#[test]
fn gpus_from_drm_dir_returns_an_empty_list_for_a_missing_directory() {
    let dir = TempDir::new().unwrap();

    // A machine with no DRM subsystem has no cards; that is an answer, not a failure.
    assert_eq!(gpus_from_drm_dir(&dir.path().join("absent"), None).unwrap(), Vec::new());
}

#[test]
fn gpus_from_drm_dir_reports_io_when_the_path_is_not_a_directory() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("drm");
    fs::write(&file, b"not a directory").unwrap();

    let error = gpus_from_drm_dir(&file, None).unwrap_err();

    assert!(matches!(error, SysinfoError::Io(_)));
}

// --- dxgi parser tests ---

/// Encodes a description the way DXGI reports one: UTF-16, NUL-terminated, padded to 128 units.
fn dxgi_description(name: &str) -> Vec<u16> {
    let mut buffer: Vec<u16> = name.encode_utf16().collect();
    buffer.resize(128, 0);
    buffer
}

#[test]
fn adapter_name_stops_at_the_first_nul() {
    assert_eq!(
        adapter_name(&dxgi_description("NVIDIA GeForce RTX 3090")),
        "NVIDIA GeForce RTX 3090"
    );
}

#[test]
fn adapter_name_ignores_padding_after_the_terminator() {
    let mut description = dxgi_description("Intel(R) UHD Graphics");
    // Uninitialised tail bytes are real: the struct is a fixed buffer and only the prefix is written.
    description[100] = u16::from(b'X');

    assert_eq!(adapter_name(&description), "Intel(R) UHD Graphics");
}

#[test]
fn adapter_name_handles_an_unterminated_description() {
    let description: Vec<u16> = "AMD Radeon".encode_utf16().collect();

    assert_eq!(adapter_name(&description), "AMD Radeon");
}

#[test]
fn adapter_name_is_empty_for_an_empty_buffer() {
    assert_eq!(adapter_name(&[0; 128]), "");
}

#[test]
fn is_software_adapter_detects_the_warp_rasteriser() {
    assert!(is_software_adapter(2));
    assert!(is_software_adapter(3));
    assert!(!is_software_adapter(0));
    assert!(!is_software_adapter(1));
}

#[test]
fn gpu_from_adapter_reads_a_discrete_card() {
    let gpu = gpu_from_adapter(&dxgi_description("NVIDIA GeForce RTX 3090"), 0x10de, 25_757_220_864).unwrap();

    assert_eq!(gpu.name, "NVIDIA GeForce RTX 3090");
    assert_eq!(gpu.vendor.as_deref(), Some("NVIDIA"));
    assert_eq!(gpu.memory, Some(25_757_220_864));
}

#[test]
fn gpu_from_adapter_reports_no_memory_for_integrated_graphics() {
    let gpu = gpu_from_adapter(&dxgi_description("Intel(R) UHD Graphics 770"), 0x8086, 0).unwrap();

    assert_eq!(gpu.vendor.as_deref(), Some("Intel"));
    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_adapter_falls_back_to_the_description_for_an_acpi_vendor_id() {
    // DXGI reports values above 0xffff as ACPI ids, which no PCI vendor table can resolve.
    let gpu = gpu_from_adapter(&dxgi_description("Qualcomm(R) Adreno(TM) GPU"), 0x0001_4d10, 0).unwrap();

    assert_eq!(gpu.vendor.as_deref(), Some("Qualcomm"));
}

#[test]
fn gpu_from_adapter_drops_an_adapter_with_no_description() {
    assert_eq!(gpu_from_adapter(&[0; 128], 0x10de, 0), None);
}

// --- ioreg parser tests ---

#[test]
fn vendor_id_from_bytes_reads_apple_from_little_endian_data() {
    // How an Apple Silicon accelerator records `vendor-id`: 0x106b, little-endian, zero-padded to four bytes.
    assert_eq!(vendor_id_from_bytes(&[0x6b, 0x10, 0x00, 0x00]), Some(0x106b));
}

#[test]
fn vendor_id_from_bytes_reads_a_pci_vendor() {
    assert_eq!(vendor_id_from_bytes(&[0xde, 0x10, 0x00, 0x00]), Some(0x10de));
}

#[test]
fn vendor_id_from_bytes_rejects_a_short_value() {
    assert_eq!(vendor_id_from_bytes(&[0x6b]), None);
    assert_eq!(vendor_id_from_bytes(&[]), None);
}

#[test]
fn data_string_trims_trailing_nul_padding() {
    // How Intel Macs record `model`: a C string in an OSData rather than an OSString.
    assert_eq!(
        data_string(b"AMD Radeon Pro 5500M\0\0\0\0"),
        Some("AMD Radeon Pro 5500M".to_string())
    );
}

#[test]
fn data_string_handles_a_value_with_no_terminator() {
    assert_eq!(data_string(b"Apple M2 Max"), Some("Apple M2 Max".to_string()));
}

#[test]
fn data_string_returns_none_for_an_empty_value() {
    assert_eq!(data_string(b""), None);
    assert_eq!(data_string(b"\0\0\0"), None);
}

#[test]
fn vram_from_megabytes_converts_to_bytes() {
    assert_eq!(vram_from_megabytes(8192), 8_589_934_592);
}

#[test]
fn gpu_from_properties_reads_an_apple_silicon_gpu() {
    let gpu = gpu_from_properties(Some("Apple M2 Max"), Some(0x106b), None).unwrap();

    assert_eq!(gpu.name, "Apple M2 Max");
    assert_eq!(gpu.vendor.as_deref(), Some("Apple"));
    // Unified memory: the GPU owns no VRAM of its own.
    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_properties_reads_an_intel_mac_discrete_gpu() {
    let gpu = gpu_from_properties(Some("AMD Radeon Pro 5500M"), Some(0x1002), Some(8_589_934_592)).unwrap();

    assert_eq!(gpu.vendor.as_deref(), Some("AMD"));
    assert_eq!(gpu.memory, Some(8_589_934_592));
}

#[test]
fn gpu_from_properties_falls_back_to_the_model_when_the_vendor_id_is_missing() {
    let gpu = gpu_from_properties(Some("NVIDIA GeForce GT 750M"), None, None).unwrap();

    assert_eq!(gpu.vendor.as_deref(), Some("NVIDIA"));
}

#[test]
fn gpu_from_properties_treats_zero_vram_as_unknown() {
    let gpu = gpu_from_properties(Some("Apple M2 Max"), Some(0x106b), Some(0)).unwrap();

    assert_eq!(gpu.memory, None);
}

#[test]
fn gpu_from_properties_drops_an_entry_with_no_model() {
    assert_eq!(gpu_from_properties(None, Some(0x106b), None), None);
    assert_eq!(gpu_from_properties(Some("   "), Some(0x106b), None), None);
}

// --- gpu_info tests ---

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn gpu_info_returns_well_formed_entries_on_this_host() {
    // An empty list is a legitimate answer on a headless machine or a container without `/sys`, so the shape of
    // each entry is asserted rather than how many there are.
    for gpu in gpu_info().unwrap() {
        assert!(!gpu.name.is_empty());
        assert_ne!(gpu.memory, Some(0), "unknown VRAM must be None, never Some(0)");
        assert_ne!(gpu.vendor.as_deref(), Some(""));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[test]
fn gpu_info_reports_an_unsupported_platform_elsewhere() {
    assert!(matches!(gpu_info(), Err(SysinfoError::UnsupportedPlatform { .. })));
}

// --- error tests ---

#[test]
fn errors_describe_themselves() {
    let io = SysinfoError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"));
    assert!(io.to_string().contains("denied"));

    let api = SysinfoError::GpuApi {
        call: "CreateDXGIFactory1",
        code: 0x8007_0057_u32 as i32,
    };
    assert_eq!(api.to_string(), "CreateDXGIFactory1 failed with status 0x80070057");

    let unsupported = SysinfoError::UnsupportedPlatform { os: "netbsd" };
    assert_eq!(unsupported.to_string(), "gpu detection is not supported on netbsd");
}

#[test]
fn only_the_io_error_carries_a_source() {
    use std::error::Error;

    let io = SysinfoError::Io(std::io::Error::other("failed"));
    assert!(io.source().is_some());

    assert!(
        SysinfoError::GpuApi {
            call: "IOServiceMatching",
            code: 0
        }
        .source()
        .is_none()
    );
    assert!(SysinfoError::UnsupportedPlatform { os: "netbsd" }.source().is_none());
}
