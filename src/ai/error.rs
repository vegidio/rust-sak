//! Why an install, a load or a session build failed.

use std::io;
use std::path::PathBuf;

use super::ExecutionProvider;
use super::hf::HfError;
use crate::fs::FsError;

/// Why [`install`](super::install) stopped: the dependency it was on, and what went wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("could not install {dependency}: {failure}")]
pub struct InstallError {
    /// The dependency's display name, as the caller gave it.
    pub dependency: &'static str,
    /// What went wrong.
    pub failure: Failure,
}

/// What went wrong installing one dependency.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// The server could not be reached, or the connection broke or timed out, after the retries.
    #[error("the network could not be reached")]
    Network,
    /// The server answered with this HTTP status.
    #[error("the server answered with HTTP {0}")]
    Http(u16),
    /// The disk filled up.
    #[error("the disk is full")]
    DiskFull,
    /// Any other file error, with the system's reason.
    #[error("a file error stopped the install: {0}")]
    Io(String),
    /// The download did not match the hash the Hugging Face API reported. It was deleted.
    #[error("a downloaded file was damaged and has been deleted; run again to download it again")]
    Damaged,
    /// The dependency's folder has no archive for this platform, as spelled by `std::env::consts`.
    #[error("it is not available for {os}/{arch}")]
    UnsupportedPlatform {
        /// As in `std::env::consts::OS`.
        os: &'static str,
        /// As in `std::env::consts::ARCH`.
        arch: &'static str,
    },
    /// The Hugging Face API answered with something that is not what it returns.
    #[error("the server's answer could not be read: {0}")]
    Malformed(String),
}

impl From<HfError> for Failure {
    fn from(error: HfError) -> Self {
        match error {
            HfError::Network => Failure::Network,
            HfError::Http(status) => Failure::Http(status),
            HfError::Malformed(reason) => Failure::Malformed(reason),
        }
    }
}

impl From<io::Error> for Failure {
    fn from(cause: io::Error) -> Self {
        match cause.kind() {
            io::ErrorKind::StorageFull => Failure::DiskFull,
            _ => Failure::Io(cause.to_string()),
        }
    }
}

impl From<FsError> for Failure {
    /// Anything but a file error is an archive the extractor refused, which a verified hash makes unreachable unless
    /// the upload itself is bad.
    fn from(cause: FsError) -> Self {
        match cause {
            FsError::Io(cause) => cause.into(),
            cause => Failure::Io(cause.to_string()),
        }
    }
}

/// Why the runtime could not be loaded. Each names the path it was looking at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// The runtime is not installed there: the folder has no marker, or no runtime library.
    #[error("the ONNX Runtime is not installed at {}", path.display())]
    NotInstalled {
        /// The folder, or the library that was not there.
        path: PathBuf,
    },
    /// The library could not be opened, or it is older than the runtime `ort` was built for.
    #[error("{} could not be opened: {reason}", path.display())]
    Open {
        /// The library.
        path: PathBuf,
        /// The loader's reason.
        reason: String,
    },
    /// The library opened, but refused to start an environment.
    #[error("the ONNX Runtime at {} did not start: {reason}", path.display())]
    Start {
        /// The library.
        path: PathBuf,
        /// The runtime's reason.
        reason: String,
    },
}

/// Why a session could not be built on any rung of its ladder, the CPU included: the model's path, and the CPU's
/// reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {reason}", model.display())]
pub struct SessionError {
    /// The model file.
    pub model: PathBuf,
    /// The runtime's reason on the CPU, the last rung.
    pub reason: String,
}

/// Why a session could not be built on one rung. It displays as the runtime's reason alone, so it can be the source of
/// a caller's own error without repeating what that names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct RungError {
    /// The rung's providers, in attach order. Empty for the CPU.
    pub providers: Vec<ExecutionProvider>,
    /// The runtime's reason.
    pub reason: String,
    /// Whether the device ran out of memory during the build: the runtime logged a failed device allocation on the
    /// building thread, or its reason names one. Never on the CPU. Releasing other sessions' device memory and trying
    /// the rung again may then work.
    pub out_of_memory: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_disk_is_told_apart_from_other_file_errors() {
        assert_eq!(
            Failure::from(io::Error::from(io::ErrorKind::StorageFull)),
            Failure::DiskFull
        );
        assert_eq!(
            Failure::from(io::Error::other("read-only volume")),
            Failure::Io("read-only volume".into())
        );
    }

    #[test]
    fn the_error_names_the_dependency_and_the_reason() {
        let error = InstallError {
            dependency: "NVIDIA cuDNN",
            failure: Failure::Http(404),
        };
        assert_eq!(
            error.to_string(),
            "could not install NVIDIA cuDNN: the server answered with HTTP 404"
        );
        let error = InstallError {
            dependency: "ONNX Runtime",
            failure: Failure::UnsupportedPlatform {
                os: "macos",
                arch: "x86_64",
            },
        };
        assert_eq!(
            error.to_string(),
            "could not install ONNX Runtime: it is not available for macos/x86_64"
        );
    }
}
