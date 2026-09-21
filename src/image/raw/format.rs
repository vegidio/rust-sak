use std::path::Path;

/// Declares [`RawFormat`] and both halves of its extension mapping from one list.
///
/// Each line is a variant, its canonical extension, and any aliases that name the same format. Writing the enum,
/// `from_extension` and `extension` out separately meant three lists to keep in agreement, and while a new variant
/// is a compile error in `extension` — the match is exhaustive — a *missing alias* in `from_extension` is silent.
/// Here an alias sits on the same line as the documentation that mentions it.
macro_rules! raw_formats {
    ($($(#[$doc:meta])* $variant:ident => $canonical:literal $(, $alias:literal)* ;)*) => {
        /// A camera RAW format that can be **decoded but never encoded**.
        ///
        /// RAW is deliberately kept out of [`ImageFormat`](super::super::ImageFormat), which is the encode target as well as the
        /// decode source: every member there has to answer "what do I write, and with which
        /// [`EncodeOptions`](super::super::EncodeOptions)", and no RAW format has an honest answer — a camera writes RAW, software
        /// reads it. Keeping the two enums apart says that in the type system rather than in a comment.
        ///
        /// The variants are the families the backend decodes, one per container rather than one per vendor: Canon's CRW, CR2
        /// and CR3 are three different formats that happen to share a manufacturer. Where a single format is written under
        /// more than one extension the extensions collapse onto one variant, the way `jpg`/`jpeg` do for
        /// [`ImageFormat::Jpeg`](super::super::ImageFormat::Jpeg).
        ///
        /// A format missing here is a camera this build cannot open. That list comes from `rawler` and grows when it does;
        /// see the module docs for what the feature costs.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum RawFormat {
            $($(#[$doc])* $variant,)*
        }

        impl RawFormat {
            /// Returns the RAW format associated with the given file-name extension (case-insensitive, no leading
            /// dot), or `None` if the extension does not map to a RAW format this build can decode.
            ///
            /// `tif`/`tiff` deliberately map to nothing here even though almost every RAW format *is* a TIFF
            /// container: a file named `.tiff` is an ordinary TIFF for [`ImageFormat`](super::super::ImageFormat) to
            /// decode, and claiming it for RAW would break the eight formats that already work.
            pub fn from_extension(extension: &str) -> Option<Self> {
                match extension.to_ascii_lowercase().as_str() {
                    $($canonical $(| $alias)* => Some(RawFormat::$variant),)*
                    _ => None,
                }
            }

            /// The canonical lowercase file extension for this format (without a leading dot).
            pub fn extension(self) -> &'static str {
                match self {
                    $(RawFormat::$variant => $canonical,)*
                }
            }
        }
    };
}

raw_formats! {
    /// ARRI (`.ari`).
    Ari => "ari";
    /// Sony (`.arw`).
    Arw => "arw";
    /// Canon, pre-2004 (`.crw`).
    Crw => "crw";
    /// Canon (`.cr2`).
    Cr2 => "cr2";
    /// Canon, current (`.cr3`; also `.crm` for Cinema RAW Light).
    Cr3 => "cr3", "crm";
    /// Kodak (`.dcr`).
    Dcr => "dcr";
    /// Kodak Professional DCS (`.dcs`).
    Dcs => "dcs";
    /// Adobe Digital Negative (`.dng`) — the vendor-neutral container, including Apple ProRAW.
    Dng => "dng";
    /// Epson (`.erf`).
    Erf => "erf";
    /// Hasselblad Phocus (`.fff`).
    Fff => "fff";
    /// Phase One (`.iiq`).
    Iiq => "iiq";
    /// Kodak (`.kdc`).
    Kdc => "kdc";
    /// Mamiya (`.mef`).
    Mef => "mef";
    /// Leaf (`.mos`).
    Mos => "mos";
    /// Minolta (`.mrw`).
    Mrw => "mrw";
    /// Nikon (`.nef`).
    Nef => "nef";
    /// Nikon, compact models (`.nrw`).
    Nrw => "nrw";
    /// Olympus (`.orf`; also `.ori`).
    Orf => "orf", "ori";
    /// Pentax (`.pef`).
    Pef => "pef";
    /// Apple QuickTake (`.qtk`).
    Qtk => "qtk";
    /// Fujifilm (`.raf`).
    Raf => "raf";
    /// Panasonic and Leica (`.rw2`; also `.rwl` and the bare `.raw` older Panasonic bodies wrote).
    Rw2 => "rw2", "rwl", "raw";
    /// Samsung (`.srw`).
    Srw => "srw";
    /// Hasselblad, straight from the camera (`.3fr`).
    ThreeFr => "3fr";
    /// Sigma Foveon (`.x3f`).
    X3f => "x3f";
}

impl RawFormat {
    /// Returns the RAW format inferred from a path's extension, or `None` if it has no recognized RAW extension.
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        path.as_ref()
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// Guesses the format from the leading bytes of a RAW file, or `None` if the bytes name no format it can pin
    /// down.
    ///
    /// **This resolves far less than [`ImageFormat::from_magic`](super::super::ImageFormat::from_magic) does, and that is
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
