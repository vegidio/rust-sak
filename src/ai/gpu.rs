//! Whether this machine's NVIDIA GPU meets minimums the caller gives, probed with `sysinfo`'s `gpu_info` and
//! `cuda_info`.
//!
//! The probe and the comparison are separate: [`check_with`] is a pure function of what a probe returned, so every
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

/// Why a machine does not meet a [`GpuRequirement`]. Each names the versions involved, and displays as a sentence
/// for a log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GpuRefusal {
    /// The machine has no NVIDIA graphics adapter, so the CUDA driver was not asked.
    #[error("there is no NVIDIA graphics adapter")]
    NoAdapter,
    /// No CUDA driver answered: none is installed, or it is too old to report a version.
    #[error("no CUDA driver answered")]
    NoDriver,
    /// The driver supports an older CUDA than required.
    #[error("the driver supports CUDA {}, and {} is required", version(*driver), version(*required))]
    DriverTooOld {
        /// The CUDA version the driver supports.
        driver: (u32, u32),
        /// [`GpuRequirement::min_driver`].
        required: (u32, u32),
    },
    /// The driver answered, but reports no CUDA device.
    #[error("the CUDA {} driver reports no device", version(*driver))]
    NoDevice {
        /// The CUDA version the driver supports.
        driver: (u32, u32),
    },
    /// The first CUDA device, the one CUDA opens, has a lower compute capability than required.
    #[error("{name} has compute capability {}, and {} is required", version(*compute), version(*required))]
    DeviceTooOld {
        /// The device's name, as the driver reports it.
        name: String,
        /// Its compute capability.
        compute: (u32, u32),
        /// [`GpuRequirement::min_compute`].
        required: (u32, u32),
    },
    /// The graphics adapters could not be enumerated.
    #[error("the graphics adapters could not be enumerated: {0}")]
    ProbeFailed(String),
}

fn version((major, minor): (u32, u32)) -> String {
    format!("{major}.{minor}")
}

/// Whether this machine has an NVIDIA GPU meeting `requirement`. A probe that fails counts as no GPU, with a warning.
/// The CUDA driver is asked only when an NVIDIA adapter is present, since that wakes the GPU.
pub fn meets(requirement: &GpuRequirement) -> bool {
    check(requirement).is_ok()
}

/// Whether this machine has an NVIDIA GPU meeting `requirement`, and if not, why. A probe that fails is
/// [`GpuRefusal::ProbeFailed`], with a warning. The CUDA driver is asked only when an NVIDIA adapter is present, since
/// that wakes the GPU.
///
/// # Errors
///
/// The [`GpuRefusal`] saying why the machine does not meet `requirement`.
pub fn check(requirement: &GpuRequirement) -> Result<(), GpuRefusal> {
    check_probed(sysinfo::gpu_info(), sysinfo::cuda_info, requirement)
}

/// [`check`], over the adapter probe's `outcome`, asking `cuda` only for a machine with an NVIDIA adapter.
fn check_probed(
    outcome: Result<Vec<GpuInfo>, SysinfoError>,
    cuda: impl FnOnce() -> Option<CudaInfo>,
    requirement: &GpuRequirement,
) -> Result<(), GpuRefusal> {
    match outcome {
        Ok(gpus) if gpus.iter().any(is_nvidia) => check_with(&gpus, cuda().as_ref(), requirement),
        Ok(_) => Err(GpuRefusal::NoAdapter),
        Err(error) => {
            tracing::warn!(%error, "the graphics adapters could not be enumerated; continuing as a machine with none");
            Err(GpuRefusal::ProbeFailed(error.to_string()))
        }
    }
}

/// Whether `gpus` include an NVIDIA adapter, and `cuda` reports a driver supporting at least
/// `requirement.min_driver` and a first device (the one CUDA opens) of at least `requirement.min_compute`.
pub fn meets_with(gpus: &[GpuInfo], cuda: Option<&CudaInfo>, requirement: &GpuRequirement) -> bool {
    check_with(gpus, cuda, requirement).is_ok()
}

/// [`meets_with`], saying why not. Pure: every refusal is reached from constructed probe results.
///
/// # Errors
///
/// The first [`GpuRefusal`] in the order the adapter, the driver, its version and the device are checked.
pub fn check_with(gpus: &[GpuInfo], cuda: Option<&CudaInfo>, requirement: &GpuRequirement) -> Result<(), GpuRefusal> {
    if !gpus.iter().any(is_nvidia) {
        return Err(GpuRefusal::NoAdapter);
    }
    let cuda = cuda.ok_or(GpuRefusal::NoDriver)?;
    let driver = cuda.driver_version;
    if driver < requirement.min_driver {
        return Err(GpuRefusal::DriverTooOld {
            driver,
            required: requirement.min_driver,
        });
    }
    let device = cuda.devices.first().ok_or(GpuRefusal::NoDevice { driver })?;
    if device.compute_capability < requirement.min_compute {
        return Err(GpuRefusal::DeviceTooOld {
            name: device.name.clone(),
            compute: device.compute_capability,
            required: requirement.min_compute,
        });
    }
    Ok(())
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
        let reason = error.to_string();
        assert_eq!(
            check_probed(Err(error), || panic!("CUDA must not be probed"), &REQUIREMENT),
            Err(GpuRefusal::ProbeFailed(reason))
        );
        assert_eq!(
            check_probed(
                Ok(vec![gpu("AMD Radeon", Some("AMD"))]),
                || panic!("CUDA is not probed without an NVIDIA adapter"),
                &REQUIREMENT
            ),
            Err(GpuRefusal::NoAdapter)
        );
        assert_eq!(
            check_probed(Ok(rtx()), || Some(cuda((13, 2), (8, 6))), &REQUIREMENT),
            Ok(())
        );
    }

    #[test]
    fn each_refusal_names_the_versions_involved() {
        assert_eq!(check_with(&rtx(), None, &REQUIREMENT), Err(GpuRefusal::NoDriver));
        assert_eq!(
            check_with(&rtx(), Some(&cuda((12, 4), (8, 6))), &REQUIREMENT),
            Err(GpuRefusal::DriverTooOld {
                driver: (12, 4),
                required: (13, 0)
            })
        );
        let no_device = CudaInfo {
            driver_version: (13, 2),
            devices: Vec::new(),
        };
        assert_eq!(
            check_with(&rtx(), Some(&no_device), &REQUIREMENT),
            Err(GpuRefusal::NoDevice { driver: (13, 2) })
        );
        let old = GpuRefusal::DeviceTooOld {
            name: "NVIDIA GeForce RTX 3080".into(),
            compute: (6, 1),
            required: (7, 5),
        };
        assert_eq!(
            check_with(&rtx(), Some(&cuda((13, 2), (6, 1))), &REQUIREMENT),
            Err(old.clone())
        );
        assert_eq!(
            check_with(&[gpu("Apple M3 Max", Some("Apple"))], None, &REQUIREMENT),
            Err(GpuRefusal::NoAdapter)
        );

        assert_eq!(
            GpuRefusal::DriverTooOld {
                driver: (12, 4),
                required: (13, 0)
            }
            .to_string(),
            "the driver supports CUDA 12.4, and 13.0 is required"
        );
        assert_eq!(
            GpuRefusal::NoDevice { driver: (13, 2) }.to_string(),
            "the CUDA 13.2 driver reports no device"
        );
        assert_eq!(
            old.to_string(),
            "NVIDIA GeForce RTX 3080 has compute capability 6.1, and 7.5 is required"
        );
    }

    #[test]
    fn the_driver_is_judged_before_the_device() {
        assert_eq!(
            check_with(&rtx(), Some(&cuda((12, 4), (6, 1))), &REQUIREMENT),
            Err(GpuRefusal::DriverTooOld {
                driver: (12, 4),
                required: (13, 0)
            })
        );
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
