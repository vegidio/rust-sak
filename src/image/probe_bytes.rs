use super::ImageFormat;
use super::dispatch::probe_with_format;
use super::error::{ImageError, Result};
use super::info::ImageInfo;

/// Reads the metadata of the encoded image in `bytes` (dimensions, color type, bit depth) **without decoding
/// the pixels**, guessing the format from its magic bytes.
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches.
///
/// ```no_run
/// use rust_sak::image::probe_bytes;
///
/// let bytes = std::fs::read("photo.png")?;
/// let info = probe_bytes(&bytes)?;
/// println!("{}x{} {:?} @ {}-bit", info.width, info.height, info.color_type, info.bit_depth);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn probe_bytes(bytes: &[u8]) -> Result<ImageInfo> {
    let format = ImageFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)?;
    probe_with_format(bytes, format)
}
