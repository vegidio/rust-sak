use std::borrow::Cow;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::extract_root::ExtractRoot;
use super::safe_path::{validate_entry_name, validate_symlink_target};
use super::{ExtractOptions, ExtractProgress, ExtractSummary, FsError, Limit, Result};

/// The largest symbolic-link target this module will read from an archive.
///
/// ZIP and 7z store a link's target as the entry's *contents*, so reading it means reading a stream an attacker
/// controls. Real targets are bounded by `PATH_MAX`; this is generous room above that and no more.
const MAX_SYMLINK_TARGET: u64 = 4096;

/// The mode a directory gets when the archive records none.
const DEFAULT_DIR_MODE: u32 = 0o755;

/// How many bytes may be written between progress updates before one is forced out.
///
/// Entry bodies are copied in chunks of a few kilobytes, so reporting per chunk would mean tens of thousands of calls
/// across a large archive. Coalescing to a byte and a time bound keeps a progress bar smooth while making the cost
/// independent of how the archive happens to be framed — the same discipline the streaming download uses.
const PROGRESS_BYTES: u64 = 256 * 1024;

/// How long may pass between progress updates before one is forced out, so a slow expansion still ticks.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The file-type mask within a Unix mode.
pub(super) const S_IFMT: u32 = 0o170_000;

/// The file-type bits marking a regular file.
pub(super) const S_IFREG: u32 = 0o100_000;

/// The file-type bits marking a symbolic link.
pub(super) const S_IFLNK: u32 = 0o120_000;

/// What an archive entry is, reduced to the four cases extraction actually distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    /// A directory.
    Dir,
    /// A regular file.
    File,
    /// A symbolic link.
    Symlink,
    /// Anything else — a device node, FIFO, socket or hard link. Never extracted.
    Other,
}

/// One archive entry, in the shape the extractor needs, whichever crate produced it.
pub(super) struct RawEntry<'a> {
    /// The entry's name exactly as the archive stored it, before any validation.
    pub name: Cow<'a, str>,
    /// What kind of thing the entry is.
    pub kind: EntryKind,
    /// The uncompressed size the archive's metadata claims. Treated as a claim, never as a fact.
    pub size: u64,
    /// The Unix mode the archive recorded, if it recorded one.
    pub mode: Option<u32>,
    /// A symbolic link's target, for formats that store it in the header rather than the entry's stream.
    pub link_target: Option<Cow<'a, str>>,
}

/// The running total of what an extraction is allowed to consume.
///
/// Every ceiling is an [`Option`] where [`None`] means "no ceiling". Nothing here uses zero as a magic value, so
/// `max_entries(0)` rejects the first entry instead of silently meaning "unlimited".
pub(super) struct Budget<'a> {
    /// The ceilings, read straight from the caller's options so there is only ever one copy of the policy.
    options: &'a ExtractOptions,
    pub(super) used_bytes: u64,
    used_entries: u64,
}

impl<'a> Budget<'a> {
    pub(super) fn new(options: &'a ExtractOptions) -> Self {
        Self {
            options,
            used_bytes: 0,
            used_entries: 0,
        }
    }

    /// Counts one entry against the entry ceiling.
    ///
    /// Called for **every** entry the archive yields, before any decision about whether it will be extracted, so
    /// padding an archive with entries that produce nothing cannot buy room under the cap.
    pub(super) fn count_entry(&mut self) -> Result<()> {
        self.used_entries += 1;
        match self.options.max_entries {
            Some(max) if self.used_entries > max => Err(FsError::LimitExceeded {
                limit: Limit::Entries,
                allowed: max,
            }),
            _ => Ok(()),
        }
    }

    /// Claims room for an entry that *says* it is `declared` bytes, before a byte of it is written.
    ///
    /// Reserving up front is the point: a header claiming to be tiny must not be able to get a file handle open and
    /// then stream gigabytes through it. Whatever is not used is handed back by [`Budget::settle`].
    pub(super) fn reserve(&mut self, declared: u64) -> Result<()> {
        if let Some(max) = self.options.max_file_bytes
            && declared > max
        {
            return Err(FsError::LimitExceeded {
                limit: Limit::FileBytes,
                allowed: max,
            });
        }

        if let Some(max) = self.options.max_total_bytes
            && self.used_bytes.saturating_add(declared) > max
        {
            return Err(FsError::LimitExceeded {
                limit: Limit::TotalBytes,
                allowed: max,
            });
        }

        self.used_bytes = self.used_bytes.saturating_add(declared);

        Ok(())
    }

    /// Returns the difference between what was reserved and what was actually written.
    ///
    /// Without this, an archive that over-declares its sizes would exhaust the budget with files that never used it.
    pub(super) fn settle(&mut self, reserved: u64, actual: u64) {
        self.used_bytes = self.used_bytes.saturating_sub(reserved.saturating_sub(actual));
    }
}

/// What an archive's header says it holds, read once by the format adapter before extraction starts.
///
/// Both fields are optional per format rather than per archive: ZIP and 7z always know, TAR.XZ never does.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Totals {
    /// How many entries the header lists.
    pub entries: Option<u64>,
    /// How many uncompressed bytes the header's entries add up to.
    pub bytes: Option<u64>,
}

/// Drives the caller's progress callback, coalescing updates and holding the running counts.
///
/// A no-op when no callback was set: [`Reporter::report`] returns immediately, so an extraction without a hook does
/// exactly what it did before — one `Option` check per chunk.
struct Reporter<'a> {
    hook: Option<&'a (dyn Fn(&ExtractProgress) + Send + Sync)>,
    progress: ExtractProgress,
    reported_bytes: u64,
    reported_at: Instant,
}

impl<'a> Reporter<'a> {
    fn new(options: &'a ExtractOptions, totals: Totals) -> Self {
        Self {
            hook: options.on_progress.as_deref(),
            progress: ExtractProgress {
                entries: 0,
                bytes: 0,
                total_entries: totals.entries,
                total_bytes: totals.bytes,
            },
            reported_bytes: 0,
            reported_at: Instant::now(),
        }
    }

    /// Adds `bytes` to the running count and calls the hook if either threshold has been crossed.
    ///
    /// Called from inside the copy loop, which is what lets a single very large entry — the shape the ONNX Runtime
    /// archive actually has — report more than the one update per-entry reporting would give it.
    fn advance(&mut self, bytes: u64) {
        self.progress.bytes += bytes;
        if self.progress.bytes - self.reported_bytes >= PROGRESS_BYTES || self.reported_at.elapsed() >= PROGRESS_INTERVAL
        {
            self.flush();
        }
    }

    /// Counts one finished entry and reports unconditionally, so every entry boundary is visible whatever the
    /// thresholds said — including the last one, which carries the final totals.
    fn finish_entry(&mut self) {
        self.progress.entries += 1;
        self.flush();
    }

    /// Hands the current snapshot to the hook and resets the coalescing window.
    fn flush(&mut self) {
        if let Some(hook) = self.hook {
            hook(&self.progress);
        }
        self.reported_bytes = self.progress.bytes;
        self.reported_at = Instant::now();
    }
}

/// A [`Write`] that feeds a [`Reporter`] as bytes pass through it on their way to the real writer.
///
/// Wrapping the writer rather than re-implementing the copy is what keeps this out of the security-critical path: the
/// reserve-before-write accounting, the declared-size check and the anchored file handle are all untouched.
struct ProgressWriter<'a, 'r, W: Write> {
    inner: W,
    reporter: &'a mut Reporter<'r>,
}

impl<W: Write> Write for ProgressWriter<'_, '_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.reporter.advance(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The stateful half of extraction, shared by every format.
///
/// The three archive crates disagree about who owns the iteration — `zip` is random access, `tar` lends entries from
/// a sequential reader, and `sevenz-rust2` only pushes through a callback — so none of them can be adapted to a
/// common `Iterator` without a lending iterator. Inverting instead, into a visitor each adapter drives, costs three
/// short straight-line loops and keeps every security decision in one place.
pub(super) struct Extractor<'a> {
    root: ExtractRoot,
    options: &'a ExtractOptions,
    budget: Budget<'a>,
    dir_modes: Vec<(PathBuf, u32)>,
    summary: ExtractSummary,
    reporter: Reporter<'a>,
}

impl<'a> Extractor<'a> {
    /// Anchors an extractor to `target_dir`, creating it if needed.
    ///
    /// `totals` is whatever the archive's header could tell the format adapter, passed straight through to the
    /// caller's progress hook.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the target directory cannot be created or opened.
    pub(super) fn new(target_dir: &Path, options: &'a ExtractOptions, totals: Totals) -> Result<Self> {
        Ok(Self {
            root: ExtractRoot::open(target_dir)?,
            options,
            budget: Budget::new(options),
            dir_modes: Vec::new(),
            summary: ExtractSummary::default(),
            reporter: Reporter::new(options, totals),
        })
    }

    /// Validates and extracts one entry, reading its contents from `data`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::LimitExceeded`] if a ceiling is hit, [`FsError::IllegalPath`] or
    /// [`FsError::IllegalSymlink`] if the entry would escape the target directory,
    /// [`FsError::DeclaredSizeMismatch`] if the entry outgrows its own header, and [`FsError::Io`] for anything the
    /// filesystem refuses.
    pub(super) fn visit(&mut self, entry: RawEntry<'_>, data: &mut dyn Read) -> Result<()> {
        let result = self.visit_entry(entry, data);
        if result.is_ok() {
            // Reported per entry as well as mid-entry, so the entry count advances and the last update of the
            // extraction always carries the final totals.
            self.reporter.finish_entry();
        }

        result
    }

    /// The body of [`Extractor::visit`], split out so every path through it is followed by one progress update.
    fn visit_entry(&mut self, entry: RawEntry<'_>, data: &mut dyn Read) -> Result<()> {
        self.budget.count_entry()?;

        let components = validate_entry_name(&entry.name)?;
        if components.is_empty() {
            // The entry names the target directory itself, which archives commonly include. There is nothing to
            // create, but it is still an entry the caller may want to know about.
            self.summary.skipped += 1;
            return Ok(());
        }

        let path: PathBuf = components.iter().collect();
        match entry.kind {
            EntryKind::Dir => self.visit_dir(&path, entry.mode),
            EntryKind::File => self.visit_file(&entry, &path, data),
            EntryKind::Symlink => self.visit_symlink(&entry, &components, &path, data),
            EntryKind::Other => {
                self.summary.skipped += 1;
                Ok(())
            }
        }
    }

    /// Finishes the extraction, applying the directory modes that were deferred.
    ///
    /// Modes are applied **deepest first**. A directory the archive wants at `0o500` cannot be tightened while its
    /// children are still being written, so the real mode waits until everything inside it exists.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if a mode cannot be applied.
    pub(super) fn finish(mut self) -> Result<ExtractSummary> {
        #[cfg(unix)]
        {
            self.dir_modes
                .sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
            for (path, mode) in &self.dir_modes {
                self.root.set_dir_mode(path, *mode)?;
            }
        }

        Ok(self.summary)
    }

    /// Creates a directory, deferring its recorded mode until [`Extractor::finish`].
    fn visit_dir(&mut self, path: &Path, mode: Option<u32>) -> Result<()> {
        self.root.create_dir(path)?;
        self.summary.directories += 1;

        if let Some(mode) = mode {
            let mode = match permissions(mode) {
                0 => DEFAULT_DIR_MODE,
                bits => bits,
            };
            self.dir_modes.push((path.to_path_buf(), mode));
        }

        Ok(())
    }

    /// Writes a regular file, reserving its declared size before opening it.
    fn visit_file(&mut self, entry: &RawEntry<'_>, path: &Path, data: &mut dyn Read) -> Result<()> {
        let declared = entry.size;
        self.budget.reserve(declared)?;

        let mode = self.options.file_mode.or(entry.mode).map(permissions);
        let handle = self.root.create_file(path, mode)?;

        // The copy goes through a counting writer rather than a reimplemented loop, so progress is reported from
        // inside a single large entry without any of the checks around it moving.
        let mut writer = ProgressWriter {
            inner: BufWriter::new(handle),
            reporter: &mut self.reporter,
        };

        // Reading one byte past the declared size is what turns "the header lied" into an error instead of a file
        // silently truncated at the length the archive claimed.
        let written = io::copy(&mut data.take(declared.saturating_add(1)), &mut writer)?;
        writer.flush()?;
        drop(writer);

        if written > declared {
            return Err(FsError::DeclaredSizeMismatch {
                entry: entry.name.to_string(),
                declared,
            });
        }

        self.budget.settle(declared, written);
        self.summary.files += 1;
        self.summary.bytes += written;

        Ok(())
    }

    /// Creates a symbolic link, after checking its target both lexically and through the anchored handle.
    fn visit_symlink(
        &mut self,
        entry: &RawEntry<'_>,
        components: &[&str],
        path: &Path,
        data: &mut dyn Read,
    ) -> Result<()> {
        if !self.options.symlinks {
            self.summary.skipped += 1;
            return Ok(());
        }

        let target = match &entry.link_target {
            Some(target) => target.to_string(),
            // ZIP and 7z keep the target in the entry's stream rather than its header.
            None => read_link_target(data)?,
        };

        validate_symlink_target(components, &target)?;

        if !self.root.create_symlink(path, &target)? {
            // Windows: no contained way to create the link, so it is skipped rather than made unsafely.
            self.summary.skipped += 1;
            return Ok(());
        }

        self.root.verify_symlink(path, &components.join("/"), &target)?;
        self.summary.symlinks += 1;

        Ok(())
    }
}

/// Reads a symbolic link's target from an entry's stream, bounded by [`MAX_SYMLINK_TARGET`].
fn read_link_target(data: &mut dyn Read) -> Result<String> {
    let mut target = String::new();
    data.take(MAX_SYMLINK_TARGET).read_to_string(&mut target)?;

    Ok(target.trim_end_matches(['\r', '\n', '\0']).to_string())
}

/// Reduces an archive's recorded mode to permission bits.
///
/// Masking with `0o777` is what keeps setuid, setgid and the sticky bit from surviving extraction — an archive must
/// never be able to leave a setuid binary behind. It also strips the file-type bits, which `zip` includes in the mode
/// it reports. A mode recording nothing usable comes back as `0`; choosing what to do about that is the caller's,
/// since only directories have a default worth falling back to.
pub(super) fn permissions(mode: u32) -> u32 {
    mode & 0o777
}
