use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::{ListOptions, Result};

/// Lists the entries of `directory`.
///
/// What comes back is governed by [`ListOptions`]: files and/or directories, one level or the whole tree, optionally
/// filtered by extension. Results are **sorted by file name within each directory**, so repeated calls return the
/// same order on every platform — worth having, since the underlying `readdir` order is arbitrary.
///
/// Returned paths keep the prefix they were given: an absolute `directory` yields absolute paths, a relative one
/// yields relative paths. That is what lets a listing be handed straight to [`copy_files`](super::copy_files).
///
/// Symbolic links are **not followed**, which keeps a link cycle from making a recursive listing run forever. A link
/// is reported as a file even when it points at a directory, and never descended into.
///
/// ```
/// use rust_sak::fs::{list_path, mk_temp_dir, ListOptions};
///
/// let dir = mk_temp_dir("example")?;
/// std::fs::write(dir.path().join("a.txt"), b"a")?;
/// std::fs::write(dir.path().join("b.log"), b"b")?;
///
/// let text = list_path(dir.path(), &ListOptions::new().extension("txt"))?;
/// assert_eq!(text.len(), 1);
/// assert!(text[0].ends_with("a.txt"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if `directory` cannot be read, and one of kind
/// [`ErrorKind::NotADirectory`](std::io::ErrorKind::NotADirectory) if it exists but is not a directory — a regular
/// file is an error, not an empty listing. A symbolic link to a directory counts as that directory. An entry that cannot be read *during*
/// the walk — a directory whose permissions deny listing, say — fails the whole call rather than being silently
/// dropped, so a partial listing is never mistaken for a complete one.
pub fn list_path(directory: impl AsRef<Path>, options: &ListOptions) -> Result<Vec<PathBuf>> {
    let directory = directory.as_ref();

    // Checked up front because `min_depth(1)` skips the root entry: a regular file would otherwise walk to nothing
    // and come back as a silent empty listing.
    if !std::fs::metadata(directory)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a directory", directory.display()),
        )
        .into());
    }

    let max_depth = if options.recursive { usize::MAX } else { 1 };

    let mut paths = Vec::new();
    for entry in WalkDir::new(directory)
        .min_depth(1)
        .max_depth(max_depth)
        .sort_by_file_name()
    {
        let entry = entry.map_err(io::Error::from)?;

        // Anything that is not a directory counts as a file here, including symlinks and FIFOs — the alternative is
        // silently dropping entries that plainly exist.
        let include = if entry.file_type().is_dir() {
            options.dirs
        } else {
            options.accepts_file(entry.path())
        };

        if include {
            paths.push(entry.into_path());
        }
    }

    Ok(paths)
}
