use std::path::Path;

use super::{ArchiveFormat, ExtractOptions, ExtractSummary, FsError, Result, un7zip, untar_xz, unzip};

/// Extracts an archive into `target_dir`, choosing the format from the file's extension.
///
/// Recognised, case-insensitively: `.zip`, `.7z`, `.tar.xz` and `.txz`. Anything else is an error rather than a
/// guess — sniffing the contents of a file the caller has told you nothing about is how an "archive" turns out to be
/// something else entirely.
///
/// The extension is matched against the whole file name, not [`Path::extension`], which would see only `xz` in
/// `backup.tar.xz` and could not tell it from a bare `.xz` stream.
///
/// Behaviour is otherwise identical to calling [`unzip`], [`un7zip`] or [`untar_xz`] directly; see [`unzip`] for what
/// the validation actually guarantees.
///
/// ```no_run
/// use rust_sak::fs::{extract, ExtractOptions};
///
/// // The same call handles every supported format.
/// for archive in ["photos.zip", "backup.tar.xz", "release.7z"] {
///     extract(archive, "/tmp/out", &ExtractOptions::new())?;
/// }
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::UnknownArchiveFormat`] if the extension is not one of the four above, and otherwise whatever
/// the format's own function returns.
pub fn extract(
    archive: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    options: &ExtractOptions,
) -> Result<ExtractSummary> {
    let archive = archive.as_ref();
    let format = ArchiveFormat::from_path(archive).ok_or_else(|| FsError::UnknownArchiveFormat {
        name: archive
            .file_name()
            .unwrap_or(archive.as_os_str())
            .to_string_lossy()
            .into_owned(),
    })?;

    extract_as(format, archive, target_dir, options)
}

/// Extracts an archive whose format the caller already knows, ignoring the file's name entirely.
///
/// This is [`extract`] without the name-based guess, for when the format came from somewhere more reliable than an
/// extension — a `Content-Type` header, the source the bytes were fetched from, or a
/// [`mk_temp_file`](super::mk_temp_file) that has no meaningful name at all.
///
/// ```no_run
/// use rust_sak::fs::{extract_as, ArchiveFormat, ExtractOptions};
///
/// // The download had no usable extension, but the server said what it was.
/// extract_as(ArchiveFormat::Zip, "/tmp/download.bin", "/tmp/out", &ExtractOptions::new())?;
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Whatever the format's own function returns — see [`unzip`], [`un7zip`] and [`untar_xz`].
pub fn extract_as(
    format: ArchiveFormat,
    archive: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    options: &ExtractOptions,
) -> Result<ExtractSummary> {
    match format {
        ArchiveFormat::Zip => unzip(archive, target_dir, options),
        ArchiveFormat::SevenZ => un7zip(archive, target_dir, options),
        ArchiveFormat::TarXz => untar_xz(archive, target_dir, options),
    }
}
