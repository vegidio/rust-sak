use ::image::DynamicImage;

use super::error::{ImageError, Result};
use super::raw_dispatch::decode_raw;

/// Decodes the camera RAW file in `bytes` into a [`DynamicImage`], developing it into a display-ready picture.
///
/// The result is always [`DynamicImage::ImageRgb16`] — 16 bits per channel, sRGB. The decoder runs its full
/// pipeline to get there: black/white-level normalisation, demosaic, white balance, the camera's colour matrix,
/// tone curve and sRGB gamma, then the crop and EXIF orientation the camera recorded. What comes back is the
/// photograph, not the sensor readings.
///
/// Unlike the other eight formats, the format is not guessed from the extension or the magic bytes and does not
/// need to be: the backend identifies the camera from the file itself.
///
/// # Errors
///
/// Returns [`ImageError::NotRaw`] if the bytes are not a RAW file, and [`ImageError::Raw`] if they are but cannot
/// be decoded — most often a camera the backend does not know, which is reported rather than guessed at.
///
/// ```no_run
/// use rust_sak::image::decode_raw_bytes;
///
/// let bytes = std::fs::read("DSC_0001.NEF")?;
/// let image = decode_raw_bytes(&bytes)?;
/// println!("{}x{} developed", image.width(), image.height());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn decode_raw_bytes(bytes: &[u8]) -> Result<DynamicImage> {
    if !::zenraw::is_raw_file(bytes) {
        return Err(ImageError::NotRaw);
    }
    decode_raw(bytes)
}
