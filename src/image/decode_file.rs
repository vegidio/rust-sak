use std::fs;
use std::path::Path;

use ::image::DynamicImage;

use super::dispatch::{DecodeFormat, decode_any};
use super::error::{ImageError, Result};

/// Decodes the image at `path` into a [`DynamicImage`], selecting the codec from the file extension.
///
/// With the `image-raw` feature on this also opens camera RAW: an extension naming a RAW format routes to the RAW
/// decoder, so a caller handling whatever a user dropped in does not have to work out which of the two entry points
/// a file needs. [`decode_raw_file`](super::decode_raw_file) remains as the narrow form, for a caller that wants
/// anything but RAW to be refused.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be read, and the codec's own error if its contents
/// are not a valid image of that format.
pub fn decode_file(path: impl AsRef<Path>) -> Result<DynamicImage> {
    let path = path.as_ref();
    let format = DecodeFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let bytes = fs::read(path)?;

    decode_any(&bytes, format)
}
