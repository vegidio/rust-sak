use super::ImageFormat;

/// Metadata describing an image, read from its header **without decoding the pixels**.
///
/// Returned by [`probe_file`](super::probe_file) and [`probe_bytes`](super::probe_bytes). The
/// [`color_type`](ImageInfo::color_type) reuses the [`image`](::image) crate's
/// [`ColorType`](::image::ColorType) enum, which already encodes the channel layout (gray/RGB, presence of
/// alpha) and the per-channel sample size for the native formats. [`bit_depth`](ImageInfo::bit_depth) reports
/// the true bits per channel, which matters for the high-bit-depth `avif`/`heif` formats whose 10- or 12-bit
/// samples [`ColorType`](::image::ColorType) cannot distinguish from 16-bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    /// The detected image format.
    pub format: ImageFormat,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// The pixel color type (channel layout and sample size), as the [`image`](::image) crate reports it.
    pub color_type: ::image::ColorType,
    /// Bits per channel (e.g. `8`, `10`, `12`, `16`).
    pub bit_depth: u8,
}
