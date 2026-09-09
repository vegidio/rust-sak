use std::path::{Path, PathBuf};

use super::safe_path::validate_entry_name;
use super::{FsError, Result};

/// Computes the path of an application's configuration directory **without creating anything**.
///
/// The result is `<platform config dir>/<name>/<sub_path>` — `~/.config/my-app/themes` on Linux,
/// `~/Library/Application Support/my-app/themes` on macOS, `%APPDATA%\my-app\themes` on Windows. Pass `""` as
/// `sub_path` for the application's directory itself.
///
/// Both `name` and `sub_path` are validated the same way archive entries are, so a `sub_path` of
/// `../../.ssh/authorized_keys` is refused rather than quietly resolved — the trap in composing this path from
/// untrusted input.
///
/// ```
/// use rust_sak::fs::user_config_dir;
///
/// let dir = user_config_dir("my-app", "themes")?;
/// assert!(dir.ends_with("my-app/themes"));
///
/// assert!(user_config_dir("my-app", "../../escape").is_err());
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` is empty, [`FsError::NoConfigDir`] if the platform has no configuration
/// directory, and [`FsError::IllegalPath`] if `name` or `sub_path` could escape it.
pub fn user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    config_path(&config_root()?, name, sub_path.as_ref())
}

/// Returns the platform's configuration directory.
///
/// # Errors
///
/// Returns [`FsError::NoConfigDir`] if the platform has none, or the environment does not say where it is.
pub(super) fn config_root() -> Result<PathBuf> {
    dirs::config_dir().ok_or(FsError::NoConfigDir)
}

/// Joins `root`, `name` and `sub_path` after validating the latter two.
///
/// This is the whole of [`user_config_dir`] except for looking up the platform directory, which is what lets tests
/// exercise the validation against a temporary root instead of the real `$HOME`.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` is empty, or [`FsError::IllegalPath`] if either part is not a contained
/// relative path.
pub(super) fn config_path(root: &Path, name: &str, sub_path: &Path) -> Result<PathBuf> {
    if name.is_empty() {
        return Err(FsError::EmptyName);
    }

    // A name of `.` or `./` passes validation but names no directory at all, which would silently hand back the
    // bare config root — the same mistake an empty name makes, so it earns the same error.
    let name_components = validate_entry_name(name)?;
    if name_components.is_empty() {
        return Err(FsError::EmptyName);
    }

    let mut path = root.to_path_buf();
    for component in name_components {
        path.push(component);
    }

    let sub_path = sub_path.to_str().ok_or_else(|| FsError::IllegalPath {
        path: sub_path.to_string_lossy().into_owned(),
        reason: "is not valid UTF-8".to_string(),
    })?;
    for component in validate_entry_name(sub_path)? {
        path.push(component);
    }

    Ok(path)
}
