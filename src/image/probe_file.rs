use std::fs;
use std::path::Path;

use super::ImageFormat;
use super::dispatch::probe_with_format;
use super::error::{ImageError, Result};
use super::info::ImageInfo;

/// Reads the metadata of the image at `path` (dimensions, color type, bit depth) **without decoding the
/// pixels**, selecting the codec from the file extension.
///
/// The file's bytes are read from disk, but only its header is parsed — the pixel data is never decoded.
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension.
///
/// ```no_run
/// use rust_sak::image::probe_file;
///
/// let info = probe_file("photo.png")?;
/// println!("{}x{} {:?} @ {}-bit", info.width, info.height, info.color_type, info.bit_depth);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn probe_file(path: impl AsRef<Path>) -> Result<ImageInfo> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let bytes = fs::read(path)?;
    probe_with_format(&bytes, format)
}
