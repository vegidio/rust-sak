use std::fs;
use std::path::Path;

use ::image::DynamicImage;

use super::dispatch::{encode_to_vec, resolve_options};
use super::error::{ImageError, Result};
use super::{EncodeOptions, ImageFormat};

/// Encodes `image` and writes it to `path`, selecting the format from the file extension.
///
/// `options` may be `None` (encode with the format's defaults) or `Some(_)`; a `Some` whose variant is for a different
/// format than the extension yields [`ImageError::FormatMismatch`]. An unrecognized extension yields
/// [`ImageError::UnknownExtension`].
pub fn encode_file(image: &DynamicImage, path: impl AsRef<Path>, options: Option<EncodeOptions>) -> Result<()> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let options = resolve_options(format, options)?;
    let bytes = encode_to_vec(image, options)?;
    fs::write(path, bytes)?;
    Ok(())
}
