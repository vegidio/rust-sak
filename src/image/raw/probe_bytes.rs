use super::super::error::Result;
use super::dispatch::probe_raw;
use super::info::RawImageInfo;

/// Reads the metadata of the camera RAW file in `bytes` (dimensions, sensor bit depth, camera make and model)
/// **without decoding the pixels**.
///
/// [`RawImageInfo::format`] comes back `None` for the TIFF-based formats — NEF, ARW, CR2, PEF and most of the rest
/// share a header and cannot be told apart by content. Use [`probe_raw_file`](super::probe_raw_file) when the file
/// name is available; it is the only thing that distinguishes them, and it always names the format.
///
/// # Errors
///
/// Returns [`ImageError::NotRaw`] if the bytes are not a RAW file, and [`ImageError::Raw`] if they are but their
/// metadata cannot be read.
///
/// ```no_run
/// use rust_sak::image::probe_raw_bytes;
///
/// let bytes = std::fs::read("DSC_0001.NEF")?;
/// let info = probe_raw_bytes(&bytes)?;
/// println!("{}x{} from a {} {}", info.width, info.height, info.make, info.model);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn probe_raw_bytes(bytes: &[u8]) -> Result<RawImageInfo> {
    // No file name, so `None` leaves `probe_raw` to resolve what it can from the magic bytes alone.
    probe_raw(bytes, None)
}
