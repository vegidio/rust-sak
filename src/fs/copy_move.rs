use std::fs;
use std::path::{Path, PathBuf};

use super::{CopyOptions, CopySummary, ListOptions, Result, list_path};

/// Copies every source into `dest_dir`, optionally removing each source once it has been copied.
///
/// This is the whole of both [`copy_files`](super::copy_files) and [`move_files`](super::move_files); the only
/// difference between them is `remove_sources`. Removal happens per file, immediately after that file has been
/// copied, so an interrupted move never loses data that was not already written to the destination.
///
/// # Errors
///
/// Returns [`FsError::Io`](super::FsError::Io) if a source cannot be read, the destination cannot be created, or a
/// copy fails. The first failure stops the transfer, leaving the files already copied in place.
pub(super) fn transfer<I, P>(
    sources: I,
    dest_dir: impl AsRef<Path>,
    options: &CopyOptions,
    remove_sources: bool,
) -> Result<CopySummary>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let dest_dir = dest_dir.as_ref();
    fs::create_dir_all(dest_dir)?;

    let mut summary = CopySummary::default();
    for source in sources {
        let source = source.as_ref();

        // `metadata` follows symlinks, so a link to a directory is walked as a directory and a link to a file is
        // copied as a file — matching what the caller sees when they look at the path themselves.
        if fs::metadata(source)?.is_dir() {
            transfer_dir(source, dest_dir, options, remove_sources, &mut summary)?;
        } else {
            let name = file_name(source)?;
            transfer_file(source, &dest_dir.join(name), options, remove_sources, &mut summary)?;
        }
    }

    Ok(summary)
}

/// Transfers the files inside a directory source.
fn transfer_dir(
    source: &Path,
    dest_dir: &Path,
    options: &CopyOptions,
    remove_sources: bool,
    summary: &mut CopySummary,
) -> Result<()> {
    let listing = ListOptions::new().recursive(options.recursive);
    for path in list_path(source, &listing)? {
        // `list_path` walked from `source`, so every path it returns is under it and the strip cannot fail.
        let relative = path.strip_prefix(source).unwrap_or(&path);
        let dest = if options.preserve_structure {
            dest_dir.join(relative)
        } else {
            dest_dir.join(file_name(&path)?)
        };

        transfer_file(&path, &dest, options, remove_sources, summary)?;
    }

    Ok(())
}

/// Copies one file, creating its parent directory, and removes the source afterwards when moving.
fn transfer_file(
    source: &Path,
    dest: &Path,
    options: &CopyOptions,
    remove_sources: bool,
    summary: &mut CopySummary,
) -> Result<()> {
    if !options.accepts(source) {
        summary.skipped += 1;
        return Ok(());
    }

    // Copying a file onto itself would truncate it to nothing before reading a byte. This catches the literal case;
    // two different spellings of the same path still collide, so transferring a directory into itself is unsupported.
    if source == dest {
        summary.skipped += 1;
        return Ok(());
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    summary.bytes += fs::copy(source, dest)?;
    summary.files += 1;

    if remove_sources {
        fs::remove_file(source)?;
    }

    Ok(())
}

/// Returns a path's final component, rejecting paths that have none.
fn file_name(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("source path has no file name: {}", path.display()),
        )
    })?;

    Ok(PathBuf::from(name))
}
