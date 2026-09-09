# `sysinfo` module

Hardware probes: **what processor**, **how much memory**, **which graphics adapters**. Everything here is **synchronous** and **nothing spawns a process** — each platform is asked through its own interface.

## Enabling

The module is gated behind the `sysinfo` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "1", features = ["sysinfo"] }
```

> This feature needs no toolchain beyond a Rust compiler — no C compiler, no downloads. It requires Rust **1.95** or newer, which is the minimum supported version of the [`sysinfo`](https://crates.io/crates/sysinfo) crate it builds on.

## Public API

| Function      | Signature                              | What it does                                    |
|---------------|----------------------------------------|-------------------------------------------------|
| `cpu_info`    | `fn cpu_info() -> CpuInfo`             | Processor model, core counts, architecture.     |
| `memory_info` | `fn memory_info() -> MemoryInfo`       | Total and available RAM, total swap, in bytes.  |
| `gpu_info`    | `fn gpu_info() -> Result<Vec<GpuInfo>>` | Every graphics adapter the OS reports.         |

### Why only one of them returns a `Result`

`cpu_info` and `memory_info` are **infallible**. The platform calls behind them report "unknown" in-band rather than failing — an unnamed processor becomes `"Unknown CPU"`, an unavailable physical-core count becomes `None` — so there is no error to propagate and no `?` to write. Enumerating GPUs genuinely can fail, so `gpu_info` returns a `Result`.

### Nothing is cached

Every call probes afresh. The CPU figures and total RAM cannot change while a machine is running, so hold onto them rather than probing in a loop:

```rust
use std::sync::OnceLock;
use rust_sak::sysinfo::{cpu_info, CpuInfo};

static CPU: OnceLock<CpuInfo> = OnceLock::new();
let cpu = CPU.get_or_init(cpu_info);
```

On Linux `gpu_info` reads the ~1.5 MB `pci.ids` database once per call, so it is the one worth calling sparingly.

## Types

### `CpuInfo`

| Field            | Type                 | Notes                                                                    |
|------------------|----------------------|--------------------------------------------------------------------------|
| `name`           | `String`             | `"Apple M2 Max"`. Falls back to `"Unknown CPU"`.                          |
| `cores`          | `usize`              | **Logical** cores — the machine's count, not this process's CPU budget.  |
| `physical_cores` | `Option<usize>`      | `None` where the platform does not report it.                            |
| `arch`           | `&'static str`       | What the binary was **compiled for**, not a runtime probe.               |

`cores` counts every core the kernel exposes, so a container with a CPU quota still sees the whole machine. Use `std::thread::available_parallelism` when you want the process's actual budget. Likewise `arch` is `std::env::consts::ARCH`, so an x86-64 build under Rosetta 2 reports `"x86_64"` — the answer a program deciding what it can execute wants.

### `MemoryInfo`

All figures are **bytes**. `available` is not the same as free memory: it counts reclaimable page cache, which free memory excludes. Windows and the BSDs report no "available" figure, so it equals free memory there. `swap_total` is `0` on a machine with no swap, which is normal in containers.

### `GpuInfo`

| Field    | Type              | Notes                                                    |
|----------|-------------------|----------------------------------------------------------|
| `name`   | `String`          | `"Apple M2 Max"`, `"NVIDIA GeForce RTX 4090"`.           |
| `vendor` | `Option<String>`  | `"NVIDIA"`, `"AMD"`, `"Intel"`, `"Apple"`, …             |
| `memory` | `Option<u64>`     | Dedicated VRAM in bytes. **Never `Some(0)`.**            |

Both optional fields distinguish *"this does not apply"* from a zero. An Apple Silicon GPU has no VRAM of its own because memory is unified, which is not the same as having none.

## How each platform is asked

| Platform | Mechanism | Source of GPU data |
|----------|-----------|--------------------|
| macOS    | **IOKit** | `IOAccelerator` entries in the IORegistry — the same data `system_profiler SPDisplaysDataType` prints. |
| Windows  | **DXGI**  | `IDXGIFactory1::EnumAdapters1` → `DXGI_ADAPTER_DESC1`, the source behind WMI's `Win32_VideoController`. |
| Linux    | **sysfs** | `/sys/class/drm/card*/device/`, plus `pci.ids` for model names. |

Linux is the odd one out because it has no GPU API at all: the kernel publishes device data as a virtual filesystem, which is what `lspci` itself reads.

On any other operating system `gpu_info` returns `SysinfoError::UnsupportedPlatform` rather than an empty list, so a caller can tell "not implemented here" from "no adapters present".

## Limitations worth knowing

**`memory` is `None` more often than you might expect.** It is reported only where the platform actually publishes it:

| Platform / driver | VRAM reported? |
|-------------------|----------------|
| Windows, any GPU  | ✅ yes         |
| macOS, Intel Mac with a discrete GPU | ✅ yes |
| macOS, Apple Silicon | ❌ no — unified memory, no separate VRAM exists |
| Linux, `amdgpu`   | ✅ yes         |
| Linux, Intel (`i915` / `xe`), `nouveau`, proprietary NVIDIA | ❌ no — nothing is published in sysfs |

The Linux gaps are kernel limitations, not shortcuts: Intel's sysfs VRAM proposal was never merged, and the proprietary NVIDIA driver exposes no framebuffer size anywhere under `/proc` or `/sys`. Reading those needs a DRM `ioctl` or NVML, neither of which this module does. A zero is never invented to paper over the gap.

Also:

- **An empty list is a normal answer**, not a failure. A headless server, a VM with no display adapter, or a container without `/sys` mounted all legitimately report no GPU.
- **Linux GPU names degrade to vendor-only** when no `pci.ids` database is installed, e.g. `"NVIDIA Device 2684"`.
- **Non-PCI SoC GPUs** (Mali, Adreno, VideoCore, Apple via Asahi) are recognised by their DRM driver. This is an allowlist, which is what keeps framebuffer stubs like `simpledrm` — present on nearly every UEFI Linux boot — from being reported as graphics hardware.
- **32-bit Windows truncates** `DedicatedVideoMemory`, which DXGI types as `SIZE_T`. Documented Microsoft behaviour, not worked around here.

## Errors

`SysinfoError` has three variants, all reporting a probe that *could not run* — never one that ran and found nothing:

| Variant                | When                                                                 |
|------------------------|----------------------------------------------------------------------|
| `Io`                   | Linux only: `/sys/class/drm` exists but cannot be read. A *missing* directory is not an error. |
| `GpuApi { call, code }` | An OS graphics call failed. `code` is a `kern_return_t` on macOS, an `HRESULT` on Windows. |
| `UnsupportedPlatform`  | GPU detection has no implementation for this operating system.       |

## Usage

```rust
use rust_sak::sysinfo::{cpu_info, gpu_info, memory_info};

let cpu = cpu_info();
let memory = memory_info();

println!("{} — {} cores ({})", cpu.name, cpu.cores, cpu.arch);
println!("{:.1} GiB RAM", memory.total as f64 / 1024_f64.powi(3));

for gpu in gpu_info()? {
    match gpu.memory {
        Some(bytes) => println!("{} — {} MiB VRAM", gpu.name, bytes / (1024 * 1024)),
        None => println!("{} — shared or unreported memory", gpu.name),
    }
}
# Ok::<(), rust_sak::sysinfo::SysinfoError>(())
```

On an Apple Silicon Mac that prints something like:

```
Apple M2 Max — 12 cores (aarch64)
64.0 GiB RAM
Apple M2 Max — shared or unreported memory
```
