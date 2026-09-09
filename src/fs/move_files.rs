use std::path::Path;

use super::copy_move::transfer;
use super::{CopyOptions, CopySummary, Result};

/// Moves each source into `dest_dir` by copying it and then removing the original.
///
/// Identical to [`copy_files`](super::copy_files) in what it selects and where it puts things; the only difference is
/// that each source file is deleted once it has been copied. Copying rather than renaming means this works across
/// filesystems, where a rename fails.
///
/// **Only files are removed, never directories.** A directory source is left behind — possibly empty, possibly still
/// holding files the extension filter skipped — because deleting it would throw away data the call deliberately did
/// not move.
///
/// ```
/// use rust_sak::fs::{move_files, mk_temp_dir, CopyOptions};
///
/// let from = mk_temp_dir("from")?;
/// let to = mk_temp_dir("to")?;
/// let source = from.path().join("a.txt");
/// std::fs::write(&source, b"hello")?;
///
/// let summary = move_files([&source], to.path(), &CopyOptions::new())?;
/// assert_eq!(summary.files, 1);
/// assert!(!source.exists());
/// assert!(to.path().join("a.txt").exists());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if a source does not exist or cannot be read, the destination cannot
/// be written, or a source cannot be removed after being copied. The first failure stops the move; sources already
/// moved stay moved, and the rest are untouched.
pub fn move_files<I, P>(sources: I, dest_dir: impl AsRef<Path>, options: &CopyOptions) -> Result<CopySummary>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    transfer(sources, dest_dir, options, true)
}
