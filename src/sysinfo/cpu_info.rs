use ::sysinfo::{CpuRefreshKind, RefreshKind, System};

use super::CpuInfo;

/// What to call a processor whose model string the platform does not report.
const UNKNOWN_CPU: &str = "Unknown CPU";

/// Reports the machine's processor: its model name, core counts and architecture.
///
/// This is infallible. The underlying platform calls cannot fail — a model string the operating system does not
/// report becomes `"Unknown CPU"` and an unavailable physical-core count becomes `None`, so there is no error to
/// propagate.
///
/// Nothing is cached. The values are stable for the life of a machine, so a caller that reads them often should
/// keep the result rather than call repeatedly:
///
/// ```
/// use std::sync::OnceLock;
/// use rust_sak::sysinfo::{cpu_info, CpuInfo};
///
/// static CPU: OnceLock<CpuInfo> = OnceLock::new();
/// let cpu = CPU.get_or_init(cpu_info);
/// ```
///
/// ```
/// use rust_sak::sysinfo::cpu_info;
///
/// let cpu = cpu_info();
/// assert!(!cpu.name.is_empty());
/// assert!(cpu.cores >= 1);
/// assert_eq!(cpu.arch, std::env::consts::ARCH);
/// ```
pub fn cpu_info() -> CpuInfo {
    // `frequency` is deliberately not requested: on several platforms it costs a sampling interval, and this
    // module does not report clock speed.
    let refresh = RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing());
    let system = System::new_with_specifics(refresh);

    let name = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim())
        .filter(|brand| !brand.is_empty())
        .unwrap_or(UNKNOWN_CPU)
        .to_string();

    CpuInfo {
        name,
        cores: system.cpus().len(),
        physical_cores: System::physical_core_count(),
        arch: std::env::consts::ARCH,
    }
}
