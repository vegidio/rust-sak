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
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, ETAG, IF_RANGE, RANGE};
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
    handle: tokio::task::JoinHandle<Result<(), DownloadError>>,
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
        Self { rx, handle, digest }
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
    /// The background task is awaited exactly once; this method returns its final result, so do **not** call
    /// [`join`](Download::join) afterward (it would re-await a finished task and panic). Reading
    /// [`progress`](Download::progress)/[`completed`](Download::completed)/[`failed`](Download::failed) afterward is
    /// fine.
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
        join_handle(&mut self.handle).await
    }

    /// Cancels the download if it is still running; a no-op if it has already completed or errored.
    ///
    /// Aborts the background task. After cancelling, [`join`](Download::join)/[`track`](Download::track) return
    /// [`DownloadError::Cancelled`]. Aborting mid-transfer leaves `<path>.part` and its sidecar on disk (no cleanup
    /// runs) but **nothing at the target path**, so a later [`DownloadMode::Resume`] picks the transfer up where it
    /// stopped and a [`DownloadMode::Skip`] correctly declines to treat it as finished.
    pub fn cancel(&self) {
        self.handle.abort();
    }

    /// Awaits completion and returns the download's final result. Consumes the handle.
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
        join_handle(&mut self.handle).await
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
/// existing file at `path` is handled, and `resume_key` is the caller's optional identity for the bytes. `algorithm`
/// opts the transfer into hashing its own bytes, and the result is published through `digest`. On return, a final
/// [`Progress`] with `completed = true` (and `failed` reflecting the outcome) is sent.
pub(super) async fn run(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: watch::Sender<Progress>,
    mode: DownloadMode,
    resume_key: Option<String>,
    algorithm: Option<DigestAlgorithm>,
    digest: Arc<OnceLock<String>>,
) -> Result<(), DownloadError> {
    let result = stream_to_file(prepared, path, &tx, mode, resume_key, algorithm, &digest).await;
    tx.send_modify(|p| {
        p.completed = true;
        p.failed = result.is_err();
    });
    result
}

/// Returns the size of the file at `path`, or `0` when it does not exist.
///
/// Only a `NotFound` error maps to `0` (the "start fresh" case). Any other stat failure — e.g. a permission error —
/// is surfaced rather than silently treated as an empty file, which would otherwise let `Resume` truncate an existing
/// but un-stat'able file.
async fn file_len(path: &Path) -> std::io::Result<u64> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => Ok(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err),
    }
}

/// Reads a response's `ETag`, if it carried one that is valid UTF-8.
fn response_etag(response: &reqwest::Response) -> Option<String> {
    response.headers().get(ETAG)?.to_str().ok().map(str::to_owned)
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
#[allow(clippy::too_many_arguments)]
async fn stream_to_file(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: &watch::Sender<Progress>,
    mode: DownloadMode,
    resume_key: Option<String>,
    algorithm: Option<DigestAlgorithm>,
    digest: &OnceLock<String>,
) -> Result<(), DownloadError> {
    let prepared = prepared?;
    let part = partial::part_path(&path);
    let sidecar = partial::sidecar_path(&path);

    // Nothing but the final rename ever writes to `path`, so a file there is finished. Both modes that respect an
    // existing file stop here — which is what makes `Skip` sound: it can no longer mistake a truncated transfer for a
    // complete download.
    if mode != DownloadMode::Overwrite && tokio::fs::try_exists(&path).await? {
        let len = file_len(&path).await?;
        tx.send_replace(Progress::in_flight(Some(len), len));
        return Ok(());
    }

    let identity = partial::Identity {
        url: prepared.effective_url(),
        resume_key,
    };

    // The resume decision is made once, up front: whether these bytes are ours is knowable before the first byte is
    // appended, and finding out afterwards (from a hash that fails) costs a whole transfer.
    if mode == DownloadMode::Overwrite {
        partial::discard(&part, &sidecar).await?;
    } else {
        partial::reconcile(&part, &sidecar, &identity).await?;
    }

    retry::with_fibonacci_backoff(prepared.retries, || async {
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
            // did not: a resource that no longer matches the recorded `ETag` answers `200` rather than `206`, which
            // the fresh-download branch below already handles as a restart.
            if let Some(etag) = recorded.as_ref().and_then(|sidecar| sidecar.etag.clone()) {
                builder = builder.header(IF_RANGE, etag);
            }
        }
        let response = builder.send().await?;

        // A `416` to a ranged request means the offset is at (or past) the end: the partial is the whole file, so it
        // is promoted rather than transferred again.
        if offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            // The partial already is the whole artifact, so no byte of it streams past the hasher on this attempt:
            // what is on disk is read back before the promotion moves it.
            if let Some(algorithm) = algorithm {
                let mut hasher = Hasher::new(algorithm);
                hasher.update_prefix(&part, offset).await?;
                let _ = digest.set(hasher.finish());
            }
            partial::promote(&part, &sidecar, &path).await?;
            tx.send_replace(Progress::in_flight(Some(offset), offset));
            return Ok(());
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
            (
                BufWriter::new(tokio::fs::File::create(&part).await?),
                0,
                response.content_length(),
            )
        };

        // Keyed off `resuming`, not the offset: it is the response that decides whether the bytes already on disk are
        // part of this artifact, since a server that answered `200` just had them truncated away.
        let mut hasher = match algorithm {
            Some(algorithm) => {
                let mut hasher = Hasher::new(algorithm);
                if resuming {
                    hasher.update_prefix(&part, offset).await?;
                }
                Some(hasher)
            }
            None => None,
        };

        // A `206` need not repeat the `ETag`, so the recorded one carries forward; a `200` replaced the bytes, so only
        // what this response said still applies.
        let etag = match response_etag(&response) {
            Some(etag) => Some(etag),
            None if resuming => recorded.and_then(|sidecar| sidecar.etag),
            None => None,
        };

        // Written as soon as the response says what the `.part` will hold, so an interruption one chunk later already
        // has something to validate the partial against.
        partial::write(
            &sidecar,
            &partial::Sidecar {
                url: identity.url.clone(),
                etag,
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
            }
        }

        // Synced before the rename, so a crash cannot leave a complete-looking file whose tail never reached the disk.
        file.flush().await?;
        file.into_inner().sync_all().await?;
        partial::promote(&part, &sidecar, &path).await?;

        // Published only once the bytes are at the target path, so a digest is never readable for a transfer that did
        // not finish.
        if let Some(hasher) = hasher {
            let _ = digest.set(hasher.finish());
        }

        // The final count always goes out, whatever the thresholds said, so an observer's last update matches what is
        // actually on disk — `track` in particular relies on seeing it.
        if downloaded != reported {
            tx.send_replace(Progress::in_flight(total, downloaded));
        }

        Ok::<(), DownloadError>(())
    })
    .await
}
