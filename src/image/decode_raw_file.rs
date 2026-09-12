use std::fs;
use std::path::Path;

use ::image::DynamicImage;

use super::error::{ImageError, Result};
use super::raw_dispatch::decode_raw;

/// Decodes the camera RAW file at `path` into a [`DynamicImage`], developing it into a display-ready picture.
///
/// See [`decode_raw_bytes`](super::decode_raw_bytes) for what "developing" covers and why the result is always
/// [`DynamicImage::ImageRgb16`].
///
/// The whole file is read, because a RAW decode needs all of it. Unlike
/// [`decode_file`](super::decode_file) the extension is not what selects the codec — the backend identifies the
/// camera from the contents — but it is still checked, so a PNG handed to this function is refused by name rather
/// than by failing somewhere inside the decoder.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized RAW extension,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be read, [`ImageError::NotRaw`] if its contents are
/// not RAW, and [`ImageError::Raw`] if the decode itself fails.
///
/// ```no_run
/// use rust_sak::image::decode_raw_file;
///
/// let image = decode_raw_file("DSC_0001.NEF")?;
/// println!("{}x{} developed", image.width(), image.height());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn decode_raw_file(path: impl AsRef<Path>) -> Result<DynamicImage> {
    let path = path.as_ref();
    if super::RawFormat::from_path(path).is_none() {
        return Err(ImageError::UnknownExtension);
    }
    let bytes = fs::read(path)?;
    if !::zenraw::is_raw_file(&bytes) {
        return Err(ImageError::NotRaw);
    }
    decode_raw(&bytes)
}
