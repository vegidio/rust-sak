use std::path::Path;

use tempfile::Builder;

use super::{Result, TempDir};

/// Creates a uniquely named temporary directory under the system temporary directory.
///
/// The returned [`TempDir`] owns the directory: dropping it removes the directory and everything inside, including on
/// an early `?` or a panic. Call [`TempDir::keep`] to defuse that and keep the directory on disk.
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if the directory could not be created.
pub fn mk_temp_dir(prefix: &str) -> Result<TempDir> {
    Ok(TempDir::new(Builder::new().prefix(prefix).tempdir()?))
}

/// Creates a uniquely named temporary directory inside `directory`.
///
/// Behaves exactly like [`mk_temp_dir`] but places the directory where the caller asks, which is what you want when
/// the temporary data must land on the same filesystem as something else — a rename across filesystems fails.
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if `directory` does not exist or the directory could not be created.
pub fn mk_temp_dir_in(directory: impl AsRef<Path>, prefix: &str) -> Result<TempDir> {
    Ok(TempDir::new(Builder::new().prefix(prefix).tempdir_in(directory)?))
}
