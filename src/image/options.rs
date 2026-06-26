use super::ImageFormat;

pub use ::heif::{Chroma, Preset};

/// Per-format encoding options.
///
/// Each variant carries the tunables that make sense for that format and **names the format it encodes to** — so the
/// variant doubles as the format selector. Every variant has a [`Default`] (via [`EncodeOptions::default_for`]); the
/// encode functions accept an `Option<EncodeOptions>`, so `None` encodes with these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeOptions {
    /// BMP encoding (no tunable parameters).
    Bmp,
    /// GIF encoding (no tunable parameters).
    Gif,
    /// JPEG encoding.
    Jpeg {
        /// Quality from 1 (worst) to 100 (best).
        quality: u8,
    },
    /// PNG encoding.
    Png {
        /// Compression effort.
        compression: PngCompression,
        /// Scanline filter strategy.
        filter: PngFilter,
    },
    /// TIFF encoding (no tunable parameters).
    Tiff,
    /// AVIF encoding (via the `avif` crate).
    Avif {
        /// Quality from 0 (worst) to 100 (best).
        quality: u8,
        /// Encoding speed from 0 (slowest, best compression) to 10 (fastest).
        speed: u8,
        /// Worker-thread count, or `None` to let the encoder auto-detect.
        threads: Option<u32>,
    },
    /// HEIF/HEIC encoding (via the `heif` crate).
    Heif {
        /// Quality from 0 (worst) to 100 (best).
        quality: u8,
        /// x265 speed preset.
        preset: Preset,
        /// Chroma subsampling.
        chroma: Chroma,
    },
    /// WebP encoding (via the `webp` crate).
    WebP {
        /// Quality from 0 (worst) to 100 (best); ignored when `lossless` is set.
        quality: u8,
        /// Alpha-channel quality from 0 to 100.
        quality_alpha: u8,
        /// Compression effort from 0 (fastest) to 6 (smallest).
        compression: u8,
        /// Encode losslessly.
        lossless: bool,
        /// Use multiple threads.
        threads: bool,
    },
}

impl EncodeOptions {
    /// The format this set of options encodes to.
    pub fn format(self) -> ImageFormat {
        match self {
            EncodeOptions::Bmp => ImageFormat::Bmp,
            EncodeOptions::Gif => ImageFormat::Gif,
            EncodeOptions::Jpeg { .. } => ImageFormat::Jpeg,
            EncodeOptions::Png { .. } => ImageFormat::Png,
            EncodeOptions::Tiff => ImageFormat::Tiff,
            EncodeOptions::Avif { .. } => ImageFormat::Avif,
            EncodeOptions::Heif { .. } => ImageFormat::Heif,
            EncodeOptions::WebP { .. } => ImageFormat::WebP,
        }
    }

    /// The default options for a given format.
    pub fn default_for(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Bmp => EncodeOptions::Bmp,
            ImageFormat::Gif => EncodeOptions::Gif,
            ImageFormat::Jpeg => EncodeOptions::Jpeg { quality: 75 },
            ImageFormat::Png => EncodeOptions::Png {
                compression: PngCompression::default(),
                filter: PngFilter::default(),
            },
            ImageFormat::Tiff => EncodeOptions::Tiff,
            ImageFormat::Avif => EncodeOptions::Avif {
                quality: 60,
                speed: 6,
                threads: None,
            },
            ImageFormat::Heif => EncodeOptions::Heif {
                quality: 50,
                preset: Preset::Medium,
                chroma: Chroma::Yuv420,
            },
            ImageFormat::WebP => EncodeOptions::WebP {
                quality: 75,
                quality_alpha: 100,
                compression: 4,
                lossless: false,
                threads: false,
            },
        }
    }
}

/// PNG compression effort (maps to [`image::codecs::png::CompressionType`](::image::codecs::png::CompressionType)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PngCompression {
    /// Fastest, largest output.
    Fast,
    /// Balanced default.
    #[default]
    Default,
    /// Slowest, smallest output.
    Best,
}

impl From<PngCompression> for ::image::codecs::png::CompressionType {
    fn from(value: PngCompression) -> Self {
        match value {
            PngCompression::Fast => ::image::codecs::png::CompressionType::Fast,
            PngCompression::Default => ::image::codecs::png::CompressionType::Default,
            PngCompression::Best => ::image::codecs::png::CompressionType::Best,
        }
    }
}

/// PNG scanline filter (maps to [`image::codecs::png::FilterType`](::image::codecs::png::FilterType)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PngFilter {
    /// No filtering.
    NoFilter,
    /// Sub filter.
    Sub,
    /// Up filter.
    Up,
    /// Average filter.
    Avg,
    /// Paeth filter.
    Paeth,
    /// Per-scanline adaptive filtering (the default).
    #[default]
    Adaptive,
}

impl From<PngFilter> for ::image::codecs::png::FilterType {
    fn from(value: PngFilter) -> Self {
        match value {
            PngFilter::NoFilter => ::image::codecs::png::FilterType::NoFilter,
            PngFilter::Sub => ::image::codecs::png::FilterType::Sub,
            PngFilter::Up => ::image::codecs::png::FilterType::Up,
            PngFilter::Avg => ::image::codecs::png::FilterType::Avg,
            PngFilter::Paeth => ::image::codecs::png::FilterType::Paeth,
            PngFilter::Adaptive => ::image::codecs::png::FilterType::Adaptive,
        }
    }
}
