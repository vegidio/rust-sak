use std::path::Path;

/// A supported image format.
///
/// Unlike [`image::ImageFormat`](::image::ImageFormat), this enum has a dedicated [`Heif`](ImageFormat::Heif) variant
/// and treats `avif`/`heif`/`webp` as first-class formats handled by their dedicated codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// Windows Bitmap (`.bmp`).
    Bmp,
    /// Graphics Interchange Format (`.gif`).
    Gif,
    /// JPEG (`.jpg`/`.jpeg`).
    Jpeg,
    /// Portable Network Graphics (`.png`).
    Png,
    /// Tagged Image File Format (`.tif`/`.tiff`).
    Tiff,
    /// AV1 Image File Format (`.avif`).
    Avif,
    /// High Efficiency Image Format (`.heif`/`.heic`).
    Heif,
    /// WebP (`.webp`).
    WebP,
}

impl ImageFormat {
    /// Every supported format, in declaration order.
    pub const ALL: [ImageFormat; 8] = [
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Tiff,
        ImageFormat::Avif,
        ImageFormat::Heif,
        ImageFormat::WebP,
    ];

    /// Returns the format associated with the given file-name extension (case-insensitive, no leading dot), or `None`
    /// if the extension does not map to a supported format.
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| {
            format
                .extensions()
                .iter()
                .any(|ext| ext.eq_ignore_ascii_case(extension))
        })
    }

    /// Returns the format inferred from a path's extension, or `None` if it has no recognized extension.
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        path.as_ref()
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// Guesses the format from the leading "magic" bytes of an encoded image, or `None` if no signature matches.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(ImageFormat::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(ImageFormat::Jpeg);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(ImageFormat::Gif);
        }
        if bytes.starts_with(b"BM") {
            return Some(ImageFormat::Bmp);
        }
        if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
            return Some(ImageFormat::Tiff);
        }
        // WebP is a RIFF container whose form type (bytes 8..12) is "WEBP".
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(ImageFormat::WebP);
        }
        // AVIF and HEIF are ISO-BMFF containers: bytes 4..8 are "ftyp", bytes 8..12 are the major brand.
        if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            return match &bytes[8..12] {
                b"avif" | b"avis" => Some(ImageFormat::Avif),
                b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => {
                    Some(ImageFormat::Heif)
                }
                _ => None,
            };
        }
        None
    }

    /// The canonical lowercase file extension for this format (without a leading dot).
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Bmp => "bmp",
            ImageFormat::Gif => "gif",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Avif => "avif",
            ImageFormat::Heif => "heif",
            ImageFormat::WebP => "webp",
        }
    }

    /// Every lowercase file extension (without a leading dot) that selects this format; the first is
    /// [`extension`](ImageFormat::extension).
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            ImageFormat::Bmp => &["bmp"],
            ImageFormat::Gif => &["gif"],
            ImageFormat::Jpeg => &["jpg", "jpeg"],
            ImageFormat::Png => &["png"],
            ImageFormat::Tiff => &["tiff", "tif"],
            ImageFormat::Avif => &["avif"],
            ImageFormat::Heif => &["heif", "heic"],
            ImageFormat::WebP => &["webp"],
        }
    }

    /// Maps the native formats to the `image` crate's [`ImageFormat`](::image::ImageFormat); returns `None` for the
    /// formats handled by their dedicated codecs (`avif`/`heif`/`webp`).
    pub(super) fn to_image_format(self) -> Option<::image::ImageFormat> {
        let format = match self {
            ImageFormat::Bmp => ::image::ImageFormat::Bmp,
            ImageFormat::Gif => ::image::ImageFormat::Gif,
            ImageFormat::Jpeg => ::image::ImageFormat::Jpeg,
            ImageFormat::Png => ::image::ImageFormat::Png,
            ImageFormat::Tiff => ::image::ImageFormat::Tiff,
            ImageFormat::Avif | ImageFormat::Heif | ImageFormat::WebP => return None,
        };
        Some(format)
    }
}
