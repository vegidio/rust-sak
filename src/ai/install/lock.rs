//! The exclusive lock on a dependency's folder, so two processes sharing a root never install into one folder at once.
//!
//! It is the operating system's advisory lock on `<folder>/.lock` (`flock` on Unix, `LockFileEx` on Windows), held
//! through an open file, so the system releases it when the process holding it ends, however it ends.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use super::marker::LOCK;

/// A held lock on a dependency's folder, released when dropped.
#[derive(Debug)]
pub(super) struct FolderLock {
    // Held only to keep the lock: closing the file releases it.
    _file: File,
}

/// Lock `dir`, creating it, and waiting for another process that holds the lock to let it go.
pub(super) async fn acquire(dir: &Path) -> io::Result<FolderLock> {
    if let Some(lock) = try_acquire(dir)? {
        return Ok(lock);
    }
    tracing::info!("waiting for another process installing into {}", dir.display());
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || acquire_blocking(&dir))
        .await
        .map_err(io::Error::other)?
}

/// Lock `dir`, creating it, unless another process holds the lock.
pub(super) fn try_acquire(dir: &Path) -> io::Result<Option<FolderLock>> {
    loop {
        let (file, path) = open(dir)?;
        match file.try_lock() {
            Ok(()) if still_at(&file, &path) => return Ok(Some(FolderLock { _file: file })),
            // The folder was removed, its lock file with it, between the open and the lock: lock the new one.
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(cause)) => return Err(cause),
        }
    }
}

fn acquire_blocking(dir: &Path) -> io::Result<FolderLock> {
    loop {
        let (file, path) = open(dir)?;
        file.lock()?;
        if still_at(&file, &path) {
            return Ok(FolderLock { _file: file });
        }
    }
}

fn open(dir: &Path) -> io::Result<(File, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOCK);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    Ok((file, path))
}

/// Whether `file` is still the lock file at `path`. A process that removes a folder unlinks its lock file while it
/// holds the lock, so one that was waiting on it ends up holding a lock no one else can see.
#[cfg(unix)]
fn still_at(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(now)) => (held.dev(), held.ino()) == (now.dev(), now.ino()),
        _ => false,
    }
}

/// Windows cannot delete a file another process has open, so the lock file at `path` is always the one held.
#[cfg(not(unix))]
fn still_at(_file: &File, _path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_held_lock_is_refused_and_waited_for() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-lock").unwrap();
        let folder = dir.path().join("cuda/13.3.0");
        let held = try_acquire(&folder).unwrap().expect("nobody holds it");
        assert!(folder.join(LOCK).is_file(), "the folder and its lock file are created");
        assert!(try_acquire(&folder).unwrap().is_none(), "held by another file");

        let waiting = tokio::spawn({
            let folder = folder.clone();
            async move { acquire(&folder).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "it waits");
        drop(held);
        let lock = tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("released")
            .unwrap();
        assert!(lock.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_waiter_on_a_removed_folder_locks_the_new_one() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-lock").unwrap();
        let folder = dir.path().join("cuda/13.3.0");
        let held = try_acquire(&folder).unwrap().unwrap();
        let waiting = tokio::spawn({
            let folder = folder.clone();
            async move { acquire(&folder).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::remove_dir_all(&folder).unwrap();
        drop(held);

        let _lock = tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("released")
            .unwrap()
            .unwrap();
        assert!(folder.join(LOCK).is_file(), "a new lock file in a new folder");
        assert!(try_acquire(&folder).unwrap().is_none(), "and it is the one held");
    }
}
