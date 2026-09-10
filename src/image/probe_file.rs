use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use ::image::ImageReader;

use super::ImageFormat;
use super::dispatch::{info_from_decoder, probe_with_format};
use super::error::{ImageError, Result};
use super::info::ImageInfo;

/// How much of a file to read before asking `avif`/`heif`/`webp` to parse its header.
///
/// Those three codecs probe a byte slice rather than a stream, so some bound has to be chosen. A header — including
/// the boxes an ISO-BMFF container puts in front of one — sits comfortably inside this; the rare file that needs more
/// is handled by re-reading it whole rather than by guessing a larger number here.
const HEADER_PREFIX: u64 = 64 * 1024;

/// Reads the metadata of the image at `path` (dimensions, color type, bit depth) **without decoding the
/// pixels**, selecting the codec from the file extension.
///
/// Only as much of the file as the header needs is read from disk. For the native formats the decoder pulls from the
/// file directly; for `avif`/`heif`/`webp`, whose codecs parse a byte slice, a bounded prefix is read and the whole
/// file is re-read only if that prefix turns out not to have been enough.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be read, and the codec's own error if the header is
/// malformed.
///
/// ```no_run
/// use rust_sak::image::probe_file;
///
/// let info = probe_file("photo.png")?;
/// println!("{}x{} {:?} @ {}-bit", info.width, info.height, info.color_type, info.bit_depth);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn probe_file(path: impl AsRef<Path>) -> Result<ImageInfo> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;

    // A native decoder reads from any `Read + Seek`, so pointing it at the file means only the header is ever
    // fetched — a multi-hundred-megabyte TIFF costs a few kilobytes of I/O instead of being loaded whole.
    if let Some(image_format) = format.to_image_format() {
        let decoder = ImageReader::with_format(BufReader::new(File::open(path)?), image_format).into_decoder()?;
        return Ok(info_from_decoder(format, decoder));
    }

    let mut file = File::open(path)?;
    let mut prefix = Vec::new();
    file.by_ref().take(HEADER_PREFIX).read_to_end(&mut prefix)?;

    // A prefix shorter than the cap is the whole file, so there is nothing left to fall back to.
    let is_whole_file = (prefix.len() as u64) < HEADER_PREFIX;
    match probe_with_format(&prefix, format) {
        Ok(info) => Ok(info),
        Err(err) if is_whole_file => Err(err),
        // The prefix was not enough. Re-read the file whole and let its error, if any, be the one reported: it comes
        // from complete input and so describes the actual problem.
        Err(_) => {
            let mut bytes = prefix;
            file.read_to_end(&mut bytes)?;
            probe_with_format(&bytes, format)
        }
    }
}
