use std::io::Write;

use ::image::DynamicImage;

use super::dispatch::{encode_into, resolve_options};
use super::error::Result;
use super::{EncodeOptions, ImageFormat};

/// Encodes `image` in the given `format` and writes the bytes to `writer`.
///
/// Unlike [`encode_file`](super::encode_file), a writer carries no file extension, so the target `format` is explicit.
/// `options` may be `None` (encode with the format's defaults) or `Some(_)`.
///
/// # Errors
///
/// Returns [`ImageError::FormatMismatch`](super::ImageError::FormatMismatch) if `options` is for a different format
/// than `format`, [`ImageError::Io`](super::ImageError::Io) if `writer` fails, and the codec's own error if encoding
/// fails.
///
/// Most formats are streamed straight into `writer`. BMP, GIF and TIFF are the exception — their encoder needs
/// [`Seek`](std::io::Seek), which a bare [`Write`] cannot provide, so those three are buffered whole before being
/// written. Pass a [`File`](std::fs::File) via [`encode_file`](super::encode_file) to stream them too.
pub fn encode_writer(
    image: &DynamicImage,
    writer: &mut impl Write,
    format: ImageFormat,
    options: Option<EncodeOptions>,
) -> Result<()> {
    let options = resolve_options(format, options)?;
    encode_into(image, options, writer)
}
