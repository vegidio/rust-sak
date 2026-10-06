use std::io;
use std::path::{Path, PathBuf};

use super::trashed::{Locator, Trashed};
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
/// The returned [`Trashed`] is what [`restore_from_trash`](super::restore_from_trash) needs to put the file back; a
/// caller with no use for that can drop it.
///
/// ```no_run
/// use rust_sak::fs::move_to_trash;
///
/// let trashed = move_to_trash("/tmp/unwanted.jpg")?;
/// assert!(trashed.original_path().ends_with("unwanted.jpg"));
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`] with [`io::ErrorKind::NotFound`] if `path` does not exist, and with
/// [`io::ErrorKind::IsADirectory`] if it is a directory; nothing is touched in either case. Returns
/// [`FsError::Trash`] if the platform refused the move, in which case the file is left in place. On macOS, a path
/// that is not valid UTF-8 is refused the same way.
pub fn move_to_trash(path: impl AsRef<Path>) -> Result<Trashed> {
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

    let original = canonical_original(path)?;
    let locator = platform::trash(&original).map_err(|message| FsError::Trash {
        path: path.to_path_buf(),
        message,
    })?;

    Ok(Trashed { original, locator })
}

/// The path with its folder canonicalized and its file name kept, as `trash` does, so a symbolic link stays the link
/// and the path matches what the platform records as the item's original location.
fn canonical_original(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
        .into());
    };

    Ok(parent.canonicalize()?.join(name))
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};

    use objc2::rc::Retained;
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    use super::Locator;

    /// Asks `NSFileManager` directly rather than through `trash`, which drops the item's new location.
    pub(super) fn trash(original: &Path) -> Result<Locator, String> {
        // APFS and HFS+ only store UTF-8 names, so this only refuses paths that could not be on them anyway.
        let Some(utf8) = original.to_str() else {
            return Err("the path is not valid UTF-8".into());
        };

        let url = NSURL::fileURLWithPath(&NSString::from_str(utf8));
        let mut resulting: Option<Retained<NSURL>> = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
            .map_err(|error| error.localizedDescription().to_string())?;

        let in_trash = resulting
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string()));

        Ok(Locator { in_trash })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::Locator;

    /// The time is taken before the move, so the item's own deletion time is never earlier than it, give or take the
    /// rounding `restore_from_trash` allows for.
    pub(super) fn trash(original: &Path) -> Result<Locator, String> {
        let trashed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX));

        trash::delete(original).map_err(|error| error.to_string())?;

        Ok(Locator { trashed_at })
    }
}
