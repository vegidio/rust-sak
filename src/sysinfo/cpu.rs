/// A description of the machine's processor.
///
/// Returned by [`cpu_info`](super::cpu_info). Everything here is read fresh on each call; nothing is cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuInfo {
    /// The processor's model string, e.g. `"Apple M2 Max"` or `"AMD Ryzen 9 7950X 16-Core Processor"`.
    ///
    /// Falls back to `"Unknown CPU"` when the platform reports nothing — some ARM boards leave `/proc/cpuinfo`'s
    /// `model name` field blank.
    pub name: String,
    /// The number of **logical** cores (hardware threads), as the operating system sees them.
    ///
    /// This is the machine's count, not this process's CPU budget: a container with a CPU quota still reports every
    /// core the kernel exposes. Use [`std::thread::available_parallelism`] when you want the latter.
    pub cores: usize,
    /// The number of **physical** cores, or `None` when the platform does not report it.
    pub physical_cores: Option<usize>,
    /// The architecture this binary was **compiled for**, e.g. `"x86_64"` or `"aarch64"`.
    ///
    /// This is [`std::env::consts::ARCH`], not a runtime probe: an x86-64 build running under Rosetta 2 on Apple
    /// Silicon reports `"x86_64"`, which is the answer a program deciding what it can execute actually wants.
    pub arch: &'static str,
}
