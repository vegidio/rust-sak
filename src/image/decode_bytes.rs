use ::image::DynamicImage;

use super::ImageFormat;
use super::dispatch::decode_with_format;
use super::error::{ImageError, Result};

/// Decodes the encoded image in `bytes` into a [`DynamicImage`], guessing the format from its magic bytes.
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches.
pub fn decode_bytes(bytes: &[u8]) -> Result<DynamicImage> {
    let format = ImageFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)?;
    decode_with_format(bytes, format)
}
