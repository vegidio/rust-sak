use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{CopyOptions, CopySummary, ListOptions, Result, list_path};

/// Copies every source into `dest_dir`, optionally removing each source once it has been copied.
///
/// This is the whole of both [`copy_files`](super::copy_files) and [`move_files`](super::move_files); the only
/// difference between them is `remove_sources`. A move renames where it can and falls back to copy-then-delete
/// across filesystems — see [`rename_or_copy`] — so an interrupted move never loses data that was not already
/// written to the destination.
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

    // Every destination parent already created, so a run of files landing in one directory costs a single
    // `create_dir_all` rather than one per file. `dest_dir` is seeded because it was just created, which is the
    // whole set when `preserve_structure` is off.
    let mut created: HashSet<PathBuf> = HashSet::from([dest_dir.to_path_buf()]);

    let mut summary = CopySummary::default();
    for source in sources {
        let source = source.as_ref();

        // `metadata` follows symlinks, so a link to a directory is walked as a directory and a link to a file is
        // copied as a file — matching what the caller sees when they look at the path themselves.
        if fs::metadata(source)?.is_dir() {
            transfer_dir(source, dest_dir, options, remove_sources, &mut created, &mut summary)?;
        } else {
            let name = file_name(source)?;
            transfer_file(
                source,
                &dest_dir.join(name),
                options,
                remove_sources,
                &mut created,
                &mut summary,
            )?;
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
    created: &mut HashSet<PathBuf>,
    summary: &mut CopySummary,
) -> Result<()> {
    // Deliberately unfiltered: the extension filter is applied in `transfer_file` instead, so that excluded files
    // are seen and counted in `CopySummary::skipped`. Filtering them out of the walk would make that counter zero.
    let listing = ListOptions::new().recursive(options.recursive);
    for path in list_path(source, &listing)? {
        // `list_path` walked from `source`, so every path it returns is under it and the strip cannot fail.
        let relative = path.strip_prefix(source).unwrap_or(&path);
        let dest = if options.preserve_structure {
            dest_dir.join(relative)
        } else {
            dest_dir.join(file_name(&path)?)
        };

        transfer_file(&path, &dest, options, remove_sources, created, summary)?;
    }

    Ok(())
}

/// Copies one file, creating its parent directory, and removes the source afterwards when moving.
fn transfer_file(
    source: &Path,
    dest: &Path,
    options: &CopyOptions,
    remove_sources: bool,
    created: &mut HashSet<PathBuf>,
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

    if let Some(parent) = dest.parent()
        && !created.contains(parent)
    {
        fs::create_dir_all(parent)?;
        created.insert(parent.to_path_buf());
    }

    summary.bytes += if remove_sources {
        rename_or_copy(source, dest)?
    } else {
        fs::copy(source, dest)?
    };
    summary.files += 1;

    Ok(())
}

/// Moves `source` onto `dest`, returning the number of bytes the file holds.
///
/// A rename costs a directory update; a copy reads and writes every byte, so for a large file the difference is the
/// whole transfer. `rename` is also atomic, which removes the window in which a copy has succeeded but the delete has
/// not and the file exists in both places at once.
///
/// The one thing a rename cannot do is cross a filesystem boundary, which is what the fallback is for — and the only
/// case in which the old copy-then-delete behaviour still applies.
fn rename_or_copy(source: &Path, dest: &Path) -> Result<u64> {
    // Read the length first: after a successful rename there is nothing left at `source` to stat, and unlike
    // `fs::copy` a rename does not report how much it moved.
    let len = fs::metadata(source)?.len();

    match fs::rename(source, dest) {
        Ok(()) => Ok(len),
        // The two paths are on different filesystems, so the bytes genuinely have to be carried across.
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            let copied = fs::copy(source, dest)?;
            fs::remove_file(source)?;
            Ok(copied)
        }
        Err(err) => Err(err.into()),
    }
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
