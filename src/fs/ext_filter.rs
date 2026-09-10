use std::path::Path;

/// A set of file extensions to match against, compared case-insensitively and without regard to a leading dot.
///
/// `"jpg"`, `".jpg"` and `"JPG"` all normalize to the same entry, so callers can pass whichever form reads best at
/// the call site. An **empty** filter matches everything, which is what makes "no filter configured" and "match all"
/// the same state rather than two cases every caller has to distinguish.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ExtFilter {
    extensions: Vec<String>,
}

impl ExtFilter {
    /// Adds one extension, normalizing it and ignoring duplicates.
    ///
    /// An extension that is empty once the leading dots are stripped is dropped: it would otherwise match every
    /// extensionless file, which is never what `extension("")` is asking for.
    pub(super) fn add(&mut self, extension: &str) {
        let normalized = extension.trim_start_matches('.').to_lowercase();
        if !normalized.is_empty() && !self.extensions.contains(&normalized) {
            self.extensions.push(normalized);
        }
    }

    /// Reports whether `path`'s extension is in the set.
    ///
    /// An empty set matches every path. A path with no extension matches only an empty set, so filtering by any
    /// extension excludes `README` and `Makefile`.
    pub(super) fn matches(&self, path: &Path) -> bool {
        if self.extensions.is_empty() {
            return true;
        }

        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return false;
        };

        // `add` already lowercased everything in the set, so an ASCII-insensitive compare avoids allocating a
        // lowercased copy of the extension for every path tested.
        self.extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    }
}
