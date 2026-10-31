//! Installing what ONNX inference needs, the ONNX Runtime and NVIDIA's CUDA, cuDNN and TensorRT, from Hugging Face
//! folders the caller names, and loading the runtime.
//!
//! Nothing here names a dependency, a version, a folder or a place on disk. The caller passes each dependency as the
//! URL of its Hugging Face folder ([`Source`]), and the root folder to install under. Every download is verified
//! against the hash the Hugging Face API reports for it ([`hf`]), so a file re-uploaded to its folder reaches every
//! caller, old builds included.
//!
//! Each dependency installs into `<root>/<name>/<version>/`, the last two segments of its folder
//! ([`dirs::dir_for`]), so different versions live side by side and callers passing the same root share them:
//!
//! ```text
//! <root>/<name>/<version>/                     installed: the archive's entries, at the paths it gives them
//! <root>/<name>/<version>/.installed           the archive's verified hash: written last, deleted first
//! <root>/<name>/<version>/.staging/<asset>     the archive (and the download's .part and sidecar while it downloads)
//! <root>/<name>/<version>/.staging/extract/    this attempt's extraction
//! ```
//!
//! The archive is verified and extracted in staging before anything else in the folder is touched. The marker is
//! deleted before the first old entry is removed and written after the last new one is in place, so it exists only
//! over a complete install, however the process ends.

#![doc = include_str!("README.md")]

pub mod dirs;
mod error;
pub mod gpu;
pub mod hf;
mod install;
mod load;
mod progress;
mod source;

#[cfg(test)]
mod test_support;

pub use error::{Failure, InstallError, LoadError};
pub use install::{Dependency, Kind, install, is_present};
pub use load::{load, runtime_library};
pub use progress::Event;
pub use source::{Source, SourceError, asset_for};
