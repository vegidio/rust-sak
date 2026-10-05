use super::ImageFormat;

/// Metadata describing an image, read from its header **without decoding the pixels**.
///
/// Returned by [`probe_file`](super::probe_file) and [`probe_bytes`](super::probe_bytes). The
/// [`color_type`](ImageInfo::color_type) reuses the [`image`](::image) crate's
/// [`ColorType`](::image::ColorType) enum, which already encodes the channel layout (gray/RGB, presence of
/// alpha) and the per-channel sample size for the native formats. [`bit_depth`](ImageInfo::bit_depth) reports
/// the true bits per channel, which matters for the high-bit-depth `avif`/`heif` formats whose 10- or 12-bit
/// samples [`ColorType`](::image::ColorType) cannot distinguish from 16-bit.
///
/// Not `Copy`: [`color_profile`](ImageInfo::color_profile) is text read from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The name of the image's color profile, when the file declares one — `None` when it does not, or when what it
    /// declares has no name.
    ///
    /// For an embedded ICC profile this is the profile's own description, as its author wrote it (`Display P3`,
    /// `sRGB IEC61966-2.1`), trimmed and capped at 64 characters. AVIF and HEIF files often carry a coded color
    /// description (CICP, in an `nclx` box) instead, which is named from a fixed table: `sRGB`, `Display P3`,
    /// `Rec. 2020`, `Rec. 2100 PQ` or `Rec. 2100 HLG`. A combination outside that table gives `None` rather than a
    /// guess.
    pub color_profile: Option<String>,
}
