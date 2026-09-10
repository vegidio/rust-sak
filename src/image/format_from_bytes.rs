use super::ImageFormat;
use super::error::{ImageError, Result};

/// Detects the [`ImageFormat`] of an encoded image from its leading magic bytes, **without decoding** the pixels.
///
/// # Errors
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches. This is the only way it can
/// fail: nothing is decoded, so a truncated or corrupt body whose signature is intact still reports its format.
pub fn format_from_bytes(bytes: &[u8]) -> Result<ImageFormat> {
    ImageFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)
}
