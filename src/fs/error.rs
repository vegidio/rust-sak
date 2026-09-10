use std::fmt;

/// A convenience alias for results returned by this module.
pub type Result<T> = std::result::Result<T, FsError>;

/// Which extraction ceiling a [`FsError::LimitExceeded`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The cumulative uncompressed size of every entry, set by
    /// [`max_total_bytes`](super::ExtractOptions::max_total_bytes).
    TotalBytes,
    /// The uncompressed size of a single entry, set by [`max_file_bytes`](super::ExtractOptions::max_file_bytes).
    FileBytes,
    /// The number of entries in the archive, set by [`max_entries`](super::ExtractOptions::max_entries).
    Entries,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Limit::TotalBytes => f.write_str("total bytes"),
            Limit::FileBytes => f.write_str("file bytes"),
            Limit::Entries => f.write_str("entries"),
        }
    }
}

/// An error produced by a filesystem or archive-extraction operation.
///
/// Failures from an underlying library are wrapped per source: [`FsError::Io`] (which also covers `tar` and `liblzma`,
/// both of which surface [`std::io::Error`]), [`FsError::Zip`] and [`FsError::SevenZ`]. The remaining variants are this
/// module's own, and the extraction ones are the security-relevant half: an archive entry is rejected *before* it is
/// written, so a caller that sees [`FsError::IllegalPath`] or [`FsError::IllegalSymlink`] knows nothing landed outside
/// the target directory.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// An underlying filesystem, `tar` or `liblzma` operation failed.
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// The ZIP archive could not be read.
    #[error("zip archive could not be read: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// The 7z archive could not be read.
    #[error("7z archive could not be read: {0}")]
    SevenZ(#[from] sevenz_rust2::Error),
    /// A name that must be non-empty was empty.
    #[error("name must not be empty")]
    EmptyName,
    /// The user's configuration directory could not be determined for this platform.
    #[error("the user configuration directory could not be determined")]
    NoConfigDir,
    /// The archive's extension is not one this module can extract.
    #[error("unknown archive format: {name}")]
    UnknownArchiveFormat {
        /// The file name that was rejected, so the caller can report which input was wrong.
        name: String,
    },
    /// An entry name was rejected before anything was written to disk.
    #[error("illegal entry path {path:?}: {reason}")]
    IllegalPath {
        /// The offending name, exactly as the archive stored it.
        path: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A symbolic link would have pointed outside the target directory.
    #[error("illegal symlink {link:?} pointing at {target:?}")]
    IllegalSymlink {
        /// The link's own name within the archive.
        link: String,
        /// The target it tried to point at.
        target: String,
    },
    /// An entry produced more bytes than its header declared, meaning the archive's metadata is untrustworthy.
    ///
    /// This is distinct from [`FsError::LimitExceeded`]: it fires even when no limits are configured, because the
    /// problem is the archive lying about itself rather than a policy the caller set.
    #[error("entry {entry:?} produced more than the {declared} bytes it declared")]
    DeclaredSizeMismatch {
        /// The entry's name within the archive.
        entry: String,
        /// The size its header claimed.
        declared: u64,
    },
    /// Extraction would have exceeded one of the configured ceilings.
    #[error("extraction exceeded the configured {limit} limit of {allowed}")]
    LimitExceeded {
        /// Which ceiling was hit.
        limit: Limit,
        /// The configured value for that ceiling.
        allowed: u64,
    },
}
