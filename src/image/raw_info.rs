use super::RawFormat;

/// Metadata describing a camera RAW image, read **without decoding the pixels**.
///
/// Returned by [`probe_raw_file`](super::probe_raw_file) and [`probe_raw_bytes`](super::probe_raw_bytes). This is
/// deliberately not [`ImageInfo`](super::ImageInfo), for two reasons that pull the same way: `ImageInfo::format` is
/// an [`ImageFormat`](super::ImageFormat), which has no RAW member and cannot honestly be given one, and RAW files
/// carry two facts — the camera's make and model — that a photographer's file listing wants and that no `ImageInfo`
/// field holds. Widening `ImageInfo` to fit would also cost it its `Copy`, which the eight native formats have no
/// reason to give up.
///
/// There is no `color_type`: a developed RAW is always 16-bit RGB here (see
/// [`decode_raw_bytes`](super::decode_raw_bytes)), so the field would report one constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImageInfo {
    /// The RAW format the file is in, when that can be established — `None` when it cannot.
    ///
    /// `None` is not a failure and does not mean the other fields are suspect; it means the format could not be
    /// *named*. [`probe_raw_file`](super::probe_raw_file) always fills this in, because the file name is what
    /// distinguishes the TIFF-based formats and it has one. [`probe_raw_bytes`](super::probe_raw_bytes) has only
    /// the header to go on, and a NEF, an ARW, a CR2 and a PEF all open with the same four TIFF bytes — so it fills
    /// this in only for the containers that identify themselves (DNG, CR3, RAF, RW2, ORF) and leaves it `None`
    /// otherwise rather than guessing.
    ///
    /// See [`RawFormat::from_magic`] for why the header cannot do better.
    pub format: Option<RawFormat>,
    /// Image width in pixels, after the crop and orientation the camera recorded.
    pub width: u32,
    /// Image height in pixels, after the crop and orientation the camera recorded.
    pub height: u32,
    /// Bits per channel as the sensor recorded them (e.g. `12`, `14`), or `None` when the file does not say.
    ///
    /// `Option` rather than a `u8` with a default, because the backend estimates this from the white level and
    /// cannot always derive it. A made-up number here would read exactly like a measured one.
    ///
    /// Note this describes the *sensor*, not the decoded output: [`decode_raw_bytes`](super::decode_raw_bytes)
    /// always yields 16 bits per channel regardless of what this says.
    pub bit_depth: Option<u8>,
    /// The camera manufacturer, as the file records it (e.g. `"NIKON CORPORATION"`). Empty when absent.
    pub make: String,
    /// The camera model, as the file records it (e.g. `"NIKON Z 8"`). Empty when absent.
    pub model: String,
    /// Whether the file is a DNG container rather than a vendor-native RAW.
    ///
    /// Not derivable from [`format`](RawImageInfo::format): a file named `.dng` is one, but so is an iPhone ProRAW
    /// capture, and a vendor format converted by Adobe's DNG Converter keeps its original extension in some
    /// workflows.
    pub is_dng: bool,
}
