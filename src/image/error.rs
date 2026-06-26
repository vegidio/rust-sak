use std::fmt;

use super::ImageFormat;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, ImageError>;

/// An error produced while detecting, decoding, or encoding an image.
///
/// Failures from the underlying codecs are wrapped per source: the `image` crate ([`ImageError::Image`]) for the
/// native formats, and the dedicated `webp`/`avif`/`heif` crates ([`ImageError::Webp`]/[`ImageError::Avif`]/
/// [`ImageError::Heif`]) for those formats. The remaining variants cover this module's own dispatch failures.
#[derive(Debug)]
pub enum ImageError {
    /// A native-format (`bmp`/`gif`/`jpeg`/`png`/`tiff`) decode or encode failed.
    Image(::image::ImageError),
    /// Reading from or writing to disk failed.
    Io(std::io::Error),
    /// A WebP decode or encode failed.
    Webp(::webp::WebpError),
    /// An AVIF decode or encode failed.
    Avif(::avif::AvifError),
    /// A HEIF/HEIC decode or encode failed.
    Heif(::heif::HeifError),
    /// A file path had no extension, or one that maps to no supported format.
    UnknownExtension,
    /// The bytes did not match the magic signature of any supported format.
    UnrecognizedFormat,
    /// The supplied [`EncodeOptions`](super::EncodeOptions) are for a different format than the encode target.
    FormatMismatch {
        /// The format being encoded to (from the file extension or the explicit argument).
        expected: ImageFormat,
        /// The format the supplied options actually tune.
        options: ImageFormat,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Image(err) => write!(f, "image codec error: {err}"),
            ImageError::Io(err) => write!(f, "image i/o error: {err}"),
            ImageError::Webp(err) => write!(f, "webp codec error: {err}"),
            ImageError::Avif(err) => write!(f, "avif codec error: {err}"),
            ImageError::Heif(err) => write!(f, "heif codec error: {err}"),
            ImageError::UnknownExtension => write!(f, "could not determine an image format from the file extension"),
            ImageError::UnrecognizedFormat => write!(f, "could not recognize the image format from the bytes"),
            ImageError::FormatMismatch { expected, options } => {
                write!(
                    f,
                    "encode options are for {options:?} but the target format is {expected:?}"
                )
            }
        }
    }
}

impl std::error::Error for ImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ImageError::Image(err) => Some(err),
            ImageError::Io(err) => Some(err),
            ImageError::Webp(err) => Some(err),
            ImageError::Avif(err) => Some(err),
            ImageError::Heif(err) => Some(err),
            ImageError::UnknownExtension | ImageError::UnrecognizedFormat | ImageError::FormatMismatch { .. } => None,
        }
    }
}

impl From<::image::ImageError> for ImageError {
    fn from(err: ::image::ImageError) -> Self {
        ImageError::Image(err)
    }
}

impl From<std::io::Error> for ImageError {
    fn from(err: std::io::Error) -> Self {
        ImageError::Io(err)
    }
}

impl From<::webp::WebpError> for ImageError {
    fn from(err: ::webp::WebpError) -> Self {
        ImageError::Webp(err)
    }
}

impl From<::avif::AvifError> for ImageError {
    fn from(err: ::avif::AvifError) -> Self {
        ImageError::Avif(err)
    }
}

impl From<::heif::HeifError> for ImageError {
    fn from(err: ::heif::HeifError) -> Self {
        ImageError::Heif(err)
    }
}
