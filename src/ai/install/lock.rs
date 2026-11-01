//! The exclusive lock on a dependency's folder, so two processes sharing a root never install into one folder at once.
//!
//! It is the operating system's advisory lock on `<folder>.lock`, beside the folder (`flock` on Unix, `LockFileEx` on
//! Windows), held through an open file, so the system releases it when the process holding it ends, however it ends.
//! The lock file is never removed, so it stays the one every process opens even while the folder is replaced or
//! removed, and the folder's contents never include it.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

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
    let (file, dir) = (open(dir)?, dir.to_path_buf());
    super::joined(
        tokio::task::spawn_blocking(move || {
            file.lock()?;
            held(file, &dir)
        })
        .await,
    )?
}

/// Lock `dir`, creating it, unless another process holds the lock.
pub(super) fn try_acquire(dir: &Path) -> io::Result<Option<FolderLock>> {
    let file = open(dir)?;
    match file.try_lock() {
        Ok(()) => held(file, dir).map(Some),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(cause)) => Err(cause),
    }
}

/// The lock file of `dir`: its name with `.lock` appended, beside it.
pub(super) fn path(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// The lock file of `dir`, opened, creating it and the folder it is in.
fn open(dir: &Path) -> io::Result<File> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path(dir))
}

/// The lock held through `file`, with `dir` created: the process it waited on may have removed the folder.
fn held(file: File, dir: &Path) -> io::Result<FolderLock> {
    std::fs::create_dir_all(dir)?;
    Ok(FolderLock { _file: file })
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
        assert!(
            folder.is_dir() && path(&folder).is_file(),
            "the folder and its lock file beside it are created"
        );
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

    #[tokio::test]
    async fn a_waiter_on_a_removed_folder_gets_it_back() {
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
        assert!(folder.is_dir(), "the folder is created again");
        assert!(try_acquire(&folder).unwrap().is_none(), "and its lock is the one held");
    }
}
