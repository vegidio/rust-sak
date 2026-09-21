use ::image::DynamicImage;

use super::dispatch::{DecodeFormat, decode_any};
use super::error::{ImageError, Result};

/// Decodes the encoded image in `bytes` into a [`DynamicImage`], guessing the format from its magic bytes.
///
/// With the `image-raw` feature on this also opens the camera RAW containers that identify themselves — CR3, RAF,
/// RW2 and ORF. The TIFF-based ones (NEF, ARW, CR2, PEF, DNG) cannot be recognized from bytes alone, because they
/// open exactly as an ordinary TIFF does, and are decoded as TIFF here; use
/// [`decode_file`](super::decode_file) or [`decode_raw_bytes`](super::decode_raw_bytes) for those.
///
/// # Errors
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches, and the codec's own error
/// if the bytes are not a valid image of the detected format.
pub fn decode_bytes(bytes: &[u8]) -> Result<DynamicImage> {
    let format = DecodeFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)?;

    decode_any(bytes, format)
}
