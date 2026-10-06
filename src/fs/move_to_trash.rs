use std::io;
use std::path::Path;

use super::{FsError, Result};

/// Moves the file at `path` to the platform's Trash: the macOS Trash, the Windows Recycle Bin, or the
/// freedesktop.org trash on Linux and the BSDs, including the per-volume `.Trash-$uid` directories it defines.
///
/// The file is never deleted outright. A volume with no usable Trash — a network share, some removable drives — is
/// an error, and the file stays where it is.
///
/// **Only files are accepted, never directories.** A symbolic link counts as what it points at for that check, and
/// it is the link itself that goes to the Trash, not its target.
///
/// ```no_run
/// use rust_sak::fs::move_to_trash;
///
/// move_to_trash("/tmp/unwanted.jpg")?;
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`] with [`io::ErrorKind::NotFound`] if `path` does not exist, and with
/// [`io::ErrorKind::IsADirectory`] if it is a directory; nothing is touched in either case. Returns
/// [`FsError::Trash`] if the platform refused the move, in which case the file is left in place.
pub fn move_to_trash(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();

    // Checked here rather than left to the platform, so "no longer exists" reads the same on every OS.
    let metadata = std::fs::metadata(path)?;
    if metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            format!("{} is a directory", path.display()),
        )
        .into());
    }

    trash::delete(path).map_err(|error| FsError::Trash {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}
