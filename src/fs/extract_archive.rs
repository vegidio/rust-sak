use std::path::Path;

use super::{ExtractOptions, ExtractSummary, FsError, Result, un7zip, untar_xz, unzip};

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
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // `.tar.xz` is checked before `.xz` would be, and `.zip`/`.7z` cannot collide with either.
    if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        untar_xz(archive, target_dir, options)
    } else if name.ends_with(".zip") {
        unzip(archive, target_dir, options)
    } else if name.ends_with(".7z") {
        un7zip(archive, target_dir, options)
    } else {
        Err(FsError::UnknownArchiveFormat)
    }
}
