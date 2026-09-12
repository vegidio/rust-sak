use std::path::Path;

/// A camera RAW format that can be **decoded but never encoded**.
///
/// RAW is deliberately kept out of [`ImageFormat`](super::ImageFormat), which is the encode target as well as the
/// decode source: every member there has to answer "what do I write, and with which
/// [`EncodeOptions`](super::EncodeOptions)", and no RAW format has an honest answer — a camera writes RAW, software
/// reads it. Keeping the two enums apart says that in the type system rather than in a comment.
///
/// The variants are the families the backend decodes, one per container rather than one per vendor: Canon's CRW, CR2
/// and CR3 are three different formats that happen to share a manufacturer. Where a single format is written under
/// more than one extension the extensions collapse onto one variant, the way `jpg`/`jpeg` do for
/// [`ImageFormat::Jpeg`](super::ImageFormat::Jpeg).
///
/// A format missing here is a camera this build cannot open. That list comes from `rawler` and grows when it does;
/// see the module docs for what the feature costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RawFormat {
    /// ARRI (`.ari`).
    Ari,
    /// Sony (`.arw`).
    Arw,
    /// Canon, pre-2004 (`.crw`).
    Crw,
    /// Canon (`.cr2`).
    Cr2,
    /// Canon, current (`.cr3`; also `.crm` for Cinema RAW Light).
    Cr3,
    /// Kodak (`.dcr`).
    Dcr,
    /// Kodak Professional DCS (`.dcs`).
    Dcs,
    /// Adobe Digital Negative (`.dng`) — the vendor-neutral container, including Apple ProRAW.
    Dng,
    /// Epson (`.erf`).
    Erf,
    /// Hasselblad Phocus (`.fff`).
    Fff,
    /// Phase One (`.iiq`).
    Iiq,
    /// Kodak (`.kdc`).
    Kdc,
    /// Mamiya (`.mef`).
    Mef,
    /// Leaf (`.mos`).
    Mos,
    /// Minolta (`.mrw`).
    Mrw,
    /// Nikon (`.nef`).
    Nef,
    /// Nikon, compact models (`.nrw`).
    Nrw,
    /// Olympus (`.orf`; also `.ori`).
    Orf,
    /// Pentax (`.pef`).
    Pef,
    /// Apple QuickTake (`.qtk`).
    Qtk,
    /// Fujifilm (`.raf`).
    Raf,
    /// Panasonic and Leica (`.rw2`; also `.rwl` and the bare `.raw` older Panasonic bodies wrote).
    Rw2,
    /// Samsung (`.srw`).
    Srw,
    /// Hasselblad, straight from the camera (`.3fr`).
    ThreeFr,
    /// Sigma Foveon (`.x3f`).
    X3f,
}

impl RawFormat {
    /// Returns the RAW format associated with the given file-name extension (case-insensitive, no leading dot), or
    /// `None` if the extension does not map to a RAW format this build can decode.
    ///
    /// `tif`/`tiff` deliberately map to nothing here even though almost every RAW format *is* a TIFF container: a
    /// file named `.tiff` is an ordinary TIFF for [`ImageFormat`](super::ImageFormat) to decode, and claiming it for
    /// RAW would break the eight formats that already work.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let ext = extension.to_ascii_lowercase();
        let format = match ext.as_str() {
            "ari" => RawFormat::Ari,
            "arw" => RawFormat::Arw,
            "crw" => RawFormat::Crw,
            "cr2" => RawFormat::Cr2,
            "cr3" | "crm" => RawFormat::Cr3,
            "dcr" => RawFormat::Dcr,
            "dcs" => RawFormat::Dcs,
            "dng" => RawFormat::Dng,
            "erf" => RawFormat::Erf,
            "fff" => RawFormat::Fff,
            "iiq" => RawFormat::Iiq,
            "kdc" => RawFormat::Kdc,
            "mef" => RawFormat::Mef,
            "mos" => RawFormat::Mos,
            "mrw" => RawFormat::Mrw,
            "nef" => RawFormat::Nef,
            "nrw" => RawFormat::Nrw,
            "orf" | "ori" => RawFormat::Orf,
            "pef" => RawFormat::Pef,
            "qtk" => RawFormat::Qtk,
            "raf" => RawFormat::Raf,
            "rw2" | "rwl" | "raw" => RawFormat::Rw2,
            "srw" => RawFormat::Srw,
            "3fr" => RawFormat::ThreeFr,
            "x3f" => RawFormat::X3f,
            _ => return None,
        };
        Some(format)
    }

    /// Returns the RAW format inferred from a path's extension, or `None` if it has no recognized RAW extension.
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        path.as_ref()
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// The canonical lowercase file extension for this format (without a leading dot).
    pub fn extension(self) -> &'static str {
        match self {
            RawFormat::Ari => "ari",
            RawFormat::Arw => "arw",
            RawFormat::Crw => "crw",
            RawFormat::Cr2 => "cr2",
            RawFormat::Cr3 => "cr3",
            RawFormat::Dcr => "dcr",
            RawFormat::Dcs => "dcs",
            RawFormat::Dng => "dng",
            RawFormat::Erf => "erf",
            RawFormat::Fff => "fff",
            RawFormat::Iiq => "iiq",
            RawFormat::Kdc => "kdc",
            RawFormat::Mef => "mef",
            RawFormat::Mos => "mos",
            RawFormat::Mrw => "mrw",
            RawFormat::Nef => "nef",
            RawFormat::Nrw => "nrw",
            RawFormat::Orf => "orf",
            RawFormat::Pef => "pef",
            RawFormat::Qtk => "qtk",
            RawFormat::Raf => "raf",
            RawFormat::Rw2 => "rw2",
            RawFormat::Srw => "srw",
            RawFormat::ThreeFr => "3fr",
            RawFormat::X3f => "x3f",
        }
    }

    /// Guesses the format from the leading bytes of a RAW file, or `None` if the bytes name no format it can pin
    /// down.
    ///
    /// **This resolves far less than [`ImageFormat::from_magic`](super::ImageFormat::from_magic) does, and that is
    /// the format's fault rather than a gap here.** Most RAW formats are plain TIFF containers — a NEF, an ARW, a
    /// CR2 and a PEF all begin with the same four bytes — so the header cannot tell them apart, and `None` is
    /// returned rather than a guess. Only the containers with a signature of their own come back: DNG (by its
    /// `DNGVersion` tag), CR3, RAF, RW2 and ORF.
    ///
    /// Use [`from_path`](RawFormat::from_path) when the name is available; it is the only thing that distinguishes
    /// the TIFF-based formats. Use [`is_raw_bytes`] to ask the weaker question of whether bytes are RAW at all.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        // `AppleDng` is a DNG with an Apple container signature; it decodes through the same path, so the extra
        // distinction is not carried into `RawFormat`.
        let format = match ::zenraw::classify(bytes) {
            ::zenraw::FileFormat::Dng | ::zenraw::FileFormat::AppleDng => RawFormat::Dng,
            ::zenraw::FileFormat::Cr3 => RawFormat::Cr3,
            ::zenraw::FileFormat::Raf => RawFormat::Raf,
            ::zenraw::FileFormat::Rw2 => RawFormat::Rw2,
            ::zenraw::FileFormat::Orf => RawFormat::Orf,
            // `TiffRaw` is every other TIFF-based RAW at once and so names none of them; `AppleAmpf` is a processed
            // JPEG despite its `.DNG` extension and holds no sensor data; `Jpeg` and `Unknown` are not RAW.
            _ => return None,
        };
        Some(format)
    }
}

/// Whether `bytes` look like a camera RAW file.
///
/// <div class="warning">
///
/// **A plain TIFF answers `true`.** This is a header-magic check, and the RAW formats are TIFF containers, so
/// "starts like a TIFF" is as far as the bytes go — an ordinary `.tiff` photograph is indistinguishable from a NEF
/// at this depth without parsing the IFD chain. Do not use this to choose between the RAW decoder and the TIFF
/// decoder; check the file extension for that, which is what actually tells them apart. What this is good for is
/// rejecting bytes that are obviously not RAW before paying for a decode.
///
/// </div>
pub fn is_raw_bytes(bytes: &[u8]) -> bool {
    ::zenraw::is_raw_file(bytes)
}
