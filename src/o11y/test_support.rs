//! Shared test-only helpers: throwaway local HTTP/1.1 servers used by the `o11y` unit tests.
//!
//! These bind an ephemeral port, so the tests exercise the real export and lookup paths without reaching the network.
//! They are deliberately blocking — the module under test has no async runtime.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A captured HTTP request: the header block as text, and the body as raw bytes.
pub(super) struct CapturedRequest {
    /// The request line plus headers, as received.
    pub(super) head: String,
    /// The request body, kept as bytes so a malformed payload is still inspectable.
    pub(super) body: Vec<u8>,
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

impl CapturedRequest {
    /// The body parsed as JSON.
    ///
    /// OTLP/JSON is text, so assertions read the payload's actual structure rather than searching it for a
    /// substring — which would happily pass on a field that landed in the wrong place.
    pub(super) fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("the captured body should be json")
    }

    /// The request path, taken from the request line.
    pub(super) fn path(&self) -> &str {
        self.head.split_whitespace().nth(1).unwrap_or_default()
    }
}

/// Spawns a server that accepts `count` connections, answering each with `status`, and records every request.
///
/// One flush posts up to three payloads — logs, metrics and traces — so a test that wants to see all of them needs
/// a collector that outlives the first connection.
pub(super) fn spawn_recording_collector(
    count: usize,
    status: &'static str,
) -> (String, Arc<Mutex<Vec<CapturedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));

    {
        let captured = Arc::clone(&captured);

        std::thread::spawn(move || {
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let request = read_request(&mut stream);
                write_response(&mut stream, status, "");
                captured.lock().unwrap_or_else(PoisonError::into_inner).push(request);
            }
        });
    }

    (format!("http://{addr}"), captured)
}

/// Blocks until `captured` holds `count` requests, returning whether it got there.
pub(super) fn captured_at_least(captured: &Mutex<Vec<CapturedRequest>>, count: usize) -> bool {
    wait_until(Duration::from_secs(5), || {
        captured.lock().unwrap_or_else(PoisonError::into_inner).len() >= count
    })
}

/// Serialises the tests that touch the process globals.
///
/// `cargo nextest` gives every test its own process, but `cargo llvm-cov --html` — which `scripts/coverage.sh`
/// runs by default — goes through `cargo test`, where the whole suite shares one process and therefore one set of
/// `static`s. Without this the two runners would disagree about whether the suite passes.
///
/// Poisoning is recovered from rather than propagated, so one failing test does not cascade into every other.
pub(super) fn global_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());

    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}
