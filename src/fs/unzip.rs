use std::borrow::Cow;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use zip::ZipArchive;

use super::extract::{EntryKind, Extractor, RawEntry, S_IFMT, S_IFREG, Totals};
use super::{ExtractOptions, ExtractSummary, Result};

/// Extracts a ZIP archive into `target_dir`, creating it if it does not exist.
///
/// Every entry is validated before anything is written: names that could escape the target directory are refused,
/// symbolic links are checked both textually and by resolving them through an anchored directory handle, and the
/// ceilings in [`ExtractOptions`] are enforced as the archive is read rather than after the damage is done.
///
/// Entry names are validated with this module's own rules rather than the `zip` crate's `enclosed_name`, so a
/// drive-lettered or device-named entry is rejected identically on every platform.
///
/// ```no_run
/// use rust_sak::fs::{unzip, ExtractOptions};
///
/// let summary = unzip("photos.zip", "/tmp/photos", &ExtractOptions::new().max_total_bytes(50 << 20))?;
/// println!("{} files, {} bytes", summary.files, summary.bytes);
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Zip`](super::FsError::Zip) if the archive is malformed,
/// [`FsError::IllegalPath`](super::FsError::IllegalPath) or
/// [`FsError::IllegalSymlink`](super::FsError::IllegalSymlink) if an entry would escape `target_dir`,
/// [`FsError::LimitExceeded`](super::FsError::LimitExceeded) if a configured ceiling is hit,
/// [`FsError::DeclaredSizeMismatch`](super::FsError::DeclaredSizeMismatch) if an entry outgrows its own header, and
/// [`FsError::Io`](super::FsError::Io) for anything the filesystem refuses.
///
/// Extraction stops at the first rejected entry. Entries already written stay on disk, but nothing has been written
/// outside `target_dir`.
pub fn unzip(
    archive: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    options: &ExtractOptions,
) -> Result<ExtractSummary> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(archive)?))?;

    // The central directory has already been read, so both totals are free. `decompressed_size` declines to answer
    // for an archive using data descriptors, where the sizes are not in the directory at all.
    let totals = Totals {
        entries: Some(zip.len() as u64),
        bytes: zip.decompressed_size().and_then(|bytes| u64::try_from(bytes).ok()),
    };
    let mut extractor = Extractor::new(target_dir.as_ref(), options, totals)?;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;

        // Every field has to be read out before `entry` is borrowed mutably as the content stream, or the immutable
        // and mutable borrows overlap.
        let name = entry.name().to_string();
        let size = entry.size();
        let mode = entry.unix_mode();
        let kind = classify(&entry, mode);

        let raw = RawEntry {
            name: Cow::Owned(name),
            kind,
            size,
            mode,
            // ZIP stores a link's target as the entry's contents, not in its header.
            link_target: None,
        };
        extractor.visit(raw, &mut entry)?;
    }

    extractor.finish()
}

/// Decides what a ZIP entry is.
///
/// The crate answers the two common questions directly. The mode is consulted only to catch the rarer case of an
/// archive carrying a device node or FIFO, which would otherwise be written out as an ordinary file. A mode with no
/// file-type bits at all is normal — plenty of tools write one — and means nothing more than "a regular file".
fn classify<R: std::io::Read>(entry: &zip::read::ZipFile<'_, R>, mode: Option<u32>) -> EntryKind {
    if entry.is_symlink() {
        return EntryKind::Symlink;
    }

    if entry.is_dir() {
        return EntryKind::Dir;
    }

    match mode {
        Some(mode) if mode & S_IFMT != 0 && mode & S_IFMT != S_IFREG => EntryKind::Other,
        _ => EntryKind::File,
    }
}
