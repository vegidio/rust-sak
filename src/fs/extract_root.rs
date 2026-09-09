use std::path::Path;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, DirBuilder, OpenOptions};

use super::{FsError, Result};

/// The mode directories are created with while extraction is still in progress.
///
/// Whatever the archive asks for, a directory has to stay writable by this process until its children are written;
/// the archive's real mode is applied afterwards by [`Extractor::finish`](super::extract::Extractor::finish).
#[cfg(unix)]
const WORKING_DIR_MODE: u32 = 0o755;

/// The mode a regular file gets when the archive records none.
#[cfg(unix)]
const DEFAULT_FILE_MODE: u32 = 0o644;

/// A target directory that archive entries cannot escape.
///
/// Every path handed to this type is resolved **through** a [`cap_std::fs::Dir`], which anchors the lookup to the
/// target directory at the OS level — `openat2` with `RESOLVE_BENEATH` on modern Linux, a component-at-a-time
/// `O_NOFOLLOW` walk elsewhere. That is what makes it different from joining paths and hoping: a symbolic link
/// planted by an earlier entry, or one that was already sitting in the target directory, cannot be traversed even
/// though the path that walks through it looks perfectly ordinary.
///
/// All the platform differences in extraction live here. On Windows there is no contained way to create a symbolic
/// link, and no mode to apply, so both degrade to a skip rather than an error.
pub(super) struct ExtractRoot {
    dir: Dir,
}

impl ExtractRoot {
    /// Creates `target_dir` if needed and anchors a handle to it.
    ///
    /// `target_dir` is the caller's own path and is trusted; only the entries written into it are not. That is why
    /// this is the one place an ambient, unsandboxed path is used.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the directory cannot be created or opened.
    pub(super) fn open(target_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(target_dir)?;
        let dir = Dir::open_ambient_dir(target_dir, ambient_authority())?;

        Ok(Self { dir })
    }

    /// Creates a directory and any missing parents.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the path escapes the root or the directory cannot be created.
    pub(super) fn create_dir(&self, path: &Path) -> Result<()> {
        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(WORKING_DIR_MODE);
        }

        match self.dir.create_dir_with(path, &builder) {
            Ok(()) => Ok(()),
            // Something already occupies the path. A recursive create succeeds when that something is a directory,
            // so reaching here means it is not one — either a plain file the archive also named, or a symbolic link
            // the anchored handle refuses to follow because it leads out of the root. Both must be refused, and
            // `metadata` failing is itself the signal for the second.
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => match self.dir.metadata(path) {
                Ok(metadata) if metadata.is_dir() => Ok(()),
                _ => Err(FsError::IllegalPath {
                    path: path.display().to_string(),
                    reason: "exists and is not a directory inside the target directory".to_string(),
                }),
            },
            Err(err) => Err(escape_or_io(err, path)),
        }
    }

    /// Creates or truncates a regular file, applying `mode` only when the file is new.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the path escapes the root or the file cannot be opened.
    pub(super) fn create_file(&self, path: &Path, mode: Option<u32>) -> Result<cap_std::fs::File> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            self.create_dir(parent)?;
        }

        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(mode.unwrap_or(DEFAULT_FILE_MODE));
        }
        #[cfg(not(unix))]
        let _ = mode;

        self.dir
            .open_with(path, &options)
            .map_err(|err| escape_or_io(err, path))
    }

    /// Creates a symbolic link, reporting `false` when the platform cannot make one safely.
    ///
    /// Windows has no contained symlink primitive in `cap-std`, and creating one through an ambient path would give
    /// up the containment this whole type exists to provide — so the entry is skipped and counted instead.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the path escapes the root or the link cannot be created.
    pub(super) fn create_symlink(&self, path: &Path, target: &str) -> Result<bool> {
        #[cfg(not(windows))]
        {
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                self.create_dir(parent)?;
            }

            self.dir.symlink(target, path).map_err(|err| escape_or_io(err, path))?;
            Ok(true)
        }

        #[cfg(windows)]
        {
            let _ = (path, target);
            Ok(false)
        }
    }

    /// Confirms that a freshly created link still resolves inside the root, removing it if it does not.
    ///
    /// This is the check that a purely lexical one cannot make. `a` → `.` followed by `b` → `a/..` is contained at
    /// every textual step, yet `b` lands in the root's parent; only resolving the finished link through the anchored
    /// handle reveals it.
    ///
    /// The check **fails closed**. Three outcomes are accepted: the target resolves (fine), it does not exist
    /// (a dangling link is legal and common), or resolving it loops (`ELOOP` — self-referential, but contained).
    /// Anything else is treated as an escape.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::IllegalSymlink`] if the link resolves outside the root.
    pub(super) fn verify_symlink(&self, path: &Path, link: &str, target: &str) -> Result<()> {
        match self.dir.metadata(path) {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) if is_symlink_loop(&err) => Ok(()),
            Err(_) => {
                // Leaving the link in place would hand the caller a booby-trapped tree, so remove it before failing.
                let _ = self.dir.remove_file(path);
                Err(FsError::IllegalSymlink {
                    link: link.to_string(),
                    target: target.to_string(),
                })
            }
        }
    }

    /// Applies a directory's recorded mode, once its contents have been written.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the mode cannot be applied.
    #[cfg(unix)]
    pub(super) fn set_dir_mode(&self, path: &Path, mode: u32) -> Result<()> {
        use cap_std::fs::{Permissions, PermissionsExt};

        self.dir.set_permissions(path, Permissions::from_mode(mode))?;

        Ok(())
    }
}

/// `ELOOP`, which `std` only exposes as `ErrorKind::FilesystemLoop` behind the unstable `io_error_more` feature.
#[cfg(any(target_os = "linux", target_os = "android"))]
const ELOOP: i32 = 40;
/// `ELOOP` on the BSDs, macOS included.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const ELOOP: i32 = 62;

/// Reports whether `err` is the kernel giving up on a self-referential link.
///
/// Such a link never resolves anywhere, least of all outside the root, so it is contained — but it still cannot be
/// stat'd, and [`ExtractRoot::verify_symlink`] fails closed on anything it does not recognise.
fn is_symlink_loop(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

/// Turns the anchored handle's "that path led outside" refusal into an error that says so.
///
/// `cap-primitives` reports an escape as a plain [`std::io::ErrorKind::PermissionDenied`], which would otherwise
/// reach the caller as an unremarkable I/O failure — the one outcome a caller most needs to be able to distinguish.
/// A real permission failure inside the target directory lands here too, so the wording covers both.
fn escape_or_io(err: std::io::Error, path: &Path) -> FsError {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => FsError::IllegalPath {
            path: path.display().to_string(),
            reason: "could not be resolved inside the target directory".to_string(),
        },
        _ => FsError::Io(err),
    }
}
