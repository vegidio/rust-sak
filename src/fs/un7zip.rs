use std::borrow::Cow;
use std::path::Path;

use sevenz_rust2::{ArchiveReader, Password};

use super::extract::{EntryKind, Extractor, RawEntry, S_IFLNK, S_IFMT};
use super::{ExtractOptions, ExtractSummary, Result};

/// The flag p7zip sets in `windows_attributes` to mean "the high 16 bits are a Unix mode".
const UNIX_EXTENSION: u32 = 0x8000;

/// Extracts a 7z archive into `target_dir`, creating it if it does not exist.
///
/// Applies the same validation as [`unzip`](super::unzip) and [`untar_xz`](super::untar_xz).
///
/// Only unencrypted archives are supported; an encrypted one fails rather than prompting for anything.
///
/// ```no_run
/// use rust_sak::fs::{un7zip, ExtractOptions};
///
/// let summary = un7zip("release.7z", "/tmp/release", &ExtractOptions::new())?;
/// println!("{} files", summary.files);
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::SevenZ`](super::FsError::SevenZ) if the archive is malformed or encrypted, and otherwise as
/// [`unzip`](super::unzip): [`FsError::IllegalPath`](super::FsError::IllegalPath), [`FsError::IllegalSymlink`](super::FsError::IllegalSymlink), [`FsError::LimitExceeded`](super::FsError::LimitExceeded),
/// [`FsError::DeclaredSizeMismatch`](super::FsError::DeclaredSizeMismatch) and [`FsError::Io`](super::FsError::Io).
pub fn un7zip(
    archive: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    options: &ExtractOptions,
) -> Result<ExtractSummary> {
    let mut reader = ArchiveReader::open(archive.as_ref(), Password::empty())?;
    let mut extractor = Extractor::new(target_dir.as_ref(), options)?;

    // `for_each_entries` can only carry a `sevenz_rust2::Error` back out, and ours are richer than that. Parking the
    // failure here and stopping the walk cleanly with `Ok(false)` keeps the real reason intact.
    let mut failure = None;

    let walk = reader.for_each_entries(|entry, data| {
        let (kind, mode) = classify(
            entry.is_directory,
            entry.has_windows_attributes,
            entry.windows_attributes,
        );
        let raw = RawEntry {
            name: Cow::Borrowed(entry.name.as_str()),
            kind,
            size: entry.size,
            mode,
            // Like ZIP, 7z stores a link's target as the entry's contents.
            link_target: None,
        };

        match extractor.visit(raw, data) {
            Ok(()) => Ok(true),
            Err(err) => {
                failure = Some(err);
                Ok(false)
            }
        }
    });

    // Ours first: stopping the walk makes it report its own, less specific, complaint.
    if let Some(err) = failure {
        return Err(err);
    }
    walk?;

    extractor.finish()
}

/// Decodes what a 7z entry is, and the Unix mode it carries, from the p7zip attribute convention.
///
/// 7z has no native notion of a symbolic link or a Unix mode. p7zip smuggles both through the Windows attribute
/// word: setting `0x8000` declares that the high 16 bits hold an `st_mode`. `sevenz-rust2` preserves the raw word but
/// never interprets it, so the decoding is ours to do.
///
/// Without that flag there is no mode and — importantly — **never a symbolic link**, which is exactly the case for
/// archives written on Windows or by `sevenz-rust2` itself.
pub(super) fn classify(
    is_directory: bool,
    has_windows_attributes: bool,
    windows_attributes: u32,
) -> (EntryKind, Option<u32>) {
    if is_directory {
        return (EntryKind::Dir, unix_mode(has_windows_attributes, windows_attributes));
    }

    match unix_mode(has_windows_attributes, windows_attributes) {
        Some(mode) if mode & S_IFMT == S_IFLNK => (EntryKind::Symlink, Some(mode)),
        mode => (EntryKind::File, mode),
    }
}

/// Extracts the `st_mode` p7zip packs into the high half of the attribute word, if it is there at all.
fn unix_mode(has_windows_attributes: bool, windows_attributes: u32) -> Option<u32> {
    if has_windows_attributes && windows_attributes & UNIX_EXTENSION != 0 {
        Some(windows_attributes >> 16)
    } else {
        None
    }
}
