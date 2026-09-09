use ::sysinfo::{MemoryRefreshKind, RefreshKind, System};

use super::MemoryInfo;

/// Reports the machine's memory, in bytes.
///
/// This is infallible, for the same reason [`cpu_info`](super::cpu_info) is: the underlying platform calls report
/// zero rather than failing. A machine with no swap configured genuinely has `swap_total: 0`.
///
/// Unlike [`cpu_info`](super::cpu_info), only [`total`](MemoryInfo::total) is stable — [`available`] moves
/// constantly, so this is worth calling fresh whenever the current figure matters.
///
/// [`available`]: MemoryInfo::available
///
/// ```
/// use rust_sak::sysinfo::memory_info;
///
/// let memory = memory_info();
/// assert!(memory.total > 0);
/// println!("{:.1} GiB of RAM", memory.total as f64 / (1024.0 * 1024.0 * 1024.0));
/// ```
pub fn memory_info() -> MemoryInfo {
    let refresh = RefreshKind::nothing().with_memory(MemoryRefreshKind::everything());
    let system = System::new_with_specifics(refresh);

    MemoryInfo {
        total: system.total_memory(),
        available: system.available_memory(),
        swap_total: system.total_swap(),
    }
}
