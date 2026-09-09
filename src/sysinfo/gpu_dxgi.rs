//! GPU detection on Windows, through DXGI.
//!
//! `IDXGIFactory1` enumerates every adapter the graphics stack knows about, and each one's
//! `DXGI_ADAPTER_DESC1` carries the description, the PCI vendor id and the dedicated video memory as plain struct
//! fields. This is the source the WMI class `Win32_VideoController` is built on, reached without spawning a shell
//! to query it.
//!
//! Only [`gpus`] is Windows-specific. The decoding below is compiled everywhere so it stays testable from any host.

use super::gpu::GpuInfo;
use super::vendor::{vendor_from_description, vendor_name};

/// The `DXGI_ADAPTER_FLAG_SOFTWARE` bit, which marks the WARP software rasteriser rather than real hardware.
const ADAPTER_FLAG_SOFTWARE: u32 = 2;

/// Decodes a `DXGI_ADAPTER_DESC1::Description`.
///
/// The field is a fixed 128-unit UTF-16 buffer holding a NUL-terminated string, so everything from the terminator
/// onward is uninitialised padding and must not be decoded. A buffer with no terminator at all is used whole.
pub(super) fn adapter_name(description: &[u16]) -> String {
    let end = description
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(description.len());

    String::from_utf16_lossy(&description[..end]).trim().to_string()
}

/// Reports whether an adapter's flags mark it the software rasteriser.
pub(super) fn is_software_adapter(flags: u32) -> bool {
    flags & ADAPTER_FLAG_SOFTWARE != 0
}

/// Assembles a [`GpuInfo`] from the fields of a `DXGI_ADAPTER_DESC1`.
///
/// An adapter with no description is dropped. `DedicatedVideoMemory` is zero for integrated graphics, which share
/// system memory rather than owning any, so zero becomes `None`.
pub(super) fn gpu_from_adapter(description: &[u16], vendor_id: u32, dedicated_video_memory: u64) -> Option<GpuInfo> {
    let name = adapter_name(description);
    if name.is_empty() {
        return None;
    }

    // DXGI widens the 16-bit PCI vendor id to 32 bits, and uses values above 0xffff for ACPI ids that no PCI
    // vendor table can resolve.
    let vendor = u16::try_from(vendor_id)
        .ok()
        .and_then(vendor_name)
        .or_else(|| vendor_from_description(&name))
        .map(str::to_string);

    Some(GpuInfo {
        name,
        vendor,
        memory: (dedicated_video_memory > 0).then_some(dedicated_video_memory),
    })
}

#[cfg(target_os = "windows")]
pub(super) use imp::gpus;

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, IDXGIFactory1};

    use super::super::error::{Result, SysinfoError};
    use super::super::gpu::GpuInfo;
    use super::{gpu_from_adapter, is_software_adapter};

    /// Enumerates the machine's graphics adapters.
    ///
    /// # Errors
    ///
    /// Returns [`SysinfoError::GpuApi`] if the DXGI factory cannot be created, or if enumeration fails for any
    /// reason other than running out of adapters.
    pub(in super::super) fn gpus() -> Result<Vec<GpuInfo>> {
        // SAFETY: `CreateDXGIFactory1` takes no arguments beyond the interface id the generic supplies.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|err| SysinfoError::GpuApi {
            call: "CreateDXGIFactory1",
            code: err.code().0,
        })?;

        let mut gpus = Vec::new();
        for index in 0.. {
            // SAFETY: `factory` is a live COM interface.
            let adapter = match unsafe { factory.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                // The documented way to learn the list has ended. Anything else is a real failure, and swallowing
                // it would silently truncate the results.
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => {
                    return Err(SysinfoError::GpuApi {
                        call: "IDXGIFactory1::EnumAdapters1",
                        code: err.code().0,
                    });
                }
            };

            // SAFETY: `adapter` is a live COM interface and `GetDesc1` fills the descriptor it is given.
            let desc = match unsafe { adapter.GetDesc1() } {
                Ok(desc) => desc,
                Err(err) => {
                    return Err(SysinfoError::GpuApi {
                        call: "IDXGIAdapter1::GetDesc1",
                        code: err.code().0,
                    });
                }
            };

            if is_software_adapter(desc.Flags) {
                continue;
            }

            if let Some(gpu) = gpu_from_adapter(&desc.Description, desc.VendorId, desc.DedicatedVideoMemory as u64) {
                gpus.push(gpu);
            }
        }

        Ok(gpus)
    }
}
