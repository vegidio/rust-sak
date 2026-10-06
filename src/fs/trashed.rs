use std::path::{Path, PathBuf};

/// A file moved to the Trash by [`move_to_trash`](super::move_to_trash), which
/// [`restore_from_trash`](super::restore_from_trash) can put back.
///
/// It remembers where the file was and how to find it in the Trash again. How it finds it differs per platform: on
/// macOS it keeps the file's own path inside the Trash, while on Windows and Linux it keeps the moment the file was
/// moved and looks it up again by its original path, see [`restore_from_trash`](super::restore_from_trash).
///
/// A handle lives only as long as the program keeps it; nothing is written to disk.
#[derive(Debug, Clone)]
pub struct Trashed {
    pub(super) original: PathBuf,
    pub(super) locator: Locator,
}

/// Where the platform put the file. `None` when the platform moved it but did not say where, so it cannot be
/// restored.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub(super) struct Locator {
    pub(super) in_trash: Option<PathBuf>,
}

/// When the file was moved, in whole seconds since the UNIX epoch, taken just before the platform was asked.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
pub(super) struct Locator {
    pub(super) trashed_at: i64,
}

impl Trashed {
    /// Where the file was before it went to the Trash, and where [`restore_from_trash`](super::restore_from_trash)
    /// puts it back. Its folder is canonical; the file name is the one it had.
    pub fn original_path(&self) -> &Path {
        &self.original
    }

    /// A handle for `original` whose item can never be found in the Trash, for tests that must not reach it.
    #[cfg(test)]
    pub(crate) fn for_tests(original: PathBuf) -> Self {
        #[cfg(target_os = "macos")]
        let locator = Locator { in_trash: None };
        #[cfg(not(target_os = "macos"))]
        let locator = Locator { trashed_at: i64::MAX };

        Self { original, locator }
    }
}
