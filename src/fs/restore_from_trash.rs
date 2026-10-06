use std::io;
use std::path::{Path, PathBuf};

use super::trashed::Trashed;
use super::{FsError, Result};

/// Puts a file moved by [`move_to_trash`](super::move_to_trash) back at its original path, and returns that path.
///
/// **It never overwrites anything.** If a file, folder or link has taken the original path since, nothing moves.
/// A missing original folder is recreated.
///
/// On macOS the file is found by the path the Trash gave it. On Windows and Linux it is looked up by its original
/// path among the items deleted at or after the moment it was moved, and the newest one is taken. So if another
/// program trashed a file at that same path afterwards, that newer copy is the one restored.
///
/// ```no_run
/// use rust_sak::fs::{move_to_trash, restore_from_trash};
///
/// let trashed = move_to_trash("/tmp/unwanted.jpg")?;
/// let back = restore_from_trash(&trashed)?;
/// assert_eq!(back, trashed.original_path());
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`] with [`io::ErrorKind::AlreadyExists`] if the original path is taken, and with
/// [`io::ErrorKind::NotFound`] if the file is no longer in the Trash (it was emptied, or the file restored by other
/// means); nothing is touched in either case. Returns [`FsError::Restore`] if the platform refused the move, in which
/// case the file stays in the Trash.
pub fn restore_from_trash(trashed: &Trashed) -> Result<PathBuf> {
    let original = trashed.original_path();

    // Checked before the platform is asked so the answer reads the same everywhere; the platform calls below refuse
    // to overwrite as well, so a file appearing in between is not overwritten either.
    if std::fs::symlink_metadata(original).is_ok() {
        return Err(already_exists(original));
    }

    platform::restore(trashed)?;

    Ok(original.to_path_buf())
}

fn already_exists(path: &Path) -> FsError {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists", path.display()),
    )
    .into()
}

fn not_in_trash(path: &Path) -> FsError {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} is no longer in the Trash", path.display()),
    )
    .into()
}

fn recreate_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_foundation::{
        NSFileManager, NSFileNoSuchFileError, NSFileReadNoSuchFileError, NSFileWriteFileExistsError, NSString, NSURL,
    };

    use super::{FsError, Result, Trashed, already_exists, not_in_trash, recreate_parent};

    pub(super) fn restore(trashed: &Trashed) -> Result<()> {
        let original = trashed.original_path();
        let Some(in_trash) = trashed
            .locator
            .in_trash
            .as_deref()
            .filter(|path| std::fs::symlink_metadata(path).is_ok())
        else {
            return Err(not_in_trash(original));
        };

        recreate_parent(original)?;

        // Both were UTF-8 when the file was moved: the original was checked, and the Trash path came from an NSURL.
        let (Some(from), Some(to)) = (in_trash.to_str(), original.to_str()) else {
            return Err(not_in_trash(original));
        };
        let from = NSURL::fileURLWithPath(&NSString::from_str(from));
        let to = NSURL::fileURLWithPath(&NSString::from_str(to));

        // `moveItemAtURL` fails rather than replaces when the destination exists.
        NSFileManager::defaultManager()
            .moveItemAtURL_toURL_error(&from, &to)
            .map_err(|error| match error.code() {
                code if code == NSFileWriteFileExistsError => already_exists(original),
                code if code == NSFileNoSuchFileError || code == NSFileReadNoSuchFileError => not_in_trash(original),
                _ => FsError::Restore {
                    path: original.to_path_buf(),
                    message: error.localizedDescription().to_string(),
                },
            })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use trash::os_limited;

    use super::{FsError, Result, Trashed, already_exists, not_in_trash, recreate_parent};

    /// Allowed between the recorded time and the item's own: both are whole seconds, and Linux stores local time,
    /// which `trash` converts back.
    const TOLERANCE_SECS: i64 = 2;

    pub(super) fn restore(trashed: &Trashed) -> Result<()> {
        let original = trashed.original_path();
        let refused = |error: trash::Error| FsError::Restore {
            path: original.to_path_buf(),
            message: error.to_string(),
        };

        let earliest = trashed.locator.trashed_at.saturating_sub(TOLERANCE_SECS);
        let item = os_limited::list()
            .map_err(refused)?
            .into_iter()
            .filter(|item| item.time_deleted >= earliest && same_path(&item.original_path(), original))
            .max_by_key(|item| item.time_deleted)
            .ok_or_else(|| not_in_trash(original))?;

        recreate_parent(original)?;

        os_limited::restore_all([item]).map_err(|error| match error {
            trash::Error::RestoreCollision { .. } => already_exists(original),
            error => refused(error),
        })
    }

    /// The recorded path is canonical, which on Windows carries the `\\?\` prefix the Recycle Bin doesn't, and
    /// Windows paths compare without case.
    #[cfg(windows)]
    fn same_path(a: &Path, b: &Path) -> bool {
        fn plain(path: &Path) -> String {
            let path = path.to_string_lossy();
            let path = match path.strip_prefix(r"\\?\UNC\") {
                Some(rest) => format!(r"\\{rest}"),
                None => path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned(),
            };
            path.to_lowercase()
        }
        plain(a) == plain(b)
    }

    #[cfg(not(windows))]
    fn same_path(a: &Path, b: &Path) -> bool {
        a == b
    }
}
