//! Shared test-only helpers: throwaway local HTTP/1.1 servers used by the `o11y` unit tests.
//!
//! These bind an ephemeral port, so the tests exercise the real export and lookup paths without reaching the network.
//! They are deliberately blocking — the module under test has no async runtime.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A captured HTTP request: the header block as text, and the body as raw bytes.
pub(super) struct CapturedRequest {
    /// The request line plus headers, as received.
    pub(super) head: String,
    /// The request body. OTLP payloads are protobuf, so this stays as bytes.
    pub(super) body: Vec<u8>,
}

impl CapturedRequest {
    /// Whether the body contains `needle` as a UTF-8 substring.
    ///
    /// Protobuf keeps string fields — attribute keys and their values — as plain UTF-8, so a substring check is
    /// enough to assert a record carried a given key or value without decoding the payload.
    pub(super) fn body_contains(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }
}

/// Spawns a server that accepts one connection, answers `200 OK`, and returns the request it captured.
///
/// Returns the base URL to point a [`Telemetry`](super::Telemetry) at, plus the handle carrying the request.
pub(super) fn spawn_collector() -> (String, JoinHandle<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        write_response(&mut stream, "200 OK", "");
        request
    });

    (format!("http://{addr}"), handle)
}

/// Spawns a server that answers every request with `body` as JSON, for the geolocation lookup.
///
/// The listener is leaked into a detached thread that serves until the test ends; the lookup is fire-and-forget, so
/// there is nothing to join on.
pub(super) fn spawn_json_server(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            read_request(&mut stream);
            write_json_response(&mut stream, body);
        }
    });

    format!("http://{addr}")
}

/// Spawns a server that counts connections and answers nothing, so a test can assert on outbound traffic.
///
/// This is how "disabled means no request was even attempted" is proved: the counter must stay at zero.
pub(super) fn spawn_counting_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&count);
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            counter.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });

    (format!("http://{addr}"), count)
}

/// Polls `condition` until it returns `true` or `timeout` elapses, reporting whether it succeeded.
///
/// Used for the background geolocation lookup, which has no completion signal of its own.
pub(super) fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    condition()
}

/// Reads a request: the header block, then exactly the `Content-Length` bytes that follow it.
fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];

    // Read until the blank line that terminates the headers.
    let header_end = loop {
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }

        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break buffer.len();
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut body = buffer[usize::min(header_end + 4, buffer.len())..].to_vec();

    // Then read exactly as many body bytes as the headers promised.
    if let Some(length) = content_length(&head) {
        while body.len() < length {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
    }

    CapturedRequest { head, body }
}

/// The index of the `\r\n\r\n` that ends the header block, if it has arrived.
fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parses the `Content-Length` header out of a request head.
fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .find(|line| line.to_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|value| value.trim().parse().ok())
}

/// Writes a minimal HTTP/1.1 response with the given status line and body.
fn write_response(stream: &mut std::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Writes a `200 OK` response carrying `body` as `application/json`.
fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
