use ::image::{DynamicImage, ImageBuffer};
use ::zenpixels::{PixelBuffer, PixelDescriptor};

use super::super::error::{ImageError, Result};
use super::{RawFormat, RawImageInfo};

/// Converts the decoder's [`PixelBuffer`] into a [`DynamicImage::ImageRgb16`]. Shared by every RAW decode entry
/// point.
///
/// Sixteen bits per channel are kept rather than narrowed to eight: a RAW file exists because the sensor recorded
/// more than eight bits, and [`DynamicImage`] already carries `ImageRgb16`, so nothing downstream has to widen to
/// accept it.
///
/// **Stride is the thing to get right here.** A [`PixelBuffer`]'s `stride` is independent of its width and the
/// decoder may SIMD-align it, so its backing bytes are not necessarily a packed image — reading them as one would
/// skew every row after the first by a few pixels and produce a sheared picture that still looks plausible enough
/// to ship. The contiguous case is taken when the buffer says it holds, and the general case copies row by row
/// through `row(y)`, which returns exactly `width * bpp` bytes with the padding already excluded.
pub(in crate::image) fn image_from_pixel_buffer(buffer: &PixelBuffer) -> Result<DynamicImage> {
    let pixels = buffer.as_slice();
    let descriptor = pixels.descriptor();

    // `Develop` output is `RGB16_SRGB`, but with the target primaries stamped on it, so this compares the memory
    // layout rather than the whole descriptor — the primaries change what the samples *mean*, not how they are laid
    // out, and reinterpreting them is exactly what this function does.
    if !descriptor.layout_compatible(PixelDescriptor::RGB16_SRGB) {
        return Err(ImageError::Raw(::zenraw::RawError::InvalidInput(format!(
            "expected a 16-bit RGB pixel buffer, got {}",
            descriptor.format.name()
        ))));
    }

    let width = buffer.width();
    let height = buffer.height();
    let samples_per_row = width as usize * descriptor.channels();
    let mut samples: Vec<u16> = Vec::with_capacity(samples_per_row * height as usize);

    // The decoder writes `u16` samples with `to_ne_bytes`, so they are read back the same way. Going through the
    // bytes rather than casting the slice also sidesteps the alignment question: a `&[u8]` from an aligned
    // allocation is not necessarily a validly aligned `&[u16]`.
    let mut push_row = |row: &[u8]| samples.extend(row.as_chunks::<2>().0.iter().map(|s| u16::from_ne_bytes(*s)));

    if pixels.is_contiguous() {
        // No padding between rows, so the whole image is one run of bytes and the per-row bookkeeping is skipped.
        let bytes = pixels
            .as_contiguous_bytes()
            .expect("a contiguous buffer yields its bytes");
        push_row(bytes);
    } else {
        for y in 0..height {
            push_row(pixels.row(y));
        }
    }

    let image = ImageBuffer::from_raw(width, height, samples).ok_or_else(|| {
        ImageError::Raw(::zenraw::RawError::InvalidInput(format!(
            "decoded pixels do not fill a {width}x{height} image"
        )))
    })?;

    Ok(DynamicImage::ImageRgb16(image))
}

/// The decoder's settings. [`RawDecodeConfig::default`] is used as-is: `Develop` output in sRGB primaries, the
/// camera's own white balance, no exposure compensation, and the crop and orientation the camera recorded — the
/// picture the camera describes, with none of this module's opinions added. Its `max_pixels` default of 200
/// megapixels sits above any camera and below a decompression bomb.
fn decode_config() -> ::zenraw::RawDecodeConfig {
    ::zenraw::RawDecodeConfig::default()
}

/// Decodes `bytes` already known to be RAW. Shared by both decode entry points.
///
/// Cancellation is [`Unstoppable`](::enough::Unstoppable): this module is synchronous throughout, and threading a
/// cancellation token through one decoder out of nine would be an inconsistency no caller asked for.
pub(in crate::image) fn decode_raw(bytes: &[u8]) -> Result<DynamicImage> {
    ensure_raw(bytes)?;

    let output =
        ::zenraw::decode(bytes, &decode_config(), &::enough::Unstoppable).map_err(|error| error.decompose().0)?;
    image_from_pixel_buffer(&output.pixels)
}

/// Rejects bytes that do not look like a RAW file.
///
/// Placed here, in the body every entry point already funnels through, rather than at each of them: what counts as
/// RAW is one decision, and repeating it per entry point meant a fifth one could quietly ship without it. It goes
/// through the module's own [`is_raw_bytes`](super::is_raw_bytes) rather than the backend directly, so the
/// predicate callers are handed and the check performed here cannot disagree.
fn ensure_raw(bytes: &[u8]) -> Result<()> {
    if super::is_raw_bytes(bytes) {
        Ok(())
    } else {
        Err(ImageError::NotRaw)
    }
}

/// Probes `bytes` already known to be RAW, with `format` resolved by whatever the caller had to go on. Shared by
/// both probe entry points.
pub(super) fn probe_raw(bytes: &[u8], format: Option<RawFormat>) -> Result<RawImageInfo> {
    ensure_raw(bytes)?;

    let info = ::zenraw::probe(bytes, &::enough::Unstoppable).map_err(|error| error.decompose().0)?;

    Ok(RawImageInfo {
        // A caller who knew the file name passed the format it names; otherwise this is whatever the header could
        // be pinned down to, which for the TIFF-based formats is nothing.
        format: format.or_else(|| RawFormat::from_magic(bytes)),
        width: info.width,
        height: info.height,
        bit_depth: info.bit_depth,
        make: info.make,
        model: info.model,
        is_dng: info.is_dng,
    })
}
