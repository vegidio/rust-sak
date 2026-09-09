use std::path::Path;

use super::ext_filter::ExtFilter;

/// How [`list_path`](super::list_path) should walk a directory.
///
/// The default lists **regular files one level deep, unfiltered** — the common case — and each builder method opts
/// into more. Methods consume and return `self`, so a one-off reads as a temporary and a reusable configuration can
/// be built once and passed to many calls:
///
/// ```
/// use rust_sak::fs::ListOptions;
///
/// // A temporary, for a single call.
/// let images = ListOptions::new().recursive(true).extensions(["jpg", "png"]);
///
/// // `extension` accumulates rather than replacing, so this is the same set.
/// let same = ListOptions::new().recursive(true).extension("jpg").extension(".PNG");
/// assert_eq!(images, same);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub(super) dirs: bool,
    pub(super) files: bool,
    pub(super) recursive: bool,
    pub(super) filter: ExtFilter,
}

impl ListOptions {
    /// Creates the default options: regular files only, one level deep, no extension filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Includes directories in the listing (default `false`).
    ///
    /// Extension filters never apply to directories, so a filtered listing with `dirs(true)` returns every
    /// subdirectory alongside the matching files.
    pub fn dirs(mut self, include: bool) -> Self {
        self.dirs = include;
        self
    }

    /// Includes files in the listing (default `true`).
    ///
    /// Set this to `false` together with `dirs(true)` to list only directories.
    pub fn files(mut self, include: bool) -> Self {
        self.files = include;
        self
    }

    /// Descends into subdirectories (default `false`).
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Adds one extension to match, case-insensitively and with an optional leading dot.
    ///
    /// Calls **accumulate**, so `.extension("jpg").extension("png")` matches both.
    pub fn extension(mut self, extension: impl AsRef<str>) -> Self {
        self.filter.add(extension.as_ref());
        self
    }

    /// Adds several extensions at once, with the same normalization as [`ListOptions::extension`].
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

    /// Reports whether a file at `path` should be listed.
    pub(super) fn accepts_file(&self, path: &Path) -> bool {
        self.files && self.filter.matches(path)
    }
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            dirs: false,
            files: true,
            recursive: false,
            filter: ExtFilter::default(),
        }
    }
}
