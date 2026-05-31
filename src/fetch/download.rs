//! Streaming file downloads with live progress tracking.
//!
//! [`Fetch::download`](super::Fetch::download) streams a response body to disk in a background task and returns a
//! [`Download`] handle immediately. Callers read live progress through [`Download::progress`] (a [`Progress`]
//! snapshot), poll [`Download::completed`]/[`Download::failed`], await updates with [`Download::changed`], or await the
//! final [`Result`] with [`Download::join`]. Progress is shared over a [`tokio::sync::watch`] channel — the background
//! task is the single producer, the handle is the observer.

use std::fmt;
use std::path::PathBuf;
use std::pin::Pin;

use futures_util::StreamExt;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::watch;

use super::{PreparedRequest, retry};

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

/// Handle to an in-flight (or finished) download started by [`Fetch::download`](super::Fetch::download).
///
/// The download runs in a background task; this handle observes its progress and final result. Dropping the handle does
/// **not** cancel the download.
pub struct Download {
    rx: watch::Receiver<Progress>,
    handle: tokio::task::JoinHandle<Result<(), DownloadError>>,
}

impl Download {
    /// Assembles a handle from the watch receiver and the spawned task. Used by
    /// [`Fetch::download`](super::Fetch::download).
    pub(super) fn from_parts(
        rx: watch::Receiver<Progress>,
        handle: tokio::task::JoinHandle<Result<(), DownloadError>>,
    ) -> Self {
        Self { rx, handle }
    }

    /// Returns the latest [`Progress`] snapshot (a cheap clone of the watched value).
    pub fn progress(&self) -> Progress {
        self.rx.borrow().clone()
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
    /// [`DownloadError::Cancelled`]. Aborting mid-transfer leaves the partially written file on disk (no cleanup runs),
    /// so callers that cancel should remove it themselves.
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
async fn join_handle(
    handle: &mut tokio::task::JoinHandle<Result<(), DownloadError>>,
) -> Result<(), DownloadError> {
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
/// setup error (an invalid URL or a client-build failure) so it surfaces through the handle. On return, a final
/// [`Progress`] with `completed = true` (and `failed` reflecting the outcome) is sent.
pub(super) async fn run(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: watch::Sender<Progress>,
) -> Result<(), DownloadError> {
    let result = stream_to_file(prepared, path, &tx).await;
    tx.send_modify(|p| {
        p.completed = true;
        p.failed = result.is_err();
    });
    result
}

/// Streams the response body to `path`, retrying the whole transfer with Fibonacci backoff.
///
/// Each attempt truncates the file and resets the progress counters, so a retry restarts cleanly from byte zero.
async fn stream_to_file(
    prepared: Result<PreparedRequest, reqwest::Error>,
    path: PathBuf,
    tx: &watch::Sender<Progress>,
) -> Result<(), DownloadError> {
    let prepared = prepared?;

    retry::with_fibonacci_backoff(prepared.retries, || async {
        let mut file = BufWriter::new(tokio::fs::File::create(&path).await?);
        tx.send_replace(Progress::default());

        let response = prepared.request().send().await?.error_for_status()?;

        let total = response.content_length();
        let mut downloaded: u64 = 0;
        tx.send_replace(Progress {
            total,
            downloaded,
            progress: fraction(total, downloaded),
            completed: false,
            failed: false,
        });

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            tx.send_replace(Progress {
                total,
                downloaded,
                progress: fraction(total, downloaded),
                completed: false,
                failed: false,
            });
        }
        file.flush().await?;
        Ok::<(), DownloadError>(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_support::{read_request, write_response, write_response_no_length};
    use crate::fetch::{Fetch, RequestOptions};
    use tokio::net::TcpListener;

    /// A unique temp path for a test, keyed by the (unique) ephemeral port the test bound.
    fn temp_path(port: u16) -> PathBuf {
        std::env::temp_dir().join(format!("rust-sak-dl-{port}.bin"))
    }

    /// Drains progress updates until the background task drops its sender, then returns the final snapshot.
    async fn drain(download: &mut Download) -> Progress {
        while download.changed().await.is_ok() {}
        download.progress()
    }

    #[tokio::test]
    async fn download_writes_file() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "200 OK", "hello download").await;
        });

        let download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        download.join().await.unwrap();
        server.await.unwrap();

        let contents = tokio::fs::read(&path).await.unwrap();
        assert_eq!(contents, b"hello download");

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn download_reports_total_and_completes_to_full() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "200 OK", "0123456789").await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        let progress = drain(&mut download).await;
        server.await.unwrap();

        assert!(progress.completed);
        assert!(!progress.failed);
        assert_eq!(progress.total, Some(10));
        assert_eq!(progress.downloaded, 10);
        assert_eq!(progress.progress, Some(1.0));

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn download_without_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response_no_length(&mut stream, "no length body").await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        let progress = drain(&mut download).await;
        server.await.unwrap();

        assert!(progress.completed);
        assert!(!progress.failed);
        assert_eq!(progress.total, None);
        assert_eq!(progress.progress, None);

        let contents = tokio::fs::read(&path).await.unwrap();
        assert_eq!(contents, b"no length body");

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn download_failed_status_sets_failed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "500 Internal Server Error", "nope").await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        while download.changed().await.is_ok() {}
        assert!(download.completed());
        assert!(download.failed());

        let err = download.join().await.unwrap_err();
        assert!(matches!(err, DownloadError::Http(_)), "unexpected error: {err}");
        server.await.unwrap();

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn track_reports_progress_and_completes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "200 OK", "hello track").await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        let mut ticks: Vec<(Option<u64>, u64, Option<f64>)> = Vec::new();
        download
            .track(|total, downloaded, progress| ticks.push((total, downloaded, progress)))
            .await
            .unwrap();
        server.await.unwrap();

        assert!(!ticks.is_empty(), "callback was never invoked");
        // The final update reports the full transfer.
        assert_eq!(ticks.last().copied(), Some((Some(11), 11, Some(1.0))));

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn track_returns_error_on_failed_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "500 Internal Server Error", "nope").await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        let err = download.track(|_, _, _| {}).await.unwrap_err();
        assert!(matches!(err, DownloadError::Http(_)), "unexpected error: {err}");
        server.await.unwrap();

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn cancel_aborts_and_join_returns_cancelled() {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        // The server advertises a large body but sends only a few bytes, then holds the connection open — so the
        // download stays in-flight (it never errors and never completes) until we cancel it.
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\nhello")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            // Keep the stream alive so the body never finishes; the test aborts this task at the end.
            std::future::pending::<()>().await;
        });

        let mut download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new());
        // Wait for the first real progress update so the transfer is genuinely in-flight before cancelling.
        download.changed().await.unwrap();
        assert!(!download.completed());

        download.cancel();
        let err = download.join().await.unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled), "unexpected error: {err}");

        server.abort();
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn download_retries_until_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let path = temp_path(addr.port());

        let server = tokio::spawn(async move {
            // First attempt fails...
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "500 Internal Server Error", "fail").await;
            // ...the retry succeeds.
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            write_response(&mut stream, "200 OK", "recovered").await;
        });

        let download = Fetch::new().download(format!("http://{addr}"), &path, RequestOptions::new().retries(1));
        download.join().await.unwrap();
        server.await.unwrap();

        let contents = tokio::fs::read(&path).await.unwrap();
        assert_eq!(contents, b"recovered");

        let _ = tokio::fs::remove_file(&path).await;
    }
}
