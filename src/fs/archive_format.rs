use std::path::Path;

/// An archive format this module can extract.
///
/// Mirrors [`ImageFormat`](crate::image::ImageFormat) in the [`image`](crate::image) module: the format is a value
/// a caller can name, hold and pass around, rather than something re-derived from a file name at every call site.
/// That matters when the format is known but the name is not — a `Content-Type` header, a body downloaded to a
/// [`mk_temp_file`](super::mk_temp_file) with no extension, or a path ending in `.bin`.
///
/// ```
/// use rust_sak::fs::ArchiveFormat;
///
/// assert_eq!(ArchiveFormat::from_path("photos.zip"), Some(ArchiveFormat::Zip));
/// assert_eq!(ArchiveFormat::from_path("backup.TAR.XZ"), Some(ArchiveFormat::TarXz));
/// assert_eq!(ArchiveFormat::from_path("notes.txt"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    /// ZIP (`.zip`).
    Zip,
    /// 7z (`.7z`).
    SevenZ,
    /// XZ-compressed TAR (`.tar.xz`, `.txz`).
    TarXz,
}

impl ArchiveFormat {
    /// Returns the format a file name implies, or `None` if it names no supported format.
    ///
    /// The whole file name is matched, case-insensitively, rather than [`Path::extension`] — which would see only
    /// `xz` in `backup.tar.xz` and could not tell it from a bare `.xz` stream.
    ///
    /// Nothing is sniffed from the file's contents. Guessing at the shape of a file the caller has told you nothing
    /// about is how an "archive" turns out to be something else entirely, so an unrecognised name is `None` here and
    /// an error in [`extract`](super::extract).
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        let name = path.as_ref().file_name()?.to_string_lossy().to_lowercase();

        // `.tar.xz` is checked before `.xz` would be, and `.zip`/`.7z` cannot collide with either.
        if name.ends_with(".tar.xz") || name.ends_with(".txz") {
            Some(ArchiveFormat::TarXz)
        } else if name.ends_with(".zip") {
            Some(ArchiveFormat::Zip)
        } else if name.ends_with(".7z") {
            Some(ArchiveFormat::SevenZ)
        } else {
            None
        }
    }
}
