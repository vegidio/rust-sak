use std::fmt;
use std::sync::Arc;

/// The progress callback, shared behind an [`Arc`] so [`ExtractOptions`] stays [`Clone`] and [`Sync`].
type ProgressHook = Arc<dyn Fn(&ExtractProgress) + Send + Sync>;

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
#[derive(Clone)]
pub struct ExtractOptions {
    pub(super) max_total_bytes: Option<u64>,
    pub(super) max_file_bytes: Option<u64>,
    pub(super) max_entries: Option<u64>,
    pub(super) symlinks: bool,
    pub(super) file_mode: Option<u32>,
    /// The caller's progress hook, shared rather than owned so the options stay [`Clone`] and [`Sync`].
    pub(super) on_progress: Option<ProgressHook>,
}

/// A callback is not comparable, so equality is decided on the ceilings and, for the hook, on whether two options
/// share the very same one. Two separately-written closures doing identical work compare unequal — the only answer
/// available without asking a function whether it equals another function.
impl PartialEq for ExtractOptions {
    fn eq(&self, other: &Self) -> bool {
        self.max_total_bytes == other.max_total_bytes
            && self.max_file_bytes == other.max_file_bytes
            && self.max_entries == other.max_entries
            && self.symlinks == other.symlinks
            && self.file_mode == other.file_mode
            && match (&self.on_progress, &other.on_progress) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl Eq for ExtractOptions {}

/// Renders every field except the hook, which is reported as present or absent — a `dyn Fn` has nothing else to say.
impl fmt::Debug for ExtractOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractOptions")
            .field("max_total_bytes", &self.max_total_bytes)
            .field("max_file_bytes", &self.max_file_bytes)
            .field("max_entries", &self.max_entries)
            .field("symlinks", &self.symlinks)
            .field("file_mode", &self.file_mode)
            .field("on_progress", &self.on_progress.is_some())
            .finish()
    }
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

    /// Sets a callback invoked as the extraction proceeds, receiving an [`ExtractProgress`] snapshot.
    ///
    /// It fires **within** an entry as well as between entries, which is the whole point for an archive that is one
    /// very large file: reporting only per entry would render such an expansion as a single jump from nothing to
    /// done. Updates are coalesced — roughly one per 256 KiB written or per 100 ms, plus one as each entry finishes —
    /// so the rate does not depend on how the archive happens to be chunked.
    ///
    /// **The callback must not block.** It runs inline in the copy loop, so anything slow in it slows the extraction
    /// itself. Send the snapshot somewhere and return; do the work elsewhere.
    ///
    /// `Fn`, not `FnMut`, so [`ExtractOptions`] stays [`Clone`] and [`Sync`]; a callback that needs to mutate state
    /// uses interior mutability, as it would for any shared callback.
    ///
    /// ```
    /// use std::sync::atomic::{AtomicU64, Ordering};
    /// use std::sync::Arc;
    /// use rust_sak::fs::ExtractOptions;
    ///
    /// let written = Arc::new(AtomicU64::new(0));
    /// let counter = Arc::clone(&written);
    /// let options = ExtractOptions::new().on_progress(move |progress| {
    ///     counter.store(progress.bytes, Ordering::Relaxed);
    /// });
    /// ```
    pub fn on_progress(mut self, f: impl Fn(&ExtractProgress) + Send + Sync + 'static) -> Self {
        self.on_progress = Some(Arc::new(f));
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
            on_progress: None,
        }
    }
}

/// How far an extraction has got, handed to the callback set by [`ExtractOptions::on_progress`].
///
/// The totals come from the archive's header, which is why they are optional rather than merely unknown-until-later:
/// ZIP's central directory and 7z's header both carry every entry's uncompressed size, so both report [`Some`].
/// TAR.XZ carries none — the sizes live inside the compressed stream, and producing a total would mean decompressing
/// the whole archive twice — so it reports [`None`] rather than a guess, and a caller renders an indeterminate bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractProgress {
    /// How many entries have been processed so far, counted exactly as [`total_entries`](ExtractProgress::total_entries)
    /// counts them — every entry the archive yields, including ones that are deliberately skipped.
    pub entries: u64,
    /// How many uncompressed bytes of file content have been written so far. Never decreases.
    pub bytes: u64,
    /// How many entries the archive says it holds, or [`None`] for a format whose header does not say.
    pub total_entries: Option<u64>,
    /// How many uncompressed bytes the archive says its entries hold, or [`None`] for a format whose header does not
    /// say.
    pub total_bytes: Option<u64>,
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
