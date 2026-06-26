use std::io::Write;

use ::image::DynamicImage;

use super::dispatch::{encode_to_vec, resolve_options};
use super::error::Result;
use super::{EncodeOptions, ImageFormat};

/// Encodes `image` in the given `format` and writes the bytes to `writer`.
///
/// Unlike [`encode_file`](super::encode_file), a writer carries no file extension, so the target `format` is explicit.
/// `options` may be `None` (encode with the format's defaults) or `Some(_)`; a `Some` whose variant is for a different
/// format than `format` yields [`ImageError::FormatMismatch`](super::ImageError::FormatMismatch).
pub fn encode_writer(
    image: &DynamicImage,
    writer: &mut impl Write,
    format: ImageFormat,
    options: Option<EncodeOptions>,
) -> Result<()> {
    let options = resolve_options(format, options)?;
    let bytes = encode_to_vec(image, options)?;
    writer.write_all(&bytes)?;
    Ok(())
}
