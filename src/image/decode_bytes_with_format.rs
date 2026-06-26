use ::image::DynamicImage;

use super::ImageFormat;
use super::dispatch::decode_with_format;
use super::error::Result;

/// Decodes the encoded image in `bytes` into a [`DynamicImage`] using the caller-supplied `format`.
///
/// Use this when the format is already known and should not be guessed from the bytes.
pub fn decode_bytes_with_format(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage> {
    decode_with_format(bytes, format)
}
