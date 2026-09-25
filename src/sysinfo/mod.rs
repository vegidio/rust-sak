//! Hardware probes: what processor, how much memory, which graphics adapters.
//!
//! Three questions are answered here — [`cpu_info`] for the processor, [`memory_info`] for RAM and swap, and
//! [`gpu_info`] for the graphics adapters. All of it is **synchronous** and none of it spawns a process: each
//! platform is asked through its own interface, which is both faster and more reliable than parsing the output of
//! a command-line tool.
//!
//! [`cpu_info`] and [`memory_info`] do not return a [`Result`], while [`gpu_info`] does. That asymmetry is real
//! rather than an oversight: the processor and memory figures come from platform calls that report "unknown"
//! in-band — an unnamed CPU becomes `"Unknown CPU"`, an unavailable core count becomes `None` — so there is no
//! failure to report. Enumerating GPUs genuinely can fail.
//!
//! Nothing is cached apart from Linux's `pci.ids` database, which [`gpu_info`] reads at most once per process. The
//! CPU and total-memory figures never change while a machine is running, so a caller reading them often should hold
//! onto the result rather than probe repeatedly.
//!
//! What a GPU can report varies by platform and driver, which is why [`GpuInfo::vendor`] and [`GpuInfo::memory`]
//! are optional — see `README.md` in this module for exactly what is available where.
//!
//! ```
//! use rust_sak::sysinfo::{cpu_info, memory_info};
//!
//! let cpu = cpu_info();
//! assert!(cpu.cores >= 1);
//!
//! let memory = memory_info();
//! assert!(memory.total > 0);
//!
//! println!("{} ({} cores), {:.1} GiB RAM", cpu.name, cpu.cores, memory.total as f64 / 1024_f64.powi(3));
//! ```

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod cpu;
mod cpu_info;
mod error;
mod gpu;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod gpu_dxgi;
mod gpu_info;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod gpu_ioreg;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod gpu_nvml;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod gpu_sysfs;
mod memory;
mod memory_info;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod pci_ids;
mod vendor;

pub use cpu::CpuInfo;
pub use cpu_info::cpu_info;
pub use error::{Result, SysinfoError};
pub use gpu::GpuInfo;
pub use gpu_info::gpu_info;
pub use memory::MemoryInfo;
pub use memory_info::memory_info;

#[cfg(test)]
mod tests;
