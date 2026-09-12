use std::io::{Cursor, Write};
use std::sync::{Mutex, PoisonError};

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
            Ok(info_from_decoder(format, decoder))
        }
    }
}

/// Reads the metadata a header-parsed decoder exposes, without touching the pixels.
///
/// Shared by [`probe_with_format`], which decodes from a byte slice, and
/// [`probe_file`](super::probe_file), which decodes straight from the file so only the header is read off disk.
pub(super) fn info_from_decoder(format: ImageFormat, decoder: impl ImageDecoder) -> ImageInfo {
    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let bit_depth = (color_type.bits_per_pixel() / color_type.channel_count() as u16) as u8;

    ImageInfo {
        format,
        width,
        height,
        color_type,
        bit_depth,
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

/// Returns `image` converted to a colour type the `format` encoder accepts, or `None` when it already accepts the
/// image as it stands (so the common case allocates nothing).
///
/// Every codec accepts a fixed set of colour types and answers `Unsupported` for the rest, which would otherwise make
/// an ordinary picture — a developed camera RAW is 16-bit, a grayscale PNG has no colour — unwritable in some formats.
/// The conversion loses as little as the target forces: depth is narrowed only where the encoder cannot hold it, alpha
/// is kept wherever the format has an alpha channel, and grayscale is expanded to colour only where the format has no
/// grayscale representation. So a 32-float picture written as PNG becomes 16-bit rather than 8-bit, and a
/// grayscale-with-alpha picture written as TIFF — which has no grayscale-with-alpha at any depth — becomes RGBA at its
/// original depth rather than being narrowed twice.
///
/// **BMP and JPEG are deliberately absent.** Both implement the `image` crate's own
/// [`ImageEncoder::make_compatible_img`](::image::ImageEncoder::make_compatible_img) hook, which runs inside
/// `write_with_encoder` *after* this; converting here as well would convert twice, and for JPEG to a worse result —
/// its hook sends `La8` to luma, keeping grayscale, where a generic narrowing would send it to luma-alpha first.
pub(super) fn make_compatible(image: &DynamicImage, format: ImageFormat) -> Option<DynamicImage> {
    use DynamicImage::{
        ImageLuma8, ImageLumaA8, ImageRgb8, ImageRgb16, ImageRgb32F, ImageRgba8, ImageRgba16, ImageRgba32F,
    };

    match format {
        // Their own `make_compatible_img` already covers everything — see the note above.
        ImageFormat::Bmp | ImageFormat::Jpeg => None,
        // 8- and 16-bit, with or without alpha, grayscale or colour; only the 32-float types need narrowing, and 16 is
        // as far as they have to come down.
        ImageFormat::Png | ImageFormat::Avif | ImageFormat::Heif => match image {
            ImageRgb32F(_) => Some(ImageRgb16(image.to_rgb16())),
            ImageRgba32F(_) => Some(ImageRgba16(image.to_rgba16())),
            _ => None,
        },
        // Every depth including 32-float, but no grayscale-with-alpha at any depth: alpha is what forces the move to
        // RGBA, so the depth comes along rather than being given up with it.
        ImageFormat::Tiff => match image {
            ImageLumaA8(_) => Some(ImageRgba8(image.to_rgba8())),
            DynamicImage::ImageLumaA16(_) => Some(ImageRgba16(image.to_rgba16())),
            _ => None,
        },
        // `Rgb8`/`Rgba8` only — the narrowest of the eight. Everything else is 8-bit colour, keeping alpha where the
        // source had it.
        ImageFormat::Gif => match image {
            ImageRgb8(_) | ImageRgba8(_) => None,
            other if other.color().has_alpha() => Some(ImageRgba8(other.to_rgba8())),
            other => Some(ImageRgb8(other.to_rgb8())),
        },
        // libwebp is 8-bit, which is a property of the codec rather than of this dispatch; the colour layout survives
        // untouched, so grayscale stays grayscale.
        ImageFormat::WebP => match image {
            ImageLuma8(_) | ImageLumaA8(_) | ImageRgb8(_) | ImageRgba8(_) => None,
            DynamicImage::ImageLuma16(_) => Some(ImageLuma8(image.to_luma8())),
            DynamicImage::ImageLumaA16(_) => Some(ImageLumaA8(image.to_luma_alpha8())),
            ImageRgb16(_) | ImageRgb32F(_) => Some(ImageRgb8(image.to_rgb8())),
            ImageRgba16(_) | ImageRgba32F(_) => Some(ImageRgba8(image.to_rgba8())),
            // `DynamicImage` is `#[non_exhaustive]`: a variant added later keeps its alpha and comes down to 8 bits.
            other if other.color().has_alpha() => Some(ImageRgba8(other.to_rgba8())),
            other => Some(ImageRgb8(other.to_rgb8())),
        },
    }
}

/// Serializes AVIF encodes against each other.
///
/// The bundled SVT-AV1 encoder keeps per-encode global state, so two concurrent encodes with *different* options can
/// read each other's settings. Holding this for the duration of the AVIF arm is what makes [`encode_into`] safe to
/// call from several threads at once. An AVIF encode runs for tens to hundreds of milliseconds, so the lock costs
/// nothing measurable next to the work it guards.
///
/// A poisoned lock is recovered rather than propagated: the codec state is reset by the next encode either way, and a
/// panic in one caller must not make the encoder permanently unusable — the same reasoning as the `PoisonError`
/// handling in [`memo`](crate::memo) and [`o11y`](crate::o11y).
static AVIF_ENCODE: Mutex<()> = Mutex::new(());

/// Encodes `image` straight into `writer` using the codec selected by `options`.
///
/// Every codec here except BMP, GIF and TIFF takes a plain [`Write`], so the encoded bytes go to the destination as
/// they are produced rather than being accumulated first. The three that go through
/// [`DynamicImage::write_to`](::image::DynamicImage::write_to) need `Write + Seek`, so they are the only ones that
/// still buffer — see [`encode_to_vec`], which is what callers with a non-seekable sink use for them.
pub(super) fn encode_into<W: Write>(image: &DynamicImage, options: EncodeOptions, writer: &mut W) -> Result<()> {
    // Ahead of the `match`, so no codec below is reachable without it. See [`make_compatible`] for what it converts.
    let converted = make_compatible(image, options.format());
    let image = converted.as_ref().unwrap_or(image);

    match options {
        // These three need `Seek` as well, which a bare `Write` cannot offer.
        EncodeOptions::Bmp | EncodeOptions::Gif | EncodeOptions::Tiff => {
            writer.write_all(&encode_to_vec(image, options)?)?;
        }
        EncodeOptions::Jpeg { quality } => {
            image.write_with_encoder(JpegEncoder::new_with_quality(writer, quality))?;
        }
        EncodeOptions::Png { compression, filter } => {
            image.write_with_encoder(PngEncoder::new_with_quality(writer, compression.into(), filter.into()))?;
        }
        EncodeOptions::Avif {
            quality,
            speed,
            threads,
        } => {
            let mut encoder = ::avif::AvifEncoder::new(writer).with_quality(quality).with_speed(speed);
            if let Some(threads) = threads {
                encoder = encoder.with_threads(threads);
            }
            // Held across the encode itself, not just the encoder's construction: the global state the codec keeps is
            // read as it runs.
            let _guard = AVIF_ENCODE.lock().unwrap_or_else(PoisonError::into_inner);
            image.write_with_encoder(encoder)?;
        }
        EncodeOptions::Heif {
            quality,
            preset,
            chroma,
        } => {
            let encoder = ::heif::HeifEncoder::new(writer)
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
            let encoder = ::webp::WebpEncoder::new(writer)
                .with_quality(quality)
                .with_quality_alpha(quality_alpha)
                .with_compression(compression)
                .with_lossless(lossless)
                .with_threads(threads);
            image.write_with_encoder(encoder)?;
        }
    }

    Ok(())
}

/// Encodes `image` into a fresh byte buffer using the codec selected by `options`.
///
/// Only BMP, GIF and TIFF genuinely need this: their encoder wants `Write + Seek`, so the bytes have to land somewhere
/// rewindable. Everything else reaches here only from a caller whose sink is not seekable, and streams through
/// [`encode_into`] otherwise.
pub(super) fn encode_to_vec(image: &DynamicImage, options: EncodeOptions) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();

    match options {
        // The three seek-bound codecs are reached from here rather than through [`encode_into`], so the compatibility
        // step runs on this path too — otherwise GIF, which needs it most, would be the one format that never got it.
        EncodeOptions::Bmp | EncodeOptions::Gif | EncodeOptions::Tiff => {
            let format = options
                .format()
                .to_image_format()
                .expect("bmp, gif and tiff map to image::ImageFormat");
            let converted = make_compatible(image, options.format());
            let image = converted.as_ref().unwrap_or(image);
            image.write_to(&mut Cursor::new(&mut buffer), format)?;
        }
        // Not seek-bound, so these stream; the buffer is only here because this caller's sink cannot. `encode_into`
        // runs the compatibility step itself.
        other => encode_into(image, other, &mut buffer)?,
    }

    Ok(buffer)
}
