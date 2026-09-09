use std::path::Path;

use super::ext_filter::ExtFilter;

/// How [`copy_files`](super::copy_files) and [`move_files`](super::move_files) should treat their sources.
///
/// The default is **non-recursive and flattened**: only the top level of a directory source is transferred, and every
/// file lands directly in the destination. Methods consume and return `self`, matching [`ListOptions`](super::ListOptions).
///
/// ```
/// use rust_sak::fs::CopyOptions;
///
/// // Mirror a tree, images only.
/// let options = CopyOptions::new()
///     .recursive(true)
///     .preserve_structure(true)
///     .extensions(["jpg", "png"]);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyOptions {
    pub(super) recursive: bool,
    pub(super) preserve_structure: bool,
    pub(super) filter: ExtFilter,
}

impl CopyOptions {
    /// Creates the default options: non-recursive, flattened, no extension filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Descends into subdirectories of a directory source (default `false`).
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Recreates each source directory's layout under the destination (default `false`).
    ///
    /// With this off, every file lands directly in the destination — so two files with the same name from different
    /// subdirectories collide, and the last one copied wins. Turn it on when the tree's shape matters.
    ///
    /// This has no effect on a source that is itself a file: such a source always lands directly in the destination.
    pub fn preserve_structure(mut self, preserve: bool) -> Self {
        self.preserve_structure = preserve;
        self
    }

    /// Adds one extension to transfer, case-insensitively and with an optional leading dot.
    ///
    /// Calls **accumulate**, so `.extension("jpg").extension("png")` transfers both. Files that do not match are
    /// counted in [`CopySummary::skipped`](super::CopySummary::skipped).
    pub fn extension(mut self, extension: impl AsRef<str>) -> Self {
        self.filter.add(extension.as_ref());
        self
    }

    /// Adds several extensions at once, with the same normalization as [`CopyOptions::extension`].
    pub fn extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for extension in extensions {
            self.filter.add(extension.as_ref());
        }
        self
    }

    /// Reports whether a file at `path` should be transferred.
    pub(super) fn accepts(&self, path: &Path) -> bool {
        self.filter.matches(path)
    }
}

/// What a [`copy_files`](super::copy_files) or [`move_files`](super::move_files) call actually did.
///
/// `skipped` is the count of files that were found but excluded by the extension filter. It exists because the
/// alternative — returning nothing — leaves "the filter matched none of them" and "the directory was empty"
/// indistinguishable without walking the tree a second time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopySummary {
    /// How many files were transferred.
    pub files: u64,
    /// How many bytes those files held in total.
    pub bytes: u64,
    /// How many files were found but excluded by the extension filter.
    pub skipped: u64,
}
