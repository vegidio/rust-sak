use super::ImageFormat;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, ImageError>;

/// An error produced while detecting, decoding, or encoding an image.
///
/// Failures from the underlying codecs are wrapped per source: the `image` crate ([`ImageError::Image`]) for the
/// native formats, and the dedicated `webp`/`avif`/`heif` crates ([`ImageError::Webp`]/[`ImageError::Avif`]/
/// [`ImageError::Heif`]) for those formats. The remaining variants cover this module's own dispatch failures.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// A native-format (`bmp`/`gif`/`jpeg`/`png`/`tiff`) decode or encode failed.
    #[error("image codec error: {0}")]
    Image(#[from] ::image::ImageError),
    /// Reading from or writing to disk failed.
    #[error("image i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A WebP decode or encode failed.
    #[error("webp codec error: {0}")]
    Webp(#[from] ::webp::WebpError),
    /// An AVIF decode or encode failed.
    #[error("avif codec error: {0}")]
    Avif(#[from] ::avif::AvifError),
    /// A HEIF/HEIC decode or encode failed.
    #[error("heif codec error: {0}")]
    Heif(#[from] ::heif::HeifError),
    /// A file path had no extension, or one that maps to no supported format.
    #[error("could not determine an image format from the file extension")]
    UnknownExtension,
    /// The bytes did not match the magic signature of any supported format.
    #[error("could not recognize the image format from the bytes")]
    UnrecognizedFormat,
    /// The supplied [`EncodeOptions`](super::EncodeOptions) are for a different format than the encode target.
    #[error("encode options are for {options:?} but the target format is {expected:?}")]
    FormatMismatch {
        /// The format being encoded to (from the file extension or the explicit argument).
        expected: ImageFormat,
        /// The format the supplied options actually tune.
        options: ImageFormat,
    },
}
