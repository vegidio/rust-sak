//! The install marker: what a dependency's folder holds, recorded when the install finishes and checked locally on
//! every later one.
//!
//! It holds the archive's verified hash and size, and every entry the archive produced with its size. A folder is
//! intact while every recorded entry is still there at its recorded size, which is a `stat` per entry and never a
//! hash: the contents were verified against the API's hash when they were installed.

use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The marker, in the dependency's folder.
pub(in crate::ai) const MARKER: &str = ".installed";

/// The lock, in the dependency's folder. Kept out of the record, and never removed by an install.
pub(super) const LOCK: &str = ".lock";

/// The staging folder, in the dependency's folder.
pub(super) const STAGING: &str = ".staging";

/// The entries of a dependency's folder that are the installer's own, not the archive's.
const OWN: [&str; 3] = [MARKER, LOCK, STAGING];

/// What a finished install left in its folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Marker {
    /// The archive's verified hash, in hex.
    pub hash: String,
    /// The archive's size in bytes, as the API reported it.
    pub size: u64,
    /// Every file and link the archive produced, by path relative to the folder.
    pub files: Vec<File>,
}

/// One installed file or link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct File {
    /// Relative to the dependency's folder, `/`-separated on every platform.
    pub path: String,
    /// Its size, or a link's own size rather than its target's.
    pub size: u64,
}

impl Marker {
    /// The marker in `dir`, if there is one that can be read. A marker of another format, such as the bare hash
    /// earlier releases wrote, reads as none, which costs a reinstall.
    pub fn read(dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join(MARKER)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Record what `dir` now holds, every entry but the installer's own, for an archive of `hash` and `size`.
    pub fn record(dir: &Path, hash: &str, size: u64) -> io::Result<Self> {
        let mut files = Vec::new();
        walk(dir, dir, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self {
            hash: hash.to_owned(),
            size,
            files,
        })
    }

    /// Write the marker into `dir`, whole: it is written beside and renamed into place, so it is never seen
    /// half-written.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        let pending = dir.join(format!("{MARKER}.tmp"));
        std::fs::write(&pending, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        std::fs::rename(&pending, dir.join(MARKER))
    }

    /// Whether every recorded entry is still in `dir` at its recorded size. A link is checked as a link, so the
    /// `libfoo.so -> libfoo.so.1` chains of the Linux archives are not read as corruption. A marker that records
    /// nothing is not an install.
    pub fn intact(&self, dir: &Path) -> bool {
        !self.files.is_empty()
            && self.files.iter().all(|file| {
                relative(&file.path).is_some_and(|path| {
                    std::fs::symlink_metadata(dir.join(path))
                        .is_ok_and(|meta| !meta.is_dir() && meta.len() == file.size)
                })
            })
    }
}

/// Every file and link under `dir`, `root` excluded at its top, appended to `files` by path relative to `root`.
fn walk(root: &Path, dir: &Path, files: &mut Vec<File>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if dir == root && entry.file_name().to_str().is_some_and(is_own) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &entry.path(), files)?;
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let path = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.push(File {
            path,
            size: std::fs::symlink_metadata(entry.path())?.len(),
        });
    }
    Ok(())
}

/// Whether `name`, at the top of a dependency's folder, is the installer's own entry: the marker and its pending
/// write, the lock, or staging.
pub(super) fn is_own(name: &str) -> bool {
    OWN.contains(&name) || name == format!("{MARKER}.tmp")
}

/// A recorded path as a relative path, or `None` for one that would leave the folder, which a marker this crate wrote
/// never holds.
fn relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> crate::fs::TempDir {
        crate::fs::mk_temp_dir("rust-sak-ai-marker").unwrap()
    }

    #[test]
    fn a_record_lists_every_file_but_the_installers_own() {
        let dir = temp();
        std::fs::write(dir.path().join("lib.dylib"), b"12345").unwrap();
        std::fs::create_dir_all(dir.path().join("sub/deeper")).unwrap();
        std::fs::write(dir.path().join("sub/deeper/a.txt"), b"ab").unwrap();
        std::fs::create_dir_all(dir.path().join(STAGING)).unwrap();
        std::fs::write(dir.path().join(STAGING).join("x.7z.part"), b"partial").unwrap();
        std::fs::write(dir.path().join(LOCK), b"").unwrap();
        std::fs::write(dir.path().join(MARKER), b"old").unwrap();

        let marker = Marker::record(dir.path(), "abc", 42).unwrap();
        assert_eq!(
            marker.files,
            [
                File {
                    path: "lib.dylib".into(),
                    size: 5
                },
                File {
                    path: "sub/deeper/a.txt".into(),
                    size: 2
                },
            ]
        );
        assert!(marker.intact(dir.path()));
    }

    #[test]
    fn a_missing_or_resized_file_is_not_intact() {
        let dir = temp();
        std::fs::write(dir.path().join("a"), b"12345").unwrap();
        std::fs::write(dir.path().join("b"), b"12").unwrap();
        let marker = Marker::record(dir.path(), "abc", 1).unwrap();
        assert!(marker.intact(dir.path()));

        std::fs::write(dir.path().join("b"), b"123").unwrap();
        assert!(!marker.intact(dir.path()), "resized");
        std::fs::write(dir.path().join("b"), b"12").unwrap();
        std::fs::remove_file(dir.path().join("a")).unwrap();
        assert!(!marker.intact(dir.path()), "deleted");
    }

    #[test]
    fn it_round_trips_and_an_old_marker_reads_as_none() {
        let dir = temp();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        let marker = Marker::record(dir.path(), "abc", 1).unwrap();
        marker.write(dir.path()).unwrap();
        assert_eq!(Marker::read(dir.path()), Some(marker));
        assert!(!dir.path().join(format!("{MARKER}.tmp")).exists());

        std::fs::write(
            dir.path().join(MARKER),
            "50c3dc45b758105bec33f640c3db9f4efad97c99b3f99c2be52e00cfe89b6811",
        )
        .unwrap();
        assert_eq!(Marker::read(dir.path()), None, "the bare hash of earlier releases");
    }

    #[test]
    fn an_empty_record_or_one_leaving_the_folder_is_not_intact() {
        let dir = temp();
        let empty = Marker {
            hash: "abc".into(),
            size: 0,
            files: Vec::new(),
        };
        assert!(!empty.intact(dir.path()));

        std::fs::write(dir.path().join("a"), b"x").unwrap();
        let escaping = Marker {
            files: vec![File {
                path: "../a".into(),
                size: 1,
            }],
            ..empty
        };
        assert!(!escaping.intact(&dir.path().join("sub")));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_recorded_and_checked_as_links() {
        let dir = temp();
        std::fs::write(
            dir.path().join("libcudart.so.13.3.0"),
            b"a much longer library than its link",
        )
        .unwrap();
        std::os::unix::fs::symlink("libcudart.so.13.3.0", dir.path().join("libcudart.so.13")).unwrap();
        let marker = Marker::record(dir.path(), "abc", 1).unwrap();
        let link = marker.files.iter().find(|file| file.path == "libcudart.so.13").unwrap();
        assert_eq!(link.size, "libcudart.so.13.3.0".len() as u64, "the link's own size");
        assert!(marker.intact(dir.path()));
    }
}
