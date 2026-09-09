use std::path::Path;

use super::copy_move::transfer;
use super::{CopyOptions, CopySummary, Result};

/// Copies each source into `dest_dir`, creating the destination if it does not exist.
///
/// A source may be a file or a directory. Files are copied directly into `dest_dir`; directories are walked according
/// to [`CopyOptions`] — one level or the whole tree, flattened or with their layout preserved. Existing destination
/// files are overwritten.
///
/// `sources` is any iterable of paths, so a `Vec<PathBuf>`, an array of `&str`, or the output of
/// [`list_path`](super::list_path) can all be passed straight in.
///
/// ```
/// use rust_sak::fs::{copy_files, mk_temp_dir, CopyOptions};
///
/// let from = mk_temp_dir("from")?;
/// let to = mk_temp_dir("to")?;
/// std::fs::write(from.path().join("a.txt"), b"hello")?;
/// std::fs::write(from.path().join("b.log"), b"ignored")?;
///
/// let summary = copy_files([from.path()], to.path(), &CopyOptions::new().extension("txt"))?;
/// assert_eq!(summary.files, 1);
/// assert_eq!(summary.bytes, 5);
/// assert_eq!(summary.skipped, 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if a source does not exist or cannot be read, or if the destination
/// cannot be written. The first failure stops the copy, leaving files already copied in place.
pub fn copy_files<I, P>(sources: I, dest_dir: impl AsRef<Path>, options: &CopyOptions) -> Result<CopySummary>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    transfer(sources, dest_dir, options, false)
}
