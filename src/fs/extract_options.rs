/// How [`extract`](super::extract) and the per-format functions should handle an archive.
///
/// The default imposes **no limits**, allows symbolic links, and honours the modes the archive records. Every limit
/// is opt-in because a sensible ceiling depends entirely on what is being extracted; every limit is also an
/// [`Option`] internally, so `max_entries(0)` genuinely means "refuse the first entry" rather than "unlimited".
///
/// ```
/// use rust_sak::fs::ExtractOptions;
///
/// // Untrusted upload: cap the damage, refuse links, and normalize every file's mode.
/// let options = ExtractOptions::new()
///     .max_total_bytes(100 * 1024 * 1024)
///     .max_file_bytes(10 * 1024 * 1024)
///     .max_entries(10_000)
///     .symlinks(false)
///     .file_mode(0o644);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractOptions {
    pub(super) max_total_bytes: Option<u64>,
    pub(super) max_file_bytes: Option<u64>,
    pub(super) max_entries: Option<u64>,
    pub(super) symlinks: bool,
    pub(super) file_mode: Option<u32>,
}

impl ExtractOptions {
    /// Creates the default options: no limits, symlinks allowed, archive modes honoured.
    pub fn new() -> Self {
        Self::default()
    }

    /// Caps the total number of uncompressed bytes written across all entries.
    ///
    /// This is the defence against a compression bomb: a few kilobytes of archive that expand to terabytes.
    pub fn max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = Some(bytes);
        self
    }

    /// Caps the number of uncompressed bytes any single entry may contribute.
    pub fn max_file_bytes(mut self, bytes: u64) -> Self {
        self.max_file_bytes = Some(bytes);
        self
    }

    /// Caps how many entries the archive may contain.
    ///
    /// **Every** entry counts, including ones that are skipped, so padding an archive with entries that extract to
    /// nothing cannot be used to slip past the cap.
    pub fn max_entries(mut self, entries: u64) -> Self {
        self.max_entries = Some(entries);
        self
    }

    /// Allows or refuses symbolic links (default `true`).
    ///
    /// Links are validated either way — a link that would point outside the target directory is always an error.
    /// Setting this to `false` skips them entirely, which is the right call when the extracted tree will be served
    /// or shipped somewhere that resolves links with more authority than this process has.
    pub fn symlinks(mut self, allow: bool) -> Self {
        self.symlinks = allow;
        self
    }

    /// Overrides the mode of every extracted **regular file**, ignoring what the archive recorded.
    ///
    /// Directories and symbolic links are unaffected. Only the permission bits are used, so an archive can never
    /// smuggle setuid, setgid or the sticky bit through this. Has no effect on Windows.
    pub fn file_mode(mut self, mode: u32) -> Self {
        self.file_mode = Some(mode);
        self
    }
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            max_total_bytes: None,
            max_file_bytes: None,
            max_entries: None,
            symlinks: true,
            file_mode: None,
        }
    }
}

/// What an extraction actually did.
///
/// The counters are the only way several behaviours become observable: an entry that is skipped — a device node, an
/// entry naming the target directory itself, a symbolic link on Windows — leaves nothing on disk, so without
/// `skipped` a caller cannot tell a partially-filtered archive from a fully-extracted one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractSummary {
    /// How many regular files were written.
    pub files: u64,
    /// How many directories were created.
    pub directories: u64,
    /// How many symbolic links were created.
    pub symlinks: u64,
    /// How many entries were deliberately not extracted.
    pub skipped: u64,
    /// How many bytes of file content were written.
    pub bytes: u64,
}
