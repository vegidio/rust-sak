use super::*;

#[test]
fn new_has_defaults() {
    let fetch = Fetch::new();
    assert!(fetch.headers.is_empty());
    assert_eq!(fetch.retries, 0);
    assert!(!fetch.disable_http2);
}

#[test]
fn builder_sets_fields() {
    let fetch = Fetch::new()
        .header("Accept", "application/json")
        .retries(3)
        .disable_http2(true);

    assert_eq!(fetch.headers.get("Accept").unwrap(), "application/json");
    assert_eq!(fetch.retries, 3);
    assert!(fetch.disable_http2);
}

#[test]
fn disable_http2_can_re_enable() {
    assert!(!Fetch::new().disable_http2(false).disable_http2);
    assert!(Fetch::new().disable_http2(false).disable_http2(true).disable_http2);
    assert!(!Fetch::new().disable_http2(true).disable_http2(false).disable_http2);
}

#[test]
fn headers_replaces_map() {
    let mut map = HeaderMap::new();
    map.insert("X-Test", "1".parse().unwrap());

    let fetch = Fetch::new().headers(map);
    assert_eq!(fetch.headers.get("X-Test").unwrap(), "1");
}

// --- text tests ---
//
// These point `text` at the throwaway local HTTP/1.1 server in `super::test_support`, so they exercise the
// real request path without reaching the network.

use super::test_support::{
    read_request, write_partial_response, write_range_not_satisfiable, write_response, write_response_in_chunks,
    write_response_no_length,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

#[tokio::test]
async fn text_returns_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "hello world").await;
    });

    let body = Fetch::new().text(format!("http://{addr}")).await.unwrap();

    assert_eq!(body, "hello world");
    server.await.unwrap();
}

#[tokio::test]
async fn text_sends_configured_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "ok").await;
        request
    });

    let body = Fetch::new()
        .header("X-Custom", "abc123")
        .text(format!("http://{addr}"))
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap();
    assert!(
        request.to_lowercase().contains("x-custom: abc123"),
        "request was:\n{request}"
    );
}

#[tokio::test]
async fn text_errors_on_failure_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "500 Internal Server Error", "nope").await;
    });

    let err = Fetch::new().text(format!("http://{addr}")).await.unwrap_err();

    assert_eq!(err.status(), Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
    server.await.unwrap();
}

#[tokio::test]
async fn text_retries_until_success() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

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

    let body = Fetch::new().retries(1).text(format!("http://{addr}")).await.unwrap();

    assert_eq!(body, "recovered");
    server.await.unwrap();
}

#[tokio::test]
async fn text_appends_query_params() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "ok").await;
        request
    });

    let body = Fetch::new()
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new().query("a", "1").query("b", "2"),
        )
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap();
    let request_line = request.lines().next().unwrap_or_default();
    assert!(request_line.contains("?a=1&b=2"), "request line was:\n{request_line}");
}

#[tokio::test]
async fn text_uses_per_request_method() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "ok").await;
        request
    });

    let body = Fetch::new()
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new().method(reqwest::Method::POST),
        )
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap();
    assert!(request.starts_with("POST "), "request was:\n{request}");
}

#[tokio::test]
async fn text_request_header_overrides_struct_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "ok").await;
        request
    });

    let body = Fetch::new()
        .header("X-Custom", "from-fetch")
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new().header("X-Custom", "from-request"),
        )
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap().to_lowercase();
    assert!(request.contains("x-custom: from-request"), "request was:\n{request}");
    assert!(!request.contains("from-fetch"), "request was:\n{request}");
}

#[tokio::test]
async fn text_merges_struct_and_request_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "ok").await;
        request
    });

    let body = Fetch::new()
        .header("X-From-Fetch", "fetch")
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new().header("X-From-Request", "request"),
        )
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap().to_lowercase();
    assert!(request.contains("x-from-fetch: fetch"), "request was:\n{request}");
    assert!(request.contains("x-from-request: request"), "request was:\n{request}");
}

#[tokio::test]
async fn text_per_request_retries_override() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First attempt fails...
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "500 Internal Server Error", "fail").await;
        // ...the per-request retry succeeds.
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "recovered").await;
    });

    // The struct disables retries; the per-request override re-enables one.
    let body = Fetch::new()
        .retries(0)
        .text_with_options(format!("http://{addr}"), RequestOptions::new().retries(1))
        .await
        .unwrap();

    assert_eq!(body, "recovered");
    server.await.unwrap();
}

#[tokio::test]
async fn post_is_not_retried_by_default() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // The server only ever answers one request, with a failure. If retries were attempted the client would
    // hang waiting for a second response; instead the single failure must surface immediately.
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "500 Internal Server Error", "fail").await;
    });

    let err = Fetch::new()
        .retries(3)
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new().method(reqwest::Method::POST),
        )
        .await
        .unwrap_err();

    assert_eq!(err.status(), Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
    server.await.unwrap();
}

#[tokio::test]
async fn post_is_retried_when_opted_in() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First attempt fails...
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "500 Internal Server Error", "fail").await;
        // ...the opted-in retry succeeds.
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "recovered").await;
    });

    let body = Fetch::new()
        .retries(1)
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new()
                .method(reqwest::Method::POST)
                .retry_non_idempotent(true),
        )
        .await
        .unwrap();

    assert_eq!(body, "recovered");
    server.await.unwrap();
}

#[tokio::test]
async fn read_timeout_errors_on_idle_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // The server accepts and reads the request but never sends a response, leaving the connection idle.
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let err = Fetch::new()
        .read_timeout(Duration::from_millis(200))
        .text(format!("http://{addr}"))
        .await
        .unwrap_err();

    assert!(err.is_timeout(), "expected a timeout error, got: {err}");
}

#[tokio::test]
async fn json_deserializes_body() {
    #[derive(serde::Deserialize)]
    struct Repo {
        name: String,
        stars: u32,
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", r#"{"name":"rust-sak","stars":42}"#).await;
    });

    let repo: Repo = Fetch::new().json(format!("http://{addr}")).await.unwrap();

    assert_eq!(repo.name, "rust-sak");
    assert_eq!(repo.stars, 42);
    server.await.unwrap();
}

#[tokio::test]
async fn json_errors_on_invalid_json() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "not json").await;
    });

    let result = Fetch::new().json::<serde_json::Value>(format!("http://{addr}")).await;

    assert!(result.is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn text_sends_request_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Read the full request including the body that follows the headers.
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            // Once the headers are in, read whatever body bytes arrived with them.
            if n < chunk.len() || buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        write_response(&mut stream, "200 OK", "ok").await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let body = Fetch::new()
        .text_with_options(
            format!("http://{addr}"),
            RequestOptions::new()
                .method(reqwest::Method::POST)
                .body(serde_json::json!({ "name": "rust" })),
        )
        .await
        .unwrap();

    assert_eq!(body, "ok");
    let request = server.await.unwrap();
    assert!(
        request.to_lowercase().contains("content-type: application/json"),
        "request was:\n{request}"
    );
    assert!(request.contains(r#"{"name":"rust"}"#), "request was:\n{request}");
}

// --- download tests ---
//
// Like the `text` tests, these point `download` at the throwaway local HTTP/1.1 server in `super::test_support`.

/// A unique temp path for a test, keyed by the (unique) ephemeral port the test bound.
fn temp_path(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("rust-sak-dl-{port}.bin"))
}

/// Drains progress updates until the background task drops its sender, then returns the final snapshot.
async fn drain(download: &mut Download) -> Progress {
    while download.changed().await.is_ok() {}
    download.progress()
}

/// Removes a test's target file and any partial bookkeeping left beside it.
async fn clean(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
    let _ = tokio::fs::remove_file(partial::part_path(path)).await;
    let _ = tokio::fs::remove_file(partial::sidecar_path(path)).await;
}

/// Seeds an interrupted transfer: `bytes` in `<path>.part`, plus the sidecar identifying them.
///
/// The recorded URL is normalized through [`reqwest::Url`] because that is what the transfer compares against — the
/// sidecar has to hold the URL the request actually resolves to, not the string a caller happened to type.
async fn seed_partial(path: &Path, url: &str, bytes: &[u8], etag: Option<&str>, resume_key: Option<&str>) {
    clean(path).await;
    tokio::fs::write(partial::part_path(path), bytes).await.unwrap();
    partial::write(
        &partial::sidecar_path(path),
        &partial::Sidecar {
            url: reqwest::Url::parse(url).unwrap().to_string(),
            etag: etag.map(str::to_owned),
            total: None,
            resume_key: resume_key.map(str::to_owned),
        },
    )
    .await
    .unwrap();
}

/// Asserts that a finished transfer left the target file with `contents` and no bookkeeping behind.
async fn assert_settled(path: &Path, contents: &[u8]) {
    assert_eq!(tokio::fs::read(path).await.unwrap(), contents);
    assert!(!partial::part_path(path).exists(), "a finished transfer left a .part");
    assert!(
        !partial::sidecar_path(path).exists(),
        "a finished transfer left a sidecar"
    );
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

    let download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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
async fn download_coalesces_progress_updates_but_reports_the_exact_total() {
    // A body far larger than the 256 KiB threshold, delivered in small writes. Every update must still add up: the
    // last one carries the exact byte count even though the intermediate ones were coalesced away.
    const BODY_LEN: usize = 1 << 20;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // 4 KiB at a time: ~256 sends, so an un-coalesced implementation would emit ~256 progress updates.
        write_response_in_chunks(&mut stream, BODY_LEN, 4096).await;
    });

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);

    let mut updates = 0_usize;
    let mut last = 0_u64;
    download
        .track(|_, downloaded, _| {
            updates += 1;
            // Progress never goes backwards.
            assert!(downloaded >= last, "{downloaded} < {last}");
            last = downloaded;
        })
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(
        last, BODY_LEN as u64,
        "the final update must carry the exact byte count"
    );
    assert_eq!(
        tokio::fs::metadata(&path).await.unwrap().len(),
        BODY_LEN as u64,
        "the file on disk must match what was reported"
    );
    // The point of the coalescing: far fewer updates than the ~256 chunks the body arrived in.
    assert!(updates <= 32, "expected coalesced updates, got {updates}");

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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
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

    let download =
        Fetch::new().download_with_options(format!("http://{addr}"), &path, RequestOptions::new().retries(1));
    download.join().await.unwrap();
    server.await.unwrap();

    let contents = tokio::fs::read(&path).await.unwrap();
    assert_eq!(contents, b"recovered");

    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn resume_appends_from_offset() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    // An interrupted transfer: the first 4 bytes of "0123456789", recorded against this URL.
    seed_partial(&path, &url, b"0123", None, None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.contains("bytes=4-"),
            "expected a resume Range header, got: {request}"
        );
        write_partial_response(&mut stream, 4, 10, "456789").await;
    });

    let mut download = Fetch::new().download(&url, &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(progress.completed);
    assert!(!progress.failed);
    assert_eq!(progress.total, Some(10));
    assert_eq!(progress.downloaded, 10);

    assert_settled(&path, b"0123456789").await;
    clean(&path).await;
}

#[tokio::test]
async fn resume_falls_back_when_server_ignores_range() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    // A partial that is ours by identity; the server ignores Range and replies with a full 200 body.
    seed_partial(&path, &url, b"stale", None, None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "full body").await;
    });

    let mut download = Fetch::new().download(&url, &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(progress.completed);
    assert!(!progress.failed);

    // The stale partial was truncated, not appended to.
    assert_settled(&path, b"full body").await;
    clean(&path).await;
}

#[tokio::test]
async fn resume_rejects_206_with_mismatched_content_range() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    // A 4-byte partial; the client will request `bytes=4-`.
    seed_partial(&path, &url, b"0123", None, None).await;

    let server = tokio::spawn(async move {
        // First attempt: a 206 that lies about its range — it claims to start at byte 0, not the requested 4.
        // Appending its body would corrupt the file, so the transfer must reject it and restart from scratch.
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.contains("bytes=4-"),
            "expected a resume Range header, got: {request}"
        );
        write_partial_response(&mut stream, 0, 10, "BADBADBAD!").await;
        // Retry: the partial was discarded, so no Range header is sent and the server returns the full body.
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            !request.to_lowercase().contains("range:"),
            "after a rejected 206 the retry must restart without a Range header, got: {request}"
        );
        write_response(&mut stream, "200 OK", "0123456789").await;
    });

    let mut download = Fetch::new().retries(1).download(&url, &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(progress.completed);
    assert!(!progress.failed);
    assert_eq!(progress.downloaded, 10);

    // The mismatched partial body was never appended; the retry produced the correct full file.
    assert_settled(&path, b"0123456789").await;
    clean(&path).await;
}

#[tokio::test]
async fn skip_when_file_exists() {
    // Bind a listener to reserve a port but never accept: Skip must not make a request.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    clean(&path).await;
    tokio::fs::write(&path, b"existing").await.unwrap();

    let mut download = Fetch::new().download_with_options(
        format!("http://{addr}"),
        &path,
        RequestOptions::new().download_mode(DownloadMode::Skip),
    );
    let progress = drain(&mut download).await;

    assert!(progress.completed);
    assert!(!progress.failed);
    assert_eq!(progress.downloaded, 8);
    assert_eq!(progress.total, Some(8));

    // The existing file is untouched.
    let contents = tokio::fs::read(&path).await.unwrap();
    assert_eq!(contents, b"existing");

    drop(listener);
    clean(&path).await;
}

#[tokio::test]
async fn overwrite_truncates_existing() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    // Both an old target file and a resumable partial: Overwrite must ignore the partial and replace the target.
    seed_partial(&path, &url, b"0123", None, None).await;
    tokio::fs::write(&path, b"old stale contents").await.unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            !request.to_lowercase().contains("range:"),
            "Overwrite must not send a Range header"
        );
        write_response(&mut stream, "200 OK", "fresh").await;
    });

    let mut download = Fetch::new().download_with_options(
        &url,
        &path,
        RequestOptions::new().download_mode(DownloadMode::Overwrite),
    );
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(progress.completed);
    assert!(!progress.failed);

    assert_settled(&path, b"fresh").await;
    clean(&path).await;
}

#[tokio::test]
async fn resume_416_treated_as_complete() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    // A partial that is in fact the whole file; the server rejects the range with 416.
    seed_partial(&path, &url, b"0123456789", None, None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_range_not_satisfiable(&mut stream, 10).await;
    });

    let mut download = Fetch::new().download(&url, &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(progress.completed);
    assert!(!progress.failed);
    assert_eq!(progress.downloaded, 10);

    // The complete partial was promoted rather than transferred again.
    assert_settled(&path, b"0123456789").await;
    clean(&path).await;
}

// --- partial-file bookkeeping ---
//
// One test per row of the resume state machine: what is on disk, what the sidecar says, and whether a byte of it may
// be kept. Each is a case where writing straight to the target path silently produced a corrupt or falsely-complete
// file.

#[tokio::test]
async fn a_completed_transfer_leaves_no_partial_behind() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    clean(&path).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "complete").await;
    });

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
    drain(&mut download).await;
    server.await.unwrap();

    assert_settled(&path, b"complete").await;
    clean(&path).await;
}

#[tokio::test]
async fn a_changed_url_discards_the_partial_instead_of_appending_to_it() {
    // The bug this bookkeeping exists for. Release assets are namespaced by version in the URL but keep the same file
    // name, so a version bump is a different URL writing to the same local path — and a resume would append the new
    // release's bytes onto the old one's prefix.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    seed_partial(&path, &format!("http://{addr}/v1/asset.bin"), b"OLD!", None, None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            !request.to_lowercase().contains("range:"),
            "a partial from a different URL must not be resumed, got: {request}"
        );
        write_response(&mut stream, "200 OK", "v2 bytes").await;
    });

    let mut download = Fetch::new().download(format!("http://{addr}/v2/asset.bin"), &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(!progress.failed);
    assert_settled(&path, b"v2 bytes").await;
    clean(&path).await;
}

#[tokio::test]
async fn a_partial_without_a_sidecar_restarts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    // Bytes whose origin is unknowable. Guessing that they belong to this request is the whole failure mode.
    clean(&path).await;
    tokio::fs::write(partial::part_path(&path), b"orphaned").await.unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            !request.to_lowercase().contains("range:"),
            "an unidentifiable partial must not be resumed, got: {request}"
        );
        write_response(&mut stream, "200 OK", "fresh bytes").await;
    });

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
    drain(&mut download).await;
    server.await.unwrap();

    assert_settled(&path, b"fresh bytes").await;
    clean(&path).await;
}

#[tokio::test]
async fn a_mismatched_resume_key_restarts() {
    // Same URL, different bytes — a release re-cut in place. Only the caller's own identity can see it.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    seed_partial(&path, &url, b"0123", None, Some("sha256:old")).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            !request.to_lowercase().contains("range:"),
            "a partial with a different resume_key must not be resumed, got: {request}"
        );
        write_response(&mut stream, "200 OK", "new bytes").await;
    });

    let mut download =
        Fetch::new().download_with_options(&url, &path, RequestOptions::new().resume_key("sha256:new"));
    drain(&mut download).await;
    server.await.unwrap();

    assert_settled(&path, b"new bytes").await;
    clean(&path).await;
}

#[tokio::test]
async fn a_matching_resume_key_resumes() {
    // The counterpart to the test above: the same key means the partial really is these bytes, so it is kept.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    seed_partial(&path, &url, b"0123", None, Some("sha256:same")).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.contains("bytes=4-"),
            "expected a resume Range header, got: {request}"
        );
        write_partial_response(&mut stream, 4, 10, "456789").await;
    });

    let mut download =
        Fetch::new().download_with_options(&url, &path, RequestOptions::new().resume_key("sha256:same"));
    drain(&mut download).await;
    server.await.unwrap();

    assert_settled(&path, b"0123456789").await;
    clean(&path).await;
}

#[tokio::test]
async fn a_recorded_etag_is_sent_as_if_range() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    seed_partial(&path, &url, b"0123", Some("\"v1\""), None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.to_lowercase().contains("if-range: \"v1\""),
            "expected the recorded ETag as If-Range, got: {request}"
        );
        write_partial_response(&mut stream, 4, 10, "456789").await;
    });

    let mut download = Fetch::new().download(&url, &path);
    drain(&mut download).await;
    server.await.unwrap();

    assert_settled(&path, b"0123456789").await;
    clean(&path).await;
}

#[tokio::test]
async fn if_range_answered_with_200_restarts() {
    // The server's own verdict that the bytes changed: it declines the range and sends the whole resource instead.
    // Nothing extra handles this — the existing fresh-download branch does, which is exactly the point.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    seed_partial(&path, &url, b"0123", Some("\"v1\""), None).await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(request.to_lowercase().contains("if-range:"));
        write_response(&mut stream, "200 OK", "replaced!!").await;
    });

    let mut download = Fetch::new().download(&url, &path);
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(!progress.failed);
    // The old prefix was discarded rather than kept in front of the new body.
    assert_settled(&path, b"replaced!!").await;
    clean(&path).await;
}

#[tokio::test]
async fn an_interrupted_transfer_records_what_its_partial_is() {
    // The write half of the bookkeeping: everything above seeds a sidecar by hand, so without this nothing proves the
    // transfer itself leaves one — and a partial with no sidecar is thrown away, which would quietly cost every resume.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());
    let url = format!("http://{addr}");

    clean(&path).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // Announce far more than is ever sent, then stall so the transfer is interrupted mid-body.
        use tokio::io::AsyncWriteExt;
        let header = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n";
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(b"partial").await.unwrap();
        stream.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let mut download = Fetch::new().download_with_options(&url, &path, RequestOptions::new().resume_key("sha256:abc"));
    download.changed().await.unwrap();
    download.cancel();
    let _ = download.join().await;
    server.abort();

    assert!(partial::part_path(&path).exists(), "the partial bytes were not kept");
    let sidecar = partial::read(&partial::sidecar_path(&path))
        .await
        .expect("an interrupted transfer must record what its partial is");
    assert_eq!(sidecar.url, reqwest::Url::parse(&url).unwrap().to_string());
    assert_eq!(sidecar.etag.as_deref(), Some("\"v1\""));
    assert_eq!(sidecar.total, Some(1000));
    assert_eq!(sidecar.resume_key.as_deref(), Some("sha256:abc"));

    clean(&path).await;
}

#[tokio::test]
async fn a_cancelled_transfer_leaves_nothing_at_the_target_path() {
    // The second half of the bug: writing straight to the target left a truncated file there, after which `Skip`
    // reported it complete forever.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    clean(&path).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // Announce far more than is ever sent, then stall so the transfer is cancelled mid-body.
        use tokio::io::AsyncWriteExt;
        let header = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n";
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(b"partial").await.unwrap();
        stream.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let mut download = Fetch::new().download(format!("http://{addr}"), &path);
    // Wait until bytes are actually in flight, then abort.
    download.changed().await.unwrap();
    download.cancel();
    assert!(matches!(download.join().await, Err(DownloadError::Cancelled)));
    server.abort();

    assert!(
        !path.exists(),
        "a cancelled transfer must not leave a file at the target path"
    );

    // ...so a following Skip has nothing to mistake for a finished download and actually transfers. A second server
    // on its own port keeps this half independent of the stalled one above.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "the real thing").await;
    });

    let mut download = Fetch::new().download_with_options(
        format!("http://{addr}"),
        &path,
        RequestOptions::new().download_mode(DownloadMode::Skip),
    );
    let progress = drain(&mut download).await;
    server.await.unwrap();

    assert!(!progress.failed);
    assert_settled(&path, b"the real thing").await;
    clean(&path).await;
}

#[tokio::test]
async fn struct_download_mode_default_is_overridable() {
    // The struct default (Overwrite) applies when the request leaves the mode unset...
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = temp_path(addr.port());

    let _ = tokio::fs::remove_file(&path).await;
    tokio::fs::write(&path, b"stale").await.unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, "200 OK", "fresh").await;
    });

    let fetch = Fetch::new().download_mode(DownloadMode::Overwrite);
    let mut download = fetch.download(format!("http://{addr}"), &path);
    drain(&mut download).await;
    server.await.unwrap();
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"fresh");

    // ...and a per-request override (Skip) wins over the struct default.
    let _ = tokio::fs::remove_file(&path).await;
    tokio::fs::write(&path, b"kept").await.unwrap();
    let mut download = fetch.download_with_options(
        format!("http://{addr}"),
        &path,
        RequestOptions::new().download_mode(DownloadMode::Skip),
    );
    let progress = drain(&mut download).await;
    assert!(progress.completed && !progress.failed);
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"kept");

    let _ = tokio::fs::remove_file(&path).await;
}

// --- connect timeout and proxy tests ---

#[test]
fn connect_timeout_defaults_to_unbounded_and_round_trips() {
    assert_eq!(Fetch::new().connect_timeout, None);

    let bounded = Fetch::new().connect_timeout(Duration::from_secs(30));
    assert_eq!(bounded.connect_timeout, Some(Duration::from_secs(30)));

    // `impl Into<Option<Duration>>` accepts `None` to clear it, matching `read_timeout`.
    assert_eq!(bounded.connect_timeout(None).connect_timeout, None);
}

#[test]
fn proxy_mode_defaults_to_detection() {
    assert_eq!(Fetch::new().proxy, ProxyMode::Detect);
}

#[test]
fn proxy_sets_an_explicit_override_and_no_proxy_disables_detection() {
    let settings = ProxySettings::new("http://proxy.example.com:3128");
    let explicit = Fetch::new().proxy(settings.clone());
    assert_eq!(explicit.proxy, ProxyMode::Explicit(settings));

    assert_eq!(Fetch::new().no_proxy().proxy, ProxyMode::Disabled);

    // The two are mutually exclusive rather than additive: the last call wins.
    let overridden = Fetch::new()
        .proxy(ProxySettings::new("http://proxy.example.com:3128"))
        .no_proxy();
    assert_eq!(overridden.proxy, ProxyMode::Disabled);
}

#[test]
fn proxy_settings_carry_credentials_and_a_bypass_list() {
    let settings = ProxySettings::new("http://proxy.example.com:3128")
        .basic_auth("user", "secret")
        .no_proxy("localhost,.internal");

    assert_eq!(settings.url, "http://proxy.example.com:3128");
    assert_eq!(settings.basic_auth, Some(("user".to_string(), "secret".to_string())));
    assert_eq!(settings.no_proxy.as_deref(), Some("localhost,.internal"));
}

#[test]
fn proxy_settings_display_never_reveals_the_password() {
    let settings = ProxySettings::new("http://proxy.example.com:3128").basic_auth("user", "secret");
    let rendered = settings.to_string();

    assert_eq!(rendered, "http://proxy.example.com:3128");
    assert!(!rendered.contains("secret"));
}

#[test]
fn proxy_settings_build_a_reqwest_proxy() {
    let settings = ProxySettings::new("http://proxy.example.com:3128")
        .basic_auth("user", "secret")
        .no_proxy("localhost");

    assert!(settings.to_reqwest().is_ok());
}

#[test]
fn an_invalid_proxy_url_surfaces_when_the_client_is_built() {
    // Deferred rather than rejected by the builder, so `proxy()` stays infallible.
    let fetch = Fetch::new().proxy(ProxySettings::new("not a url"));
    assert!(fetch.client().is_err());
}

#[test]
fn a_valid_proxy_url_builds_a_client() {
    assert!(
        Fetch::new()
            .proxy(ProxySettings::new("http://proxy.example.com:3128"))
            .client()
            .is_ok()
    );
}

#[test]
fn config_builders_reset_the_cached_client() {
    // A cached client built before the proxy was set would keep routing directly, so both of these
    // must clear it.
    let fetch = Fetch::new();
    assert!(fetch.client().is_ok());
    assert!(fetch.client.get().is_some());

    let reconfigured = fetch.connect_timeout(Duration::from_secs(5));
    assert!(reconfigured.client.get().is_none());

    assert!(reconfigured.client().is_ok());
    let reconfigured = reconfigured.no_proxy();
    assert!(reconfigured.client.get().is_none());
}

#[test]
fn clone_carries_the_connect_timeout_and_proxy() {
    let original = Fetch::new()
        .connect_timeout(Duration::from_secs(7))
        .proxy(ProxySettings::new("http://proxy.example.com:3128"));
    let copy = original.clone();

    assert_eq!(copy.connect_timeout, Some(Duration::from_secs(7)));
    assert_eq!(copy.proxy, original.proxy);
    // The clone starts with an empty client cache, as the existing `Clone` contract states.
    assert!(copy.client.get().is_none());
}
