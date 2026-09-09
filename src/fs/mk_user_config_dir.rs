use std::fs::DirBuilder;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::Result;
use super::user_config_dir::{config_path, config_root};

/// The mode new configuration directories are created with: readable and traversable by all, writable by the owner.
#[cfg(unix)]
const DIR_MODE: u32 = 0o755;

/// Creates an application's configuration directory, including any missing parents, and returns its path.
///
/// The path is [`user_config_dir`](super::user_config_dir) computes, so the same validation applies. Directories are
/// created with mode `0o755` on Unix. Succeeds silently if the directory already exists, and leaves an existing
/// directory's mode alone.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`](super::FsError::EmptyName) if `name` is empty,
/// [`FsError::NoConfigDir`](super::FsError::NoConfigDir) if the platform has no configuration directory,
/// [`FsError::IllegalPath`](super::FsError::IllegalPath) if `name` or `sub_path` could escape it, and
/// [`FsError::Io`](super::FsError::Io) if the directory could not be created.
pub fn mk_user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    mk_config_dir_at(&config_root()?, name, sub_path.as_ref())
}

/// Creates the configuration directory under an explicit root.
///
/// The testable half of [`mk_user_config_dir`], so the suite can create directories under a temporary root rather
/// than the real one.
///
/// # Errors
///
/// As [`mk_user_config_dir`], minus the platform lookup.
pub(super) fn mk_config_dir_at(root: &Path, name: &str, sub_path: &Path) -> Result<PathBuf> {
    let path = config_path(root, name, sub_path)?;

    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(DIR_MODE);
    builder.create(&path)?;

    Ok(path)
}
