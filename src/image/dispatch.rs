use std::io::Cursor;

use ::image::codecs::jpeg::JpegEncoder;
use ::image::codecs::png::PngEncoder;
use ::image::{DynamicImage, ImageDecoder, ImageReader};

use super::error::{ImageError, Result};
use super::info::ImageInfo;
use super::{EncodeOptions, ImageFormat};

/// Decodes `bytes` known to be in `format`, routing native formats through the `image` crate and `avif`/`heif`/`webp`
/// through their dedicated codecs. Shared by every decode entry point.
pub(super) fn decode_with_format(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage> {
    match format {
        ImageFormat::Avif => Ok(::avif::decode(bytes)?),
        ImageFormat::Heif => Ok(::heif::decode(bytes)?),
        ImageFormat::WebP => Ok(::webp::decode(bytes)?),
        native => {
            let image_format = native
                .to_image_format()
                .expect("native formats map to image::ImageFormat");
            Ok(::image::load_from_memory_with_format(bytes, image_format)?)
        }
    }
}

/// Reads `bytes` known to be in `format` and returns its header metadata **without decoding the pixels**,
/// routing native formats through the `image` crate's streaming decoder and `avif`/`heif`/`webp` through their
/// dedicated crates' `probe` helpers. Shared by every probe entry point.
pub(super) fn probe_with_format(bytes: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    match format {
        ImageFormat::Avif => {
            let info = ::avif::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: avif_bit_depth(info.bit_depth),
            })
        }
        ImageFormat::Heif => {
            let info = ::heif::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: heif_bit_depth(info.bit_depth),
            })
        }
        ImageFormat::WebP => {
            // WebP is always 8 bits per channel.
            let info = ::webp::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: 8,
            })
        }
        native => {
            let image_format = native
                .to_image_format()
                .expect("native formats map to image::ImageFormat");
            // `into_decoder` parses the header only; `read_image` (never called here) is what decodes pixels.
            let decoder = ImageReader::with_format(Cursor::new(bytes), image_format).into_decoder()?;
            let (width, height) = decoder.dimensions();
            let color_type = decoder.color_type();
            let bit_depth = (color_type.bits_per_pixel() / color_type.channel_count() as u16) as u8;
            Ok(ImageInfo {
                format,
                width,
                height,
                color_type,
                bit_depth,
            })
        }
    }
}

/// Maps the `avif` crate's bit-depth enum to bits per channel. Both enums are `#[non_exhaustive]`, so the
/// catch-all keeps this compiling against future codec variants (defaulting to 8-bit).
fn avif_bit_depth(depth: ::avif::BitDepth) -> u8 {
    match depth {
        ::avif::BitDepth::Eight => 8,
        ::avif::BitDepth::Ten => 10,
        ::avif::BitDepth::Twelve => 12,
        _ => 8,
    }
}

/// Maps the `heif` crate's bit-depth enum to bits per channel. See [`avif_bit_depth`] for the catch-all rationale.
fn heif_bit_depth(depth: ::heif::BitDepth) -> u8 {
    match depth {
        ::heif::BitDepth::Eight => 8,
        ::heif::BitDepth::Ten => 10,
        ::heif::BitDepth::Twelve => 12,
        _ => 8,
    }
}

/// Resolves the caller-supplied options against the encode `format`: `None` yields the format's defaults, while
/// `Some(options)` for a different format is rejected as an [`ImageError::FormatMismatch`]. Shared by both encoders.
pub(super) fn resolve_options(format: ImageFormat, options: Option<EncodeOptions>) -> Result<EncodeOptions> {
    let options = options.unwrap_or_else(|| EncodeOptions::default_for(format));
    let actual = options.format();
    if actual == format {
        Ok(options)
    } else {
        Err(ImageError::FormatMismatch {
            expected: format,
            options: actual,
        })
    }
}

/// Encodes `image` into a fresh byte buffer using the codec selected by `options`. Shared by `encode_file` and
/// `encode_writer`; both then move the bytes to their destination.
pub(super) fn encode_to_vec(image: &DynamicImage, options: EncodeOptions) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();

    match options {
        EncodeOptions::Bmp => image.write_to(&mut Cursor::new(&mut buffer), ::image::ImageFormat::Bmp)?,
        EncodeOptions::Gif => image.write_to(&mut Cursor::new(&mut buffer), ::image::ImageFormat::Gif)?,
        EncodeOptions::Tiff => image.write_to(&mut Cursor::new(&mut buffer), ::image::ImageFormat::Tiff)?,
        EncodeOptions::Jpeg { quality } => {
            image.write_with_encoder(JpegEncoder::new_with_quality(&mut buffer, quality))?;
        }
        EncodeOptions::Png { compression, filter } => {
            image.write_with_encoder(PngEncoder::new_with_quality(
                &mut buffer,
                compression.into(),
                filter.into(),
            ))?;
        }
        EncodeOptions::Avif {
            quality,
            speed,
            threads,
        } => {
            let mut encoder = ::avif::AvifEncoder::new(&mut buffer)
                .with_quality(quality)
                .with_speed(speed);
            if let Some(threads) = threads {
                encoder = encoder.with_threads(threads);
            }
            image.write_with_encoder(encoder)?;
        }
        EncodeOptions::Heif {
            quality,
            preset,
            chroma,
        } => {
            let encoder = ::heif::HeifEncoder::new(&mut buffer)
                .with_quality(quality)
                .with_preset(preset)
                .with_chroma(chroma);
            image.write_with_encoder(encoder)?;
        }
        EncodeOptions::WebP {
            quality,
            quality_alpha,
            compression,
            lossless,
            threads,
        } => {
            let encoder = ::webp::WebpEncoder::new(&mut buffer)
                .with_quality(quality)
                .with_quality_alpha(quality_alpha)
                .with_compression(compression)
                .with_lossless(lossless)
                .with_threads(threads);
            image.write_with_encoder(encoder)?;
        }
    }

    Ok(buffer)
}
