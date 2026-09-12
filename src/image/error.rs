use super::ImageFormat;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, ImageError>;

/// An error produced while detecting, decoding, or encoding an image.
///
/// Failures from the underlying codecs are wrapped per source: the `image` crate ([`ImageError::Image`]) for the
/// native formats, and the dedicated `webp`/`avif`/`heif` crates ([`ImageError::Webp`]/[`ImageError::Avif`]/
/// [`ImageError::Heif`]) for those formats. The remaining variants cover this module's own dispatch failures.
///
/// With the `image-raw` feature on, camera RAW adds two more: `ImageError::Raw` wraps the `zenraw` decoder's own
/// error the same way, and `ImageError::NotRaw` is this module's own refusal.
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
    /// A camera RAW decode or probe failed.
    ///
    /// Wraps the `zenraw` decoder's error, which carries the reason — an unsupported camera or compression, a
    /// malformed file, a pixel count over the decoder's limit. A camera the backend does not know arrives here
    /// rather than as a wrong picture.
    ///
    /// The `zenraw` API returns this inside a `whereat::At` location wrapper, which is dropped at this boundary:
    /// `At`'s own `Display` is just the inner error, so the wrapper adds nothing a caller can read, and keeping it
    /// would put a second crate into this enum's public signature for the sake of a trace only `zenraw` can decode.
    #[cfg(feature = "image-raw")]
    #[error("raw codec error: {0}")]
    Raw(#[from] ::zenraw::RawError),
    /// The bytes are not a camera RAW file.
    ///
    /// Distinct from [`ImageError::UnrecognizedFormat`], which means no format at all was recognized: this one
    /// means the bytes were recognizable and are not RAW, so a RAW decoder was asked for the wrong thing.
    #[cfg(feature = "image-raw")]
    #[error("the bytes are not a camera raw file")]
    NotRaw,
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
