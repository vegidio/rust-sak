use std::fs;
use std::path::Path;

use ::image::DynamicImage;

use super::ImageFormat;
use super::dispatch::decode_with_format;
use super::error::{ImageError, Result};

/// Decodes the image at `path` into a [`DynamicImage`], selecting the codec from the file extension.
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension.
pub fn decode_file(path: impl AsRef<Path>) -> Result<DynamicImage> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let bytes = fs::read(path)?;
    decode_with_format(&bytes, format)
}
