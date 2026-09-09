use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use super::mk_user_config_dir::mk_config_dir_at;
use super::user_config_dir::{config_path, config_root};
use super::{FsError, Result};

/// The mode new configuration files are created with: readable by all, writable by the owner.
#[cfg(unix)]
const FILE_MODE: u32 = 0o644;

/// Creates (if needed) and opens a file inside an application's configuration directory, for reading and writing.
///
/// `file_path` is a relative path within the application's directory, so `"config.toml"` and `"themes/dark.toml"`
/// both work — any missing parent directories are created with mode `0o755`. The file itself is created with mode
/// `0o644` on Unix.
///
/// **The file is not truncated.** An existing configuration file is opened with its contents intact and the cursor at
/// the start, so a caller that means to replace it must truncate deliberately. An existing file also keeps its
/// current mode, since the mode applies at creation only.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` or `file_path` is empty, [`FsError::NoConfigDir`] if the platform has no
/// configuration directory, [`FsError::IllegalPath`] if `name` or `file_path` could escape it, and [`FsError::Io`] if
/// the file could not be created or opened.
pub fn mk_user_config_file(name: &str, file_path: impl AsRef<Path>) -> Result<File> {
    mk_config_file_at(&config_root()?, name, file_path.as_ref())
}

/// Creates and opens the configuration file under an explicit root.
///
/// The testable half of [`mk_user_config_file`], so the suite can write files under a temporary root rather than the
/// real one.
///
/// # Errors
///
/// As [`mk_user_config_file`], minus the platform lookup.
pub(super) fn mk_config_file_at(root: &Path, name: &str, file_path: &Path) -> Result<File> {
    // Validate the full path first, so an illegal `file_path` is refused before any directory is created for it.
    let path = config_path(root, name, file_path)?;

    // `config_path` drops `.` and empty components, so a `file_path` naming no file at all lands back on the
    // application directory — which is a directory, not a file the caller can write to.
    let parent = path.parent().ok_or(FsError::EmptyName)?;
    if parent == root {
        return Err(FsError::EmptyName);
    }

    let sub_dir = file_path.parent().unwrap_or_else(|| Path::new(""));
    mk_config_dir_at(root, name, sub_dir)?;

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(FILE_MODE);

    Ok(options.open(&path)?)
}
