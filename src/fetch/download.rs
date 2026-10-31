//! Streaming file downloads with live progress tracking.
//!
//! [`Fetch::download`](super::Fetch::download) streams a response body to disk in a background task and returns a
//! [`Download`] handle immediately. Callers read live progress through [`Download::progress`] (a [`Progress`]
//! snapshot), poll [`Download::completed`]/[`Download::failed`], await updates with [`Download::changed`], or await the
//! final [`Result`] with [`Download::join`]. Progress is shared over a [`tokio::sync::watch`] channel — the background
//! task is the single producer, the handle is the observer.
//!
//! Bytes never land on the target path directly: a transfer writes `<path>.part` alongside a `<path>.part.json`
//! sidecar (see [`partial`](super::partial)) and renames it into place only once it is complete. So a file at the
//! target path is complete by construction, and a partial one can be checked against what it claims to be before a
//! resume appends to it.
//!
//! A transfer can also hash its own bytes on the way past, via
//! [`RequestOptions::digest`](super::RequestOptions::digest); the result is read back from [`Download::digest`].
//!
//! [`DownloadMode`] decides what an existing file means: [`DownloadMode::Resume`] (the default) continues a matching
//! `<path>.part` via an HTTP `Range` request, [`DownloadMode::Overwrite`] discards any partial and re-downloads, and
//! [`DownloadMode::Skip`] reports an existing target file as complete without contacting the server.

use std::fmt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::watch;

use super::digest::{DigestAlgorithm, Hasher};
use super::{PreparedRequest, partial, retry};

/// How many bytes may arrive between progress updates before one is forced out.
///
/// A response body arrives in chunks of a few kilobytes, so sending an update per chunk means tens of thousands of
/// them across a large transfer, each waking every observer. Coalescing to a byte and a time bound keeps a progress
/// bar smooth while making the cost independent of how the body happens to be framed.
const PROGRESS_BYTES: u64 = 256 * 1024;

/// How long may pass between progress updates before one is forced out, so a slow transfer still ticks.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Controls what [`Fetch::download`](super::Fetch::download) does when a file already exists at the target path.
///
/// Set as a struct-wide default with [`Fetch::download_mode`](super::Fetch::download_mode) or per request with
/// [`RequestOptions::download_mode`](super::RequestOptions::download_mode); the per-request value takes priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadMode {
    /// Resume an incomplete transfer from its `<path>.part` via an HTTP `Range` request, appending the remaining
    /// bytes. Falls back to a full redownload if the partial does not belong to this request (see
    /// [`RequestOptions::resume_key`](super::RequestOptions::resume_key)) or if the server ignores `Range` (responds
    /// `200` instead of `206`). A file already at the target path is complete — nothing else is ever written there —
    /// so it is reported as such without contacting the server. This is the default.
    #[default]
    Resume,
    /// Always download from byte zero, discarding any existing partial and replacing whatever is at the target path.
    Overwrite,
    /// If a file already exists at the target path, do nothing and report the transfer complete without contacting the
    /// server. Any partial transfer in progress is resumed as under [`DownloadMode::Resume`], since only a completed
    /// transfer ever reaches the target path.
    Skip,
}

/// A snapshot of a download's progress, carried over the [`watch`] channel and returned by [`Download::progress`].
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// Total bytes expected, from the `Content-Length` header. `None` when the server did not advertise a length.
    pub total: Option<u64>,
    /// Bytes written to disk so far.
    pub downloaded: u64,
    /// Fraction complete in `0.0..=1.0`. `None` when [`total`](Progress::total) is unknown.
    pub progress: Option<f64>,
    /// `true` once the transfer has finished — on success **or** failure.
    pub completed: bool,
    /// `true` when the transfer finished with an error.
    pub failed: bool,
}

impl Progress {
    /// Builds an in-flight snapshot: `progress` is derived from `total`/`downloaded` via [`fraction`] and both terminal
    /// flags are `false`. The background task sets `completed`/`failed` once via [`watch::Sender::send_modify`] in
    /// [`run`] when the transfer ends.
    fn in_flight(total: Option<u64>, downloaded: u64) -> Self {
        Self {
            total,
            downloaded,
            progress: fraction(total, downloaded),
            completed: false,
            failed: false,
        }
    }
}

/// Handle to an in-flight (or finished) download started by [`Fetch::download`](super::Fetch::download).
///
/// The download runs in a background task; this handle observes its progress and final result. Dropping the handle does
/// **not** cancel the download.
#[derive(Debug)]
pub struct Download {
    rx: watch::Receiver<Progress>,
    /// Taken by whichever of [`Download::track`]/[`Download::join`] awaits it first: a finished task cannot be
    /// awaited twice, and `None` is what lets the second call answer instead of panicking.
    handle: Option<tokio::task::JoinHandle<Result<(), DownloadError>>>,
    /// Whether the awaited task succeeded, once it has been awaited. Kept here rather than read off [`Progress`],
    /// because a cancelled task never gets to mark itself failed.
    succeeded: Option<bool>,
    /// Set once by the background task when a transfer it actually ran completes successfully. Shared rather than
    /// carried in [`Progress`], because a digest only exists at the end and an in-flight snapshot has nothing to say
    /// about it.
    digest: Arc<OnceLock<String>>,
}

impl Download {
    /// Assembles a handle from the watch receiver and the spawned task. Used by
    /// [`Fetch::download`](super::Fetch::download).
    pub(super) fn from_parts(
        rx: watch::Receiver<Progress>,
        handle: tokio::task::JoinHandle<Result<(), DownloadError>>,
        digest: Arc<OnceLock<String>>,
    ) -> Self {
        Self {
            rx,
            handle: Some(handle),
            succeeded: None,
            digest,
        }
    }

    /// Returns the latest [`Progress`] snapshot (a cheap clone of the watched value).
    pub fn progress(&self) -> Progress {
        self.rx.borrow().clone()
    }

    /// The hex digest of the artifact, once the transfer has completed successfully.
    ///
    /// `None` until then, and `None` for the whole life of a download that did not ask for one via
    /// [`RequestOptions::digest`](super::RequestOptions::digest). It is also `None` after a
    /// [`DownloadMode::Resume`]/[`DownloadMode::Skip`] transfer that found a file already at the target path: it
    /// reports completion without contacting the server, so no bytes passed through the hasher. A caller that must
    /// verify hashes the file itself in that one case — see
    /// [`crypto::sha256_file`](crate::crypto::sha256_file).
    ///
    /// The digest covers the whole artifact even when the transfer resumed a partial: the bytes already on disk are
    /// hashed before the first appended chunk.
    pub fn digest(&self) -> Option<String> {
        self.digest.get().cloned()
    }

    /// `true` once the transfer has finished, whether it succeeded or failed.
    pub fn completed(&self) -> bool {
        self.rx.borrow().completed
    }

    /// `true` when the transfer finished with an error. Use [`Download::join`] to retrieve the error itself.
    pub fn failed(&self) -> bool {
        self.rx.borrow().failed
    }

    /// Waits for the next progress update.
    ///
    /// # Errors
    ///
    /// Returns an error once the background task has ended and dropped its sender (i.e. there will be no more updates);
    /// the last [`Progress`] remains readable via [`Download::progress`].
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.rx.changed().await
    }

    /// Invokes `callback` for every progress update until the download finishes, then returns its final result.
    /// Borrows the handle, so the [`Download`] remains usable afterward — e.g. to read the final
    /// [`progress`](Download::progress) snapshot.
    ///
    /// The callback receives `(total, downloaded, progress)` from each [`Progress`] update — the same fields as
    /// [`Progress::total`]/[`Progress::downloaded`]/[`Progress::progress`]. It is **not** called for the initial
    /// zero-valued snapshot (only for updates produced by the transfer) and it **is** called for the final update.
    ///
    /// Updates are **coalesced**, not one per received chunk: one goes out when the response headers land, then at
    /// most one per 256 KiB or per 100 ms, and always a last one carrying the final byte count. So the callback fires
    /// often enough to drive a progress bar without its rate depending on how the server happened to frame the body.
    ///
    /// The background task is awaited exactly once, and this method returns its final result. Calling it again, or
    /// [`join`](Download::join) afterward, does not panic — see [`join`](Download::join) for what a second call
    /// returns. Reading [`progress`](Download::progress)/[`completed`](Download::completed)/[`failed`](Download::failed)
    /// afterward is fine.
    ///
    /// # Errors
    ///
    /// Returns the [`DownloadError`] that ended the download — an HTTP/transport failure, a disk-write failure, or
    /// [`DownloadError::Cancelled`] if it was aborted via [`Download::cancel`].
    ///
    /// # Panics
    ///
    /// Panics if the background task panicked.
    pub async fn track<F>(&mut self, mut callback: F) -> Result<(), DownloadError>
    where
        F: FnMut(Option<u64>, u64, Option<f64>),
    {
        while self.rx.changed().await.is_ok() {
            let progress = self.rx.borrow_and_update().clone();
            callback(progress.total, progress.downloaded, progress.progress);
        }
        self.join_once().await
    }

    /// Cancels the download if it is still running; a no-op if it has already completed or errored.
    ///
    /// Aborts the background task. After cancelling, [`join`](Download::join)/[`track`](Download::track) return
    /// [`DownloadError::Cancelled`]. Aborting mid-transfer leaves `<path>.part` and its sidecar on disk (no cleanup
    /// runs) but **nothing at the target path**, so a later [`DownloadMode::Resume`] picks the transfer up where it
    /// stopped and a [`DownloadMode::Skip`] correctly declines to treat it as finished.
    pub fn cancel(&self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }

    /// Awaits completion and returns the download's final result. Consumes the handle.
    ///
    /// After [`track`](Download::track) has already returned the result, this returns `Ok(())` for a download that
    /// succeeded, and for one that failed or was cancelled a [`DownloadError::Io`] of kind
    /// [`std::io::ErrorKind::Other`] saying the real error was handed to `track` — a [`DownloadError`] is not
    /// [`Clone`], so it cannot be returned twice.
    ///
    /// # Errors
    ///
    /// Returns the [`DownloadError`] that ended the download — an HTTP/transport failure, a disk-write failure, or
    /// [`DownloadError::Cancelled`] if it was aborted via [`Download::cancel`].
    ///
    /// # Panics
    ///
    /// Panics if the background task panicked (a bug), as distinct from being cancelled.
    pub async fn join(mut self) -> Result<(), DownloadError> {
        self.join_once().await
    }

    /// Awaits the task the first time it is called, and reports the already-returned outcome every time after.
    async fn join_once(&mut self) -> Result<(), DownloadError> {
        match (self.handle.take(), self.succeeded) {
            (Some(mut handle), _) => {
                let result = join_handle(&mut handle).await;
                self.succeeded = Some(result.is_ok());
                result
            }
            (None, Some(false)) => Err(DownloadError::Io(std::io::Error::other(
                "the download did not succeed, and its error was already returned by an earlier track()",
            ))),
            (None, _) => Ok(()),
        }
    }
}

/// Awaits the download task, mapping a cancellation into [`DownloadError::Cancelled`] and re-raising a genuine task
/// panic. Shared by [`Download::join`] and [`Download::track`]. Drives the handle through a mutable borrow (a
/// [`JoinHandle`](tokio::task::JoinHandle) is [`Unpin`]) so `track` can keep the [`Download`] usable afterward.
async fn join_handle(handle: &mut tokio::task::JoinHandle<Result<(), DownloadError>>) -> Result<(), DownloadError> {
    match Pin::new(handle).await {
        Ok(result) => result,
        Err(err) if err.is_cancelled() => Err(DownloadError::Cancelled),
        Err(err) => std::panic::resume_unwind(err.into_panic()),
    }
}

/// What the caller said identifies a download's bytes: [`RequestOptions::resume_key`](super::RequestOptions::resume_key)
/// and [`RequestOptions::resume_across_urls`](super::RequestOptions::resume_across_urls).
#[derive(Debug, Clone, Default)]
pub(super) struct Resume {
    pub key: Option<String>,
    pub across_urls: bool,
}

/// An error from a streaming download: an HTTP/transport failure, a failure writing the file to disk, or cancellation.
#[derive(Debug)]
pub enum DownloadError {
    /// The request failed, returned an error status, or the response stream errored.
    Http(reqwest::Error),
    /// Writing the downloaded bytes to the disk failed.
    Io(std::io::Error),
    /// The download was cancelled via [`Download::cancel`] before it finished.
    Cancelled,
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::Http(err) => write!(f, "download request failed: {err}"),
            DownloadError::Io(err) => write!(f, "writing download to disk failed: {err}"),
            DownloadError::Cancelled => write!(f, "download was cancelled"),
        }
    }
}

impl std::error::Error for DownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DownloadError::Http(err) => Some(err),
            DownloadError::Io(err) => Some(err),
            DownloadError::Cancelled => None,
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        DownloadError::Http(err)
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        DownloadError::Io(err)
    }
}

/// Parses the first-byte position from a `206` response's `Content-Range` header (e.g. `bytes 1024-2047/4096` → `1024`).
///
/// Returns `None` when the header is absent, not valid UTF-8, or malformed — any of which makes the partial response
/// untrustworthy for a resume.
fn content_range_start(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    // RFC 9110 form: "bytes <start>-<end>/<complete-length|*>".
    let start = value.strip_prefix("bytes ")?.split('-').next()?;
    start.trim().parse::<u64>().ok()
}

/// Parses the complete length from a `416` response's `Content-Range` header (e.g. `bytes */4096` → `4096`).
///
/// Returns `None` when the header is absent or malformed, in which case the response says nothing about how long the
/// resource is.
fn unsatisfied_range_total(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    value.strip_prefix("bytes */")?.trim().parse::<u64>().ok()
}

/// Reports whether a failed attempt is worth another, for the download's retry loop.
///
/// HTTP failures follow the same rule as every other request ([`retry::is_transient`]). A disk failure that will
/// only fail again — no permission, no space, a path that is not a directory — is final. Any other I/O error is
/// retried; that includes the deliberate `InvalidData` raised after discarding a partial the server disowned, which is
/// how the next attempt starts over from byte zero.
fn is_retryable(err: &DownloadError) -> bool {
    use std::io::ErrorKind;

    match err {
        DownloadError::Http(err) => retry::is_transient(err),
        DownloadError::Io(err) => !matches!(
            err.kind(),
            ErrorKind::PermissionDenied
                | ErrorKind::NotFound
                | ErrorKind::ReadOnlyFilesystem
                | ErrorKind::StorageFull
                | ErrorKind::QuotaExceeded
                | ErrorKind::FileTooLarge
                | ErrorKind::IsADirectory
                | ErrorKind::NotADirectory
                | ErrorKind::InvalidInput
        ),
        DownloadError::Cancelled => false,
    }
}

/// Computes the completion fraction, clamped to `0.0..=1.0`. `None` when the total is unknown; a known total of zero
/// (an empty file) is reported as fully complete.
fn fraction(total: Option<u64>, downloaded: u64) -> Option<f64> {
    total.map(|t| {
        if t == 0 {
            1.0
        } else {
            (downloaded as f64 / t as f64).min(1.0)
        }
    })
}

/// Drives a download to completion, broadcasting progress over `tx` and returning the final result.
///
/// Called inside the background task spawned by [`Fetch::download`](super::Fetch::download). `prepared` carries any
/// setup error (an invalid URL or a client-build failure) so it surfaces through the handle. `mode` decides how an
/// existing file at `path` is handled, and `resume` is the caller's optional identity for the bytes. `algorithm`
/// opts the transfer into hashing its own bytes, and the result is published through `digest`. On return, a final
/// [`Progress`] with `completed = true` (and `failed` reflecting the outcome) is sent.
pub(super) async fn run(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: watch::Sender<Progress>,
    mode: DownloadMode,
    resume: Resume,
    algorithm: Option<DigestAlgorithm>,
    digest_out: Arc<OnceLock<String>>,
) -> Result<(), DownloadError> {
    let result = stream_to_file(prepared, path, &tx, mode, resume, algorithm).await;

    // The single place a digest is published, so it cannot be set from a path that did not finish the transfer.
    if let Ok(Some(digest)) = &result {
        let _ = digest_out.set(digest.clone());
    }

    tx.send_modify(|p| {
        p.completed = true;
        p.failed = result.is_err();
    });

    result.map(|_| ())
}

/// Returns the size of the file at `path`, or `0` when it does not exist.
///
/// Only a `NotFound` error maps to `0` (the "start fresh" case). Any other stat failure — e.g. a permission error —
/// is surfaced rather than silently treated as an empty file, which would otherwise let `Resume` truncate an existing
/// but un-stat'able file.
async fn file_len(path: &Path) -> std::io::Result<u64> {
    Ok(existing_len(path).await?.unwrap_or(0))
}

/// Returns the size of the file at `path`, or `None` when there is no file there.
///
/// One `metadata` call answers both "is it there" and "how big is it". Asking `try_exists` and then `file_len`
/// stats the same inode twice, and each stat is its own hop onto tokio's blocking pool.
///
/// Only a `NotFound` error maps to `None`; any other stat failure is surfaced, for the reason [`file_len`] gives.
async fn existing_len(path: &Path) -> std::io::Result<Option<u64>> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => Ok(Some(meta.len())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Brings `checkpoint` up to the first `offset` bytes of the partial at `part`, reading back only the gap it does
/// not already cover, and hands back the hash to continue with.
///
/// The carried hash is taken only when it covers no more than `offset`: a checkpoint claiming *more* bytes than the
/// partial now holds describes an artifact that is no longer on disk, and there is no way to un-hash the excess, so
/// that case starts over.
async fn hash_prefix(
    checkpoint: &Mutex<Option<(Hasher, u64)>>,
    algorithm: DigestAlgorithm,
    part: &Path,
    offset: u64,
) -> Result<Hasher, DownloadError> {
    // A `Mutex` rather than a `RefCell` only because the transfer runs in a spawned task, which has to be
    // `Send`. It is never contended, and the guard is never held across an `await`.
    let carried = checkpoint
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .filter(|(_, hashed)| *hashed <= offset);
    let (mut hasher, hashed) = carried.unwrap_or_else(|| (Hasher::new(algorithm), 0));

    if hashed < offset {
        hasher.update_range(part, hashed, offset).await?;
    }

    Ok(hasher)
}

/// Drops the carried hash, for every branch that truncates the partial it describes.
///
/// A checkpoint covering bytes that were just thrown away would otherwise be picked up by the next resume and folded
/// into a digest of different content.
fn forget(checkpoint: &Mutex<Option<(Hasher, u64)>>) {
    *checkpoint.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Reads a response's `ETag`, if it carried a **strong** one that is valid UTF-8.
///
/// Only a strong validator may be sent back as `If-Range` (RFC 9110 §13.1.5); a server receiving a weak one ignores
/// the `Range` and answers `200`, so every resume would quietly restart from zero.
fn response_etag(response: &reqwest::Response) -> Option<String> {
    let etag = response.headers().get(ETAG)?.to_str().ok()?;
    (!etag.starts_with("W/")).then(|| etag.to_owned())
}

/// Reads a response's `Last-Modified`, if it carried one that is valid UTF-8.
fn response_last_modified(response: &reqwest::Response) -> Option<String> {
    response.headers().get(LAST_MODIFIED)?.to_str().ok().map(str::to_owned)
}

/// Streams the response body to `<path>.part`, retrying the whole transfer with Fibonacci backoff, and renames it onto
/// `path` once it is complete.
///
/// The behavior when something already exists is governed by `mode`:
/// - Under [`DownloadMode::Resume`] and [`DownloadMode::Skip`], a file at `path` is complete (only the final rename
///   ever puts one there) and is reported as such without a request.
/// - Those two modes otherwise resume `<path>.part`, but only after its sidecar confirms it belongs to this request; a
///   mismatched or unidentifiable partial is deleted instead of appended to.
/// - [`DownloadMode::Overwrite`] discards any partial and downloads from byte zero.
///
/// The offset is re-read from disk at the start of each attempt, so a retry resumes from whatever bytes are already
/// present rather than restarting.
async fn stream_to_file(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: &watch::Sender<Progress>,
    mode: DownloadMode,
    resume: Resume,
    algorithm: Option<DigestAlgorithm>,
) -> Result<Option<String>, DownloadError> {
    let prepared = prepared?;
    let part = partial::part_path(&path);
    let sidecar = partial::sidecar_path(&path);

    // Nothing but the final rename ever writes to `path`, so a file there is finished. Both modes that respect an
    // existing file stop here — which is what makes `Skip` sound: it can no longer mistake a truncated transfer for a
    // complete download.
    if mode != DownloadMode::Overwrite
        && let Some(len) = existing_len(&path).await?
    {
        tx.send_replace(Progress::in_flight(Some(len), len));
        return Ok(None);
    }

    let identity = partial::Identity {
        url: prepared.effective_url(),
        resume_key: resume.key,
        across_urls: resume.across_urls,
    };

    // The resume decision is made once, up front: whether these bytes are ours is knowable before the first byte is
    // appended, and finding out afterwards (from a hash that fails) costs a whole transfer.
    if mode == DownloadMode::Overwrite {
        partial::discard(&part, &sidecar).await?;
    } else {
        partial::reconcile(&part, &sidecar, &identity).await?;
    }

    // The hash of the artifact prefix that is known to be on disk, carried from one attempt to the next.
    //
    // Building it fresh per attempt means re-reading and re-hashing the *whole* partial every time a transfer
    // drops — for a 500 MB download that fails twice, close to a gigabyte of reads and SHA-256 to reproduce a
    // hash the previous attempt had already computed. It is written only immediately after a flush, so the byte
    // count it claims is never more than the next attempt will find on disk, and the gap it does not cover is
    // bounded by one progress interval rather than by the size of the file.
    let checkpoint: Mutex<Option<(Hasher, u64)>> = Mutex::new(None);

    // What a `429`/`503` asked the next attempt to wait, read while the response is still in hand.
    let retry_after = retry::RetryAfter::default();

    retry::with_fibonacci_backoff(prepared.retries, is_retryable, &retry_after, || async {
        // The offset is re-read each attempt, so a retry resumes from whatever is already on disk.
        let offset = match mode {
            DownloadMode::Overwrite => 0,
            DownloadMode::Resume | DownloadMode::Skip => file_len(&part).await?,
        };
        // Re-read rather than cached across attempts, so an `ETag` an earlier attempt recorded reaches this one.
        let recorded = if offset > 0 {
            partial::read(&sidecar).await
        } else {
            None
        };

        let mut builder = prepared.request();
        if offset > 0 {
            builder = builder.header(RANGE, format!("bytes={offset}-"));
            // The server-side half of the identity check, and the only one that can see bytes changing at a URL that
            // did not: a resource that no longer matches the recorded validator answers `200` rather than `206`, which
            // the fresh-download branch below already handles as a restart. A strong `ETag` is preferred; the
            // `Last-Modified` date is the validator `If-Range` accepts when there is none.
            let validator = recorded
                .as_ref()
                .and_then(|sidecar| sidecar.etag.clone().or_else(|| sidecar.last_modified.clone()));
            if let Some(validator) = validator {
                builder = builder.header(IF_RANGE, validator);
            }
        }
        let response = builder.send().await?;
        retry_after.record(&response);

        // A `416` to a ranged request means the offset is at (or past) the end — which makes the partial the whole
        // file only if the resource is exactly that long. One that has shrunk, or been replaced by something shorter,
        // also answers `416`, so the length the response reports (or the sidecar recorded) has to agree first. A
        // partial that disagrees is discarded, and the error sends the next attempt back to byte zero. When neither
        // says how long the resource is, there is nothing to check against, and the partial is taken as complete.
        if offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            let total = unsatisfied_range_total(&response).or(recorded.as_ref().and_then(|sidecar| sidecar.total));
            if total.is_some_and(|total| total != offset) {
                partial::discard(&part, &sidecar).await?;
                forget(&checkpoint);
                return Err(DownloadError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "server answered 416 for a partial whose length does not match the resource",
                )));
            }

            // The partial already is the whole artifact, so no byte of it streams past the hasher on this attempt:
            // whatever the checkpoint does not already cover is read back before the promotion moves it.
            let digest = match algorithm {
                Some(algorithm) => Some(hash_prefix(&checkpoint, algorithm, &part, offset).await?.finish()),
                None => None,
            };

            partial::promote(&part, &sidecar, &path).await?;
            tx.send_replace(Progress::in_flight(Some(offset), offset));
            return Ok(digest);
        }

        let response = response.error_for_status()?;
        let resuming = offset > 0 && response.status() == StatusCode::PARTIAL_CONTENT;

        // A `206` is only a trustworthy resume if its `Content-Range` begins exactly at the byte we asked for. A
        // server that ignored `Range` answers `200` (handled as a fresh download below); one that returns a `206` for
        // some *other* range would corrupt the file if we blindly appended its body. Reject it: discard the partial
        // file so the next attempt re-reads a zero offset and re-requests without `Range`, and error out so the retry
        // (or a final failure) kicks in rather than writing bad bytes.
        if resuming && content_range_start(&response) != Some(offset) {
            tokio::fs::File::create(&part).await?;
            forget(&checkpoint);
            return Err(DownloadError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "server returned a 206 whose Content-Range does not match the requested offset",
            )));
        }

        // Open the file only after a good response, so a failed attempt never leaves a stray empty file (which would
        // corrupt the next attempt's offset).
        let (mut file, mut downloaded, total) = if resuming {
            // A `206` `Content-Length` reports the remaining bytes, so the total is `offset + remaining`.
            let total = response.content_length().map(|remaining| offset + remaining);
            let handle = tokio::fs::OpenOptions::new().append(true).open(&part).await?;
            (BufWriter::new(handle), offset, total)
        } else {
            // A `200` (fresh download, or a server that ignored `Range`): truncate and start from byte zero.
            forget(&checkpoint);
            (
                BufWriter::new(tokio::fs::File::create(&part).await?),
                0,
                response.content_length(),
            )
        };

        // Keyed off `resuming`, not the offset: it is the response that decides whether the bytes already on disk are
        // part of this artifact, since a server that answered `200` just had them truncated away — which is also why
        // a carried checkpoint is only reachable through the resuming branch.
        let mut hasher = match algorithm {
            Some(algorithm) if resuming => Some(hash_prefix(&checkpoint, algorithm, &part, offset).await?),
            Some(algorithm) => Some(Hasher::new(algorithm)),
            None => None,
        };

        // A `206` need not repeat the `ETag`, so the recorded one carries forward; a `200` replaced the bytes, so only
        // what this response said still applies.
        let (etag, last_modified) = match recorded {
            Some(recorded) if resuming => (
                response_etag(&response).or(recorded.etag),
                response_last_modified(&response).or(recorded.last_modified),
            ),
            _ => (response_etag(&response), response_last_modified(&response)),
        };

        // Written as soon as the response says what the `.part` will hold, so an interruption one chunk later already
        // has something to validate the partial against.
        partial::write(
            &sidecar,
            &partial::Sidecar {
                url: identity.url.clone(),
                etag,
                last_modified,
                total,
                resume_key: identity.resume_key.clone(),
            },
        )
        .await?;

        // Unconditional, so `total` is published as soon as it is known rather than waiting for the first threshold.
        tx.send_replace(Progress::in_flight(total, downloaded));

        let mut stream = response.bytes_stream();
        let mut reported = downloaded;
        let mut reported_at = Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            if let Some(hasher) = hasher.as_mut() {
                hasher.update(&chunk);
            }
            downloaded += chunk.len() as u64;

            if downloaded - reported >= PROGRESS_BYTES || reported_at.elapsed() >= PROGRESS_INTERVAL {
                tx.send_replace(Progress::in_flight(total, downloaded));
                reported = downloaded;
                reported_at = Instant::now();

                // Flushed *before* the hash is recorded, so the checkpoint describes bytes a retry will actually
                // find in the partial. This is what makes the checkpoint usable rather than what makes it safe:
                // recording it first would leave it covering bytes still in the writer's buffer, `hash_prefix`
                // would reject it as covering more than the partial holds, and the attempt would fall back to
                // re-reading the whole prefix — correct, but exactly the work this exists to avoid.
                if let Some(hasher) = hasher.as_ref() {
                    file.flush().await?;
                    *checkpoint.lock().unwrap_or_else(PoisonError::into_inner) = Some((hasher.clone(), downloaded));
                }
            }
        }

        // Synced before the rename, so a crash cannot leave a complete-looking file whose tail never reached the disk.
        file.flush().await?;
        file.into_inner().sync_all().await?;
        partial::promote(&part, &sidecar, &path).await?;

        // Returned rather than written through a handle, so "a digest exists only for a transfer that finished" is
        // a property of the type instead of one of where the statement happens to sit.
        let digest = hasher.map(Hasher::finish);

        // The final count always goes out, whatever the thresholds said, so an observer's last update matches what is
        // actually on disk — `track` in particular relies on seeing it.
        if downloaded != reported {
            tx.send_replace(Progress::in_flight(total, downloaded));
        }

        Ok::<Option<String>, DownloadError>(digest)
    })
    .await
}
