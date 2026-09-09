/// The machine's memory, in **bytes**.
///
/// Returned by [`memory_info`](super::memory_info).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    /// Total physical RAM.
    pub total: u64,
    /// RAM available for reuse — not the same as free RAM, which excludes reclaimable page cache.
    ///
    /// Windows and the BSDs do not report an "available" figure, so this equals free memory there.
    pub available: u64,
    /// Total swap space. `0` on a machine with no swap configured, which is common in containers.
    pub swap_total: u64,
}
