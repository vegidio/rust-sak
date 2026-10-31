//! Whether this machine's NVIDIA GPU meets minimums the caller gives, probed with `sysinfo`'s `gpu_info` and
//! `cuda_info`.
//!
//! The probe and the comparison are separate: [`meets_with`] is a pure function of what a probe returned, so every
//! branch is tested against constructed values on a machine with no NVIDIA GPU.

use crate::sysinfo::{self, CudaInfo, GpuInfo, SysinfoError};

/// The least an NVIDIA GPU needs, as the caller's GPU libraries require it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuRequirement {
    /// The CUDA version the driver must support, as `(major, minor)`, such as `(13, 0)`.
    pub min_driver: (u32, u32),
    /// The compute capability the CUDA device must have, as `(major, minor)`, such as `(7, 5)`.
    pub min_compute: (u32, u32),
}

/// Whether this machine has an NVIDIA GPU meeting `requirement`. A probe that fails counts as no GPU, with a warning.
/// The CUDA driver is asked only when an NVIDIA adapter is present, since that wakes the GPU.
pub fn meets(requirement: &GpuRequirement) -> bool {
    meets_probed(sysinfo::gpu_info(), sysinfo::cuda_info, requirement)
}

/// [`meets`], over the adapter probe's `outcome`, asking `cuda` only for a machine with an NVIDIA adapter.
fn meets_probed(
    outcome: Result<Vec<GpuInfo>, SysinfoError>,
    cuda: impl FnOnce() -> Option<CudaInfo>,
    requirement: &GpuRequirement,
) -> bool {
    match outcome {
        Ok(gpus) => gpus.iter().any(is_nvidia) && meets_with(&gpus, cuda().as_ref(), requirement),
        Err(error) => {
            tracing::warn!(%error, "the graphics adapters could not be enumerated; continuing as a machine with none");
            false
        }
    }
}

/// Whether `gpus` include an NVIDIA adapter, and `cuda` reports a driver supporting at least
/// `requirement.min_driver` and a first device (the one CUDA opens) of at least `requirement.min_compute`.
pub fn meets_with(gpus: &[GpuInfo], cuda: Option<&CudaInfo>, requirement: &GpuRequirement) -> bool {
    let Some(cuda) = cuda else { return false };
    gpus.iter().any(is_nvidia)
        && cuda.driver_version >= requirement.min_driver
        && cuda
            .devices
            .first()
            .is_some_and(|device| device.compute_capability >= requirement.min_compute)
}

/// Whether `gpu` is NVIDIA's: its vendor or its name says so. Linux takes the vendor from `pci.ids`, which a machine
/// may not have, and the name still says `NVIDIA` there.
fn is_nvidia(gpu: &GpuInfo) -> bool {
    let says = |text: &str| text.to_ascii_lowercase().contains("nvidia");
    gpu.vendor.as_deref().is_some_and(says) || says(&gpu.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysinfo::CudaDevice;

    const REQUIREMENT: GpuRequirement = GpuRequirement {
        min_driver: (13, 0),
        min_compute: (7, 5),
    };

    fn gpu(name: &str, vendor: Option<&str>) -> GpuInfo {
        GpuInfo {
            name: name.into(),
            vendor: vendor.map(Into::into),
            memory: Some(12 << 30),
        }
    }

    fn cuda(driver: (u32, u32), compute: (u32, u32)) -> CudaInfo {
        CudaInfo {
            driver_version: driver,
            devices: vec![CudaDevice {
                name: "NVIDIA GeForce RTX 3080".into(),
                compute_capability: compute,
                total_memory: Some(10 << 30),
            }],
        }
    }

    fn rtx() -> Vec<GpuInfo> {
        vec![gpu("NVIDIA GeForce RTX 3080", Some("NVIDIA"))]
    }

    #[test]
    fn a_capable_machine_meets_it() {
        assert!(meets_with(&rtx(), Some(&cuda((13, 2), (8, 6))), &REQUIREMENT));
        assert!(
            meets_with(&rtx(), Some(&cuda((13, 0), (7, 5))), &REQUIREMENT),
            "the minimums themselves"
        );
        // No vendor from `pci.ids`, but the name says NVIDIA.
        assert!(meets_with(
            &[gpu("NVIDIA RTX A4000", None)],
            Some(&cuda((13, 2), (8, 6))),
            &REQUIREMENT
        ));
    }

    #[test]
    fn an_old_gpu_or_driver_does_not() {
        assert!(
            !meets_with(&rtx(), Some(&cuda((13, 2), (6, 1))), &REQUIREMENT),
            "compute capability 6.1"
        );
        assert!(
            !meets_with(&rtx(), Some(&cuda((12, 4), (8, 6))), &REQUIREMENT),
            "a driver for CUDA 12.4"
        );
    }

    #[test]
    fn no_nvidia_adapter_or_no_cuda_does_not() {
        let apple = [gpu("Apple M3 Max", Some("Apple"))];
        assert!(
            !meets_with(&apple, Some(&cuda((13, 2), (8, 6))), &REQUIREMENT),
            "no NVIDIA adapter"
        );
        assert!(!meets_with(&rtx(), None, &REQUIREMENT), "no CUDA driver answered");
        let no_device = CudaInfo {
            driver_version: (13, 2),
            devices: Vec::new(),
        };
        assert!(!meets_with(&rtx(), Some(&no_device), &REQUIREMENT), "no CUDA device");
    }

    #[test]
    fn a_probe_error_counts_as_no_gpu_and_never_asks_cuda() {
        let error = SysinfoError::UnsupportedPlatform { os: "plan9" };
        assert!(!meets_probed(
            Err(error),
            || panic!("CUDA must not be probed"),
            &REQUIREMENT
        ));
        assert!(!meets_probed(
            Ok(vec![gpu("AMD Radeon", Some("AMD"))]),
            || panic!("CUDA is not probed without an NVIDIA adapter"),
            &REQUIREMENT
        ));
        assert!(meets_probed(Ok(rtx()), || Some(cuda((13, 2), (8, 6))), &REQUIREMENT));
    }

    #[test]
    fn the_same_machine_fails_a_stricter_requirement() {
        let stricter = GpuRequirement {
            min_driver: (14, 0),
            min_compute: (9, 0),
        };
        assert!(meets_with(&rtx(), Some(&cuda((13, 2), (8, 6))), &REQUIREMENT));
        assert!(!meets_with(&rtx(), Some(&cuda((13, 2), (8, 6))), &stricter));
    }
}
