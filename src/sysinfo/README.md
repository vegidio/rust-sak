# `sysinfo` module

Hardware probes: **what processor**, **how much memory**, **which graphics adapters**, **whether WebGPU can run**. Everything here is **synchronous** and **nothing spawns a process** — each platform is asked through its own interface.

## Enabling

The module is gated behind the `sysinfo` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "2", features = ["sysinfo"] }
```

> This feature needs no toolchain beyond a Rust compiler — no C compiler, no downloads. It requires Rust **1.95** or newer, which is the minimum supported version of the [`sysinfo`](https://crates.io/crates/sysinfo) crate it builds on.

## Public API

| Function              | Signature                               | What it does                                   |
|-----------------------|-----------------------------------------|------------------------------------------------|
| `cpu_info`            | `fn cpu_info() -> CpuInfo`              | Processor model, core counts, architecture.    |
| `memory_info`         | `fn memory_info() -> MemoryInfo`        | Total and available RAM, total swap, in bytes. |
| `gpu_info`            | `fn gpu_info() -> Result<Vec<GpuInfo>>` | Every graphics adapter the OS reports.         |
| `is_webgpu_supported` | `fn is_webgpu_supported() -> bool`      | Whether a WebGPU implementation can run here.  |

### Why only one of them returns a `Result`

`cpu_info` and `memory_info` are **infallible**. The platform calls behind them report "unknown" in-band rather than failing — an unnamed processor becomes `"Unknown CPU"`, an unavailable physical-core count becomes `None` — so there is no error to propagate and no `?` to write. Enumerating GPUs genuinely can fail, so `gpu_info` returns a `Result`.

### Results are not cached

Every call probes the hardware afresh. The CPU figures and total RAM cannot change while a machine is running, so hold onto them rather than probing in a loop:

```rust
use std::sync::OnceLock;
use rust_sak::sysinfo::{cpu_info, CpuInfo};

static CPU: OnceLock<CpuInfo> = OnceLock::new();
let cpu = CPU.get_or_init(cpu_info);
```

The one exception is the ~1.5 MB `pci.ids` database `gpu_info` consults on Linux for model names: it cannot meaningfully change while the process runs, so it is read at most once per process rather than once per call. The GPU probe itself still runs every time.

## Types

### `CpuInfo`

| Field            | Type            | Notes                                                                   |
|------------------|-----------------|-------------------------------------------------------------------------|
| `name`           | `String`        | `"Apple M2 Max"`. Falls back to `"Unknown CPU"`.                        |
| `cores`          | `usize`         | **Logical** cores — the machine's count, not this process's CPU budget. |
| `physical_cores` | `Option<usize>` | `None` where the platform does not report it.                           |
| `arch`           | `&'static str`  | What the binary was **compiled for**, not a runtime probe.              |

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

| Platform | Mechanism | Source of GPU data                                                                                      |
|----------|-----------|---------------------------------------------------------------------------------------------------------|
| macOS    | **IOKit** | `IOAccelerator` entries in the IORegistry — the same data `system_profiler SPDisplaysDataType` prints.  |
| Windows  | **DXGI**  | `IDXGIFactory1::EnumAdapters1` → `DXGI_ADAPTER_DESC1`, the source behind WMI's `Win32_VideoController`. |
| Linux    | **sysfs** | `/sys/class/drm/card*/device/`, plus `pci.ids` for model names.                                         |
| Linux    | **NVML**  | `libnvidia-ml.so.1`, loaded at runtime — only when sysfs shows no NVIDIA card.                          |

Linux is the odd one out because it has no GPU API at all: the kernel publishes device data as a virtual filesystem, which is what `lspci` itself reads.

That filesystem only lists a card some DRM driver has claimed, and an NVIDIA GPU is often claimed by none: under **WSL2** the GPU is paravirtualised through `/dev/dxg` and no PCI display device exists, and the proprietary driver running **without `nvidia-drm`** — the usual setup on headless machines and in containers — never registers the card with DRM. Both still ship NVML with the driver (WSL in `/usr/lib/wsl/lib`), so when sysfs finds no NVIDIA card, `gpu_info` asks NVML and adds what it reports. A machine without the NVIDIA driver has no NVML, and simply gets the sysfs answer.

On any other operating system `gpu_info` returns `SysinfoError::UnsupportedPlatform` rather than an empty list, so a caller can tell "not implemented here" from "no adapters present".

## WebGPU

`is_webgpu_supported` answers whether the platform's native graphics API that WebGPU implementations (Dawn, wgpu) run on is usable:

| Platform | Backend        | Answer                                                                       |
|----------|----------------|------------------------------------------------------------------------------|
| macOS    | Metal          | Always `true`.                                                               |
| Windows  | Direct3D 12    | Always `true`.                                                               |
| Linux    | Vulkan         | `true` only if Vulkan lists a GPU supporting Vulkan 1.1 or newer.            |
| Other    | —              | Always `false`.                                                              |

Linux ships no graphics API, so Vulkan needs both the loader (`libvulkan.so.1`: `libvulkan1` on Debian and Ubuntu, `vulkan-loader` on Fedora, `vulkan-icd-loader` on Arch) and a driver for the GPU — `mesa-vulkan-drivers` for AMD, Intel and NVK, or the one the proprietary NVIDIA driver installs itself. Package names are never inspected: the loader is opened at runtime, an instance is created and its physical devices are listed, which is what a WebGPU implementation does first.

A **CPU device does not count**. `mesa-vulkan-drivers` also installs `lavapipe`, a software rasteriser that reports itself as a Vulkan device, so a machine with no GPU driver still lists one — and WebGPU on it is slower than using the CPU directly. Vulkan **1.0-only** GPUs do not count either, because Dawn requires 1.1.

A `true` is what the machine offers, not a guarantee: an adapter can still lack a feature or limit a particular WebGPU implementation requires. Each call on Linux creates a Vulkan instance, which loads every installed driver, so hold onto the answer rather than asking repeatedly.

## Limitations worth knowing

**`memory` is `None` more often than you might expect.** It is reported only where the platform actually publishes it:

| Platform / driver                                           | VRAM reported?                                  |
|-------------------------------------------------------------|-------------------------------------------------|
| Windows, any GPU                                            | ✅ yes                                          |
| macOS, Intel Mac with a discrete GPU                        | ✅ yes                                          |
| macOS, Apple Silicon                                        | ❌ no — unified memory, no separate VRAM exists |
| Linux, `amdgpu`                                             | ✅ yes                                          |
| Linux, NVIDIA found through NVML (WSL2, no `nvidia-drm`)    | ✅ yes                                          |
| Linux, Intel (`i915` / `xe`), `nouveau`, proprietary NVIDIA | ❌ no — nothing is published in sysfs           |

The Linux gaps are kernel limitations, not shortcuts: Intel's sysfs VRAM proposal was never merged, and the proprietary NVIDIA driver exposes no framebuffer size anywhere under `/proc` or `/sys`. Reading those needs a DRM `ioctl` or NVML, and NVML is only asked when sysfs found no NVIDIA card at all — so an NVIDIA card sysfs *does* list keeps `None`. A zero is never invented to paper over the gap.

Also:

- **An empty list is a normal answer**, not a failure. A headless server, a VM with no display adapter, or a container without `/sys` mounted all legitimately report no GPU.
- **Linux GPU names degrade to vendor-only** when no `pci.ids` database is installed, e.g. `"NVIDIA Device 2684"`.
- **Non-PCI SoC GPUs** (Mali, Adreno, VideoCore, Apple via Asahi) are recognised by their DRM driver. This is an allowlist, which is what keeps framebuffer stubs like `simpledrm` — present on nearly every UEFI Linux boot — from being reported as graphics hardware.
- **32-bit Windows truncates** `DedicatedVideoMemory`, which DXGI types as `SIZE_T`. Documented Microsoft behaviour, not worked around here.

## Errors

`SysinfoError` has three variants, all reporting a probe that *could not run* — never one that ran and found nothing:

| Variant                 | When                                                                                           |
|-------------------------|------------------------------------------------------------------------------------------------|
| `Io`                    | Linux only: `/sys/class/drm` exists but cannot be read. A *missing* directory is not an error. |
| `GpuApi { call, code }` | An OS graphics call failed. `code` is a `kern_return_t` on macOS, an `HRESULT` on Windows.     |
| `UnsupportedPlatform`   | GPU detection has no implementation for this operating system.                                 |

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

```text
Apple M2 Max — 12 cores (aarch64)
64.0 GiB RAM
Apple M2 Max — shared or unreported memory
```
