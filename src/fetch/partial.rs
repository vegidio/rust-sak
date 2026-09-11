//! Bookkeeping for a download's partially transferred bytes.
//!
//! A transfer never writes to its target path. It writes to `<path>.part` and records what that partial *is* in a
//! `<path>.part.json` sidecar; only a finished transfer renames the `.part` onto the target. Two properties follow,
//! and both are the point of the file existing:
//!
//! - **A file at the target path is complete**, always. An interrupted or cancelled transfer leaves a `.part`, not a
//!   truncated file that [`DownloadMode::Skip`](super::DownloadMode::Skip) would report as finished forever.
//! - **A resume is provably a resume of the right thing.** The sidecar records the URL the bytes came from (and,
//!   when the caller supplied one, a [`resume_key`](super::RequestOptions::resume_key)); a partial that does not
//!   match the current request is deleted rather than appended to. Release assets are commonly versioned in the URL
//!   while keeping the same file name, so "same local path" says nothing about "same bytes".

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the current request believes its bytes are, compared against a sidecar before any byte is appended.
pub(super) struct Identity {
    /// The fully resolved request URL, query parameters included.
    pub url: String,
    /// The caller's own identity for these bytes, from [`RequestOptions::resume_key`](super::RequestOptions::resume_key).
    /// `None` for the callers that have no such value, which is most of them.
    pub resume_key: Option<String>,
}

/// The `<path>.part.json` sidecar: everything needed to decide whether `<path>.part` may be resumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Sidecar {
    /// The URL the partial bytes were requested from.
    pub url: String,
    /// The `ETag` the response carried, when it carried one. Sent back as `If-Range` on the resume, so a server whose
    /// bytes changed at an unchanged URL answers `200` instead of `206`.
    #[serde(default)]
    pub etag: Option<String>,
    /// The full size the response advertised, when it advertised one. Recorded for diagnostics — the resume offset
    /// always comes from the `.part` file's actual length, never from here.
    #[serde(default)]
    pub total: Option<u64>,
    /// The caller-supplied identity, mirrored from [`Identity::resume_key`].
    #[serde(default)]
    pub resume_key: Option<String>,
}

impl Sidecar {
    /// `true` when this sidecar describes the same bytes `identity` is asking for.
    ///
    /// Both halves must agree. The URL alone already catches the common case (a version bump moves the URL but not
    /// the local file name); `resume_key` catches what it cannot — the same URL re-cut with different bytes.
    fn matches(&self, identity: &Identity) -> bool {
        self.url == identity.url && self.resume_key == identity.resume_key
    }
}

/// The path a transfer writes to: the target path with `.part` appended.
///
/// Appended to the whole name rather than replacing an extension, so `archive.tar.xz` becomes `archive.tar.xz.part`
/// and the target's own format stays readable. It also keeps the partial in the same directory as its target, which
/// is what makes the final rename a same-filesystem operation and therefore atomic.
pub(super) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// The path of the sidecar describing `<path>.part`.
pub(super) fn sidecar_path(path: &Path) -> PathBuf {
    let mut name = part_path(path).into_os_string();
    name.push(".json");
    PathBuf::from(name)
}

/// Reads the sidecar at `path`, or `None` if it is missing, unreadable or not valid JSON.
///
/// Every failure collapses to `None` deliberately: a sidecar that cannot be understood is worth exactly as much as
/// one that is not there, and both mean "this partial's identity is unknown", which the caller resolves by starting
/// fresh.
pub(super) async fn read(path: &Path) -> Option<Sidecar> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes `sidecar` to `path`, replacing whatever was there.
///
/// # Errors
///
/// Returns an [`io::Error`] if the file cannot be written.
pub(super) async fn write(path: &Path, sidecar: &Sidecar) -> io::Result<()> {
    let json = serde_json::to_vec(sidecar).map_err(io::Error::other)?;
    tokio::fs::write(path, json).await
}

/// Deletes the partial and its sidecar, ignoring either being absent.
///
/// # Errors
///
/// Returns an [`io::Error`] if a file exists but cannot be removed — a permission problem must not be mistaken for
/// "already gone", or the next attempt would append to bytes it thinks it deleted.
pub(super) async fn discard(part: &Path, sidecar: &Path) -> io::Result<()> {
    remove(part).await?;
    remove(sidecar).await
}

/// Applies the resume decision: keeps `part` only when its sidecar says it belongs to `identity`, and deletes both
/// otherwise.
///
/// The cases that end in a fresh start are a partial whose sidecar names a different URL or `resume_key`, and a
/// partial with no sidecar at all — its identity is unknowable, and guessing is the bug this module exists to
/// prevent.
///
/// # Errors
///
/// Returns an [`io::Error`] if the partial cannot be stat'd or removed.
pub(super) async fn reconcile(part: &Path, sidecar: &Path, identity: &Identity) -> io::Result<()> {
    let resumable = tokio::fs::try_exists(part).await?
        && read(sidecar).await.is_some_and(|recorded| recorded.matches(identity));

    if resumable {
        return Ok(());
    }

    discard(part, sidecar).await
}

/// Turns a finished `.part` into the target file: drops the sidecar, then renames.
///
/// The sidecar goes first so the two possible crash points both fail safe. Crash between them and the leftover
/// `.part` has no sidecar, so the next attempt restarts — correct, at the cost of the bytes. The reverse order would
/// leave a sidecar describing a file that is no longer partial.
///
/// The caller is responsible for having flushed and synced the `.part` before calling this: a rename is only as
/// durable as what preceded it, and an unsynced tail would otherwise hide behind a complete-looking name.
///
/// # Errors
///
/// Returns an [`io::Error`] if the sidecar cannot be removed or the rename fails. The rename replaces an existing
/// target on both Unix and Windows, and `.part` sits beside its target, so it never degrades into a copy.
pub(super) async fn promote(part: &Path, sidecar: &Path, path: &Path) -> io::Result<()> {
    remove(sidecar).await?;
    tokio::fs::rename(part, path).await
}

/// Removes `path`, treating "it was not there" as success.
async fn remove(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_and_sidecar_append_to_the_whole_name() {
        let path = Path::new("/tmp/archive.tar.xz");
        assert_eq!(part_path(path), Path::new("/tmp/archive.tar.xz.part"));
        assert_eq!(sidecar_path(path), Path::new("/tmp/archive.tar.xz.part.json"));
    }

    #[test]
    fn matches_requires_both_url_and_resume_key() {
        let sidecar = Sidecar {
            url: "https://example.com/a.7z".to_string(),
            etag: Some("\"abc\"".to_string()),
            total: Some(10),
            resume_key: Some("hash".to_string()),
        };

        assert!(sidecar.matches(&Identity {
            url: "https://example.com/a.7z".to_string(),
            resume_key: Some("hash".to_string()),
        }));
        // Same file name, different release: the URL differs, so the partial is not ours.
        assert!(!sidecar.matches(&Identity {
            url: "https://example.com/v2/a.7z".to_string(),
            resume_key: Some("hash".to_string()),
        }));
        // Same URL, re-cut bytes: only the caller's key can tell.
        assert!(!sidecar.matches(&Identity {
            url: "https://example.com/a.7z".to_string(),
            resume_key: Some("other".to_string()),
        }));
        // A caller that stops supplying a key is not the same request either.
        assert!(!sidecar.matches(&Identity {
            url: "https://example.com/a.7z".to_string(),
            resume_key: None,
        }));
    }

    #[tokio::test]
    async fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bin.part.json");
        let sidecar = Sidecar {
            url: "https://example.com/x.bin".to_string(),
            etag: None,
            total: None,
            resume_key: None,
        };

        write(&path, &sidecar).await.unwrap();
        assert_eq!(read(&path).await, Some(sidecar));
    }

    #[tokio::test]
    async fn read_is_none_for_missing_and_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.json");
        assert_eq!(read(&missing).await, None);

        let garbage = dir.path().join("garbage.json");
        tokio::fs::write(&garbage, b"not json").await.unwrap();
        assert_eq!(read(&garbage).await, None);
    }

    #[tokio::test]
    async fn reconcile_keeps_a_matching_partial_and_clears_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let identity = Identity {
            url: "https://example.com/x.bin".to_string(),
            resume_key: None,
        };

        let seed = async |name: &str, sidecar: Option<&Sidecar>| {
            let part = dir.path().join(format!("{name}.part"));
            let side = dir.path().join(format!("{name}.part.json"));
            tokio::fs::write(&part, b"0123").await.unwrap();
            if let Some(sidecar) = sidecar {
                write(&side, sidecar).await.unwrap();
            }
            (part, side)
        };
        let ours = Sidecar {
            url: identity.url.clone(),
            etag: None,
            total: None,
            resume_key: None,
        };

        let (part, side) = seed("match", Some(&ours)).await;
        reconcile(&part, &side, &identity).await.unwrap();
        assert!(part.exists() && side.exists());

        let stale = Sidecar {
            url: "https://example.com/v2/x.bin".to_string(),
            ..ours.clone()
        };
        let (part, side) = seed("stale", Some(&stale)).await;
        reconcile(&part, &side, &identity).await.unwrap();
        assert!(!part.exists() && !side.exists());

        let (part, side) = seed("orphan", None).await;
        reconcile(&part, &side, &identity).await.unwrap();
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn promote_replaces_the_target_and_removes_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bin");
        let part = part_path(&path);
        let side = sidecar_path(&path);

        tokio::fs::write(&path, b"old").await.unwrap();
        tokio::fs::write(&part, b"new").await.unwrap();
        write(
            &side,
            &Sidecar {
                url: "https://example.com/x.bin".to_string(),
                etag: None,
                total: None,
                resume_key: None,
            },
        )
        .await
        .unwrap();

        promote(&part, &side, &path).await.unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"new");
        assert!(!part.exists());
        assert!(!side.exists());
    }
}
