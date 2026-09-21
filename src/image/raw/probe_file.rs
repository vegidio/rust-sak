use std::fs;
use std::path::Path;

use super::super::error::{ImageError, Result};
use super::RawFormat;
use super::dispatch::probe_raw;
use super::info::RawImageInfo;

/// Reads the metadata of the camera RAW file at `path` (dimensions, sensor bit depth, camera make and model)
/// **without decoding the pixels**.
///
/// [`RawImageInfo::format`] is always filled in here, from the extension — which is the only thing that
/// distinguishes the TIFF-based RAW formats from one another.
///
/// <div class="warning">
///
/// **This reads the whole file**, where [`probe_file`](super::super::probe_file) reads only a header. That asymmetry is
/// the backend's rather than a choice made here: it probes a byte slice, and RAW metadata lives in IFD chains whose
/// offsets routinely point deep into a 40 MB file. A bounded prefix — the trick [`probe_file`](super::super::probe_file)
/// uses for AVIF, HEIF and WebP — would miss more often than it hit, making the whole-file re-read the normal path
/// and the two reads more expensive than the one. Worth knowing if you are listing a directory of RAWs.
///
/// </div>
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized RAW extension,
/// [`ImageError::Io`](super::super::ImageError::Io) if the file cannot be read, [`ImageError::NotRaw`] if its contents are
/// not RAW, and [`ImageError::Raw`] if its metadata cannot be read.
///
/// ```no_run
/// use rust_sak::image::probe_raw_file;
///
/// let info = probe_raw_file("DSC_0001.NEF")?;
/// println!("{}x{} from a {} {}", info.width, info.height, info.make, info.model);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn probe_raw_file(path: impl AsRef<Path>) -> Result<RawImageInfo> {
    let path = path.as_ref();
    let format = RawFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let bytes = fs::read(path)?;
    probe_raw(&bytes, Some(format))
}
