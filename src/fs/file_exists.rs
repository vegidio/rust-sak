use std::path::Path;

/// Reports whether `path` exists and is a regular file.
///
/// Symbolic links are followed, so a link pointing at a regular file counts as one. Directories do **not**, and
/// neither does a path that cannot be read for any other reason — a permission error on an intermediate component is
/// indistinguishable from absence here, so callers that need to tell the two apart should use
/// [`std::fs::metadata`] directly.
pub fn file_exists(path: impl AsRef<Path>) -> bool {
    path.as_ref().is_file()
}
