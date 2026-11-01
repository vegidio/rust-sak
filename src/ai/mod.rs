//! Installing what ONNX inference needs, the ONNX Runtime and NVIDIA's CUDA, cuDNN and TensorRT, from Hugging Face
//! folders the caller names, loading the runtime, and building sessions on the execution providers the machine offers.
//!
//! Nothing here names a dependency, a version, a folder, a model, a tuning or a place on disk. The caller passes each dependency as the
//! URL of its Hugging Face folder ([`Source`]), and the root folder to install under. Every download is verified
//! against the hash the Hugging Face API reports for it ([`hf`]), so a file re-uploaded to its folder reaches every
//! caller, old builds included.
//!
//! Each dependency installs into `<root>/<name>/<version>/`, the last two segments of its folder
//! ([`dirs::dir_for`]), so different versions live side by side and callers passing the same root share them:
//!
//! ```text
//! <root>/<name>/<version>/                     installed: the archive's entries, at the paths it gives them
//! <root>/<name>/<version>/.installed           the archive's verified hash and size, and every file it produced
//!                                              with its size: written last, deleted first
//! <root>/<name>/<version>/.lock                held by the process installing into the folder
//! <root>/<name>/<version>/.staging/<asset>     the archive (and the download's .part and sidecar while it downloads)
//! <root>/<name>/<version>/.staging/extract/    this attempt's extraction
//! ```
//!
//! The archive is verified and extracted in staging before anything else in the folder is touched. The marker is
//! deleted before the first old entry is removed and written after the last new one is in place, so it exists only
//! over a complete install, however the process ends.
//!
//! A session is asked for with an ordered list of [`ExecutionProvider`]s, which [`ladder`] turns into rungs: each one
//! keeps every accelerator below it attached, and the CPU is always the last. [`session`] builds on the first rung
//! that works, with each provider's baseline, the caller's [`Tuning`] over it, and the caller's cache folder.
//! [`libpath`] makes the NVIDIA libraries findable, and is called first in `main`.

#![doc = include_str!("README.md")]

pub mod dirs;
mod error;
pub mod gpu;
pub mod hf;
mod install;
pub mod libpath;
mod load;
mod progress;
mod providers;
mod session;
mod source;
mod tuning;
mod webgpu;

#[cfg(test)]
mod test_support;

/// The `ort` this module is built on, so a consumer names `Session` and tensors from the one pinned version without
/// depending on it itself.
pub use ort;

pub use error::{Failure, InstallError, LoadError, SessionError};
pub use install::{Dependency, Kind, Plan, Planned, State, install, is_present, plan};
pub use load::{LoadOptions, environment, load, load_with, runtime_library, webgpu_devices};
pub use progress::{Event, Phase};
pub use providers::{AUTO_ORDER, ExecutionProvider, Supported, UnknownProvider, ladder};
pub use session::{Built, SessionRequest, session};
pub use source::{Source, SourceError, asset_for};
pub use tuning::{
    CachePaths, CoreMlComputeUnits, CoreMlSpecialization, ExecutionMode, Tuning, provider_options, trt_workspace_bytes,
};
