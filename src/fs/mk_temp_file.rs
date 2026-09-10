use std::path::Path;

use tempfile::Builder;

use super::{NamedTempFile, Result};

/// Creates a uniquely named temporary file under the system temporary directory.
///
/// The returned [`NamedTempFile`] owns the file: dropping it deletes the file, including on an early `?` or a panic.
/// Call [`NamedTempFile::keep`] to defuse that and keep the file on disk.
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if the file could not be created.
pub fn mk_temp_file(prefix: &str) -> Result<NamedTempFile> {
    Ok(NamedTempFile::new(Builder::new().prefix(prefix).tempfile()?))
}

/// Creates a uniquely named temporary file inside `directory`.
///
/// Behaves exactly like [`mk_temp_file`] but places the file where the caller asks, which is what you want when the
/// file is later renamed into place — a rename across filesystems fails.
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if `directory` does not exist or the file could not be created.
pub fn mk_temp_file_in(directory: impl AsRef<Path>, prefix: &str) -> Result<NamedTempFile> {
    Ok(NamedTempFile::new(
        Builder::new().prefix(prefix).tempfile_in(directory)?,
    ))
}
