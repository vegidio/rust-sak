use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use ::image::DynamicImage;

use super::dispatch::{encode_into, resolve_options};
use super::error::{ImageError, Result};
use super::{EncodeOptions, ImageFormat};

/// Encodes `image` and writes it to `path`, selecting the format from the file extension.
///
/// The encoded bytes are streamed to the file as they are produced rather than accumulated in memory first, so peak
/// memory is the decoded image plus a buffer, not the decoded image plus a whole encoded copy of it.
///
/// `options` may be `None` (encode with the format's defaults) or `Some(_)`.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension,
/// [`ImageError::FormatMismatch`] if `options` is for a different format than the extension names,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be written, and the codec's own error if encoding
/// fails.
pub fn encode_file(image: &DynamicImage, path: impl AsRef<Path>, options: Option<EncodeOptions>) -> Result<()> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let options = resolve_options(format, options)?;

    let mut writer = BufWriter::new(File::create(path)?);
    encode_into(image, options, &mut writer)?;

    // `BufWriter` swallows a write failure on drop, which would turn a full disk into a silently truncated image.
    std::io::Write::flush(&mut writer)?;

    Ok(())
}
