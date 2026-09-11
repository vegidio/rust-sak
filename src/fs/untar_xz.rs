use std::borrow::Cow;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use liblzma::read::XzDecoder;
use tar::EntryType;

use super::extract::{EntryKind, Extractor, RawEntry, Totals};
use super::{ExtractOptions, ExtractSummary, FsError, Result};

/// Extracts an xz-compressed TAR archive into `target_dir`, creating it if it does not exist.
///
/// Applies exactly the same validation as [`unzip`](super::unzip) — escaping names refused, symbolic links checked
/// lexically and through an anchored directory handle, [`ExtractOptions`] ceilings enforced as the stream is read.
///
/// TAR can describe things a filesystem entry cannot safely be created from: device nodes, FIFOs, sockets and hard
/// links. These are **skipped and counted**, never created, since honouring them would let an archive hand a caller a
/// character device where they expected a file.
///
/// Names are read with `path_bytes` rather than `path`, because the latter already rewrites separators on Windows —
/// which would let `..\escape` arrive pre-laundered into something the validator no longer recognises as an escape.
///
/// ```no_run
/// use rust_sak::fs::{untar_xz, ExtractOptions};
///
/// let summary = untar_xz("backup.tar.xz", "/tmp/backup", &ExtractOptions::new())?;
/// println!("{} files, {} skipped", summary.files, summary.skipped);
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::Io`] if the archive is missing, malformed, or not valid xz — `tar` and `liblzma` both report
/// through [`std::io::Error`], so a corrupt stream arrives as an I/O failure rather than a distinct variant.
/// Otherwise as [`unzip`](super::unzip): [`FsError::IllegalPath`], [`FsError::IllegalSymlink`],
/// [`FsError::LimitExceeded`] and [`FsError::DeclaredSizeMismatch`].
pub fn untar_xz(
    archive: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    options: &ExtractOptions,
) -> Result<ExtractSummary> {
    let decoder = XzDecoder::new(BufReader::new(File::open(archive)?));
    let mut tar = tar::Archive::new(decoder);

    // TAR carries its sizes inside the compressed stream, so a total would mean decompressing the whole archive twice.
    // Reporting nothing is the honest answer; a caller renders an indeterminate bar.
    let mut extractor = Extractor::new(target_dir.as_ref(), options, Totals::default())?;

    for entry in tar.entries()? {
        let mut entry = entry?;

        // Everything is read out of the header before `entry` is borrowed as the content stream.
        let name = utf8_name(&entry.path_bytes(), "entry name")?;
        let entry_type = entry.header().entry_type();
        let mode = entry.header().mode().ok();
        let size = entry.size();
        let link_target = match entry.link_name_bytes() {
            Some(target) => Some(Cow::Owned(utf8_name(&target, "symlink target")?)),
            None => None,
        };

        let raw = RawEntry {
            name: Cow::Owned(name),
            kind: classify(entry_type),
            size,
            mode,
            link_target,
        };
        extractor.visit(raw, &mut entry)?;
    }

    extractor.finish()
}

/// Decides what a TAR entry is, mapping everything a filesystem cannot safely reproduce to [`EntryKind::Other`].
fn classify(entry_type: EntryType) -> EntryKind {
    if entry_type.is_dir() {
        EntryKind::Dir
    } else if entry_type.is_symlink() {
        EntryKind::Symlink
    } else if entry_type.is_file() {
        EntryKind::File
    } else {
        // Hard links, FIFOs, character and block devices, sockets, and the metadata entry types.
        EntryKind::Other
    }
}

/// Decodes a TAR header field as UTF-8, refusing rather than substituting.
///
/// A lossy conversion would replace the offending bytes and hand the validator a name that is no longer the one the
/// archive stored — the last thing a security check should be looking at.
fn utf8_name(bytes: &[u8], what: &str) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| FsError::IllegalPath {
        path: String::from_utf8_lossy(bytes).into_owned(),
        reason: format!("{what} is not valid UTF-8"),
    })
}
