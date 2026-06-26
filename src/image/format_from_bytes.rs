use super::ImageFormat;
use super::error::{ImageError, Result};

/// Detects the [`ImageFormat`] of an encoded image from its leading magic bytes, **without decoding** the pixels.
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches.
pub fn format_from_bytes(bytes: &[u8]) -> Result<ImageFormat> {
    ImageFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)
}
