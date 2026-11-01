//! A local stand-in for the Hugging Face Hub, for the `ai` tests: `revision`, `tree` and `resolve` over plain HTTP/1.1
//! on an ephemeral port, with ranged downloads, and switches for the failures the tests need.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// One request, as it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    /// The path and query.
    pub path: String,
    /// Where a `Range: bytes=<from>-` asked to start.
    pub range_from: Option<u64>,
}

#[derive(Default)]
struct State {
    base: String,
    repo: String,
    commit: String,
    /// Each `revision` request answers a new commit, `<commit>-<n>`, as if `main` moved after every answer.
    moving: bool,
    resolved: usize,
    /// Path to (bytes, stored in LFS).
    files: BTreeMap<String, (Vec<u8>, bool)>,
    /// Every API request answers this status.
    api_status: Option<u16>,
    /// Downloads of these paths serve these bytes instead of what the listing describes.
    serve_instead: BTreeMap<String, Vec<u8>>,
    /// Downloads of these paths answer this status.
    file_status: BTreeMap<String, u16>,
    /// Downloads of these paths send this many bytes of the body, then hold the connection open.
    hang_after: BTreeMap<String, usize>,
    /// Downloads of these paths send this many bytes of the body, then close the connection.
    break_after: BTreeMap<String, usize>,
    /// Downloads of these paths wait this long before answering.
    delay: BTreeMap<String, Duration>,
    page_size: Option<usize>,
    requests: Vec<Request>,
}

/// The server. It runs until the process ends.
#[derive(Clone)]
pub(crate) struct Server {
    base: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    /// Serve `repo` with `main` at `commit`, and no files.
    pub fn start(repo: &str, commit: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            base: base.clone(),
            repo: repo.into(),
            commit: commit.into(),
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                std::thread::spawn(move || answer(stream, &state));
            }
        });
        Self { base, state }
    }

    /// A base URL nothing listens on.
    pub fn closed_base() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The URL of `folder`'s tree at `main`, as a caller names a dependency.
    pub fn tree_url(&self, folder: &str) -> String {
        format!("{}/{}/tree/main/{folder}", self.base, self.lock().repo)
    }

    /// Put `bytes` at `path`, in LFS or not, replacing what was there.
    pub fn put(&self, path: &str, bytes: &[u8], lfs: bool) {
        self.lock().files.insert(path.into(), (bytes.to_vec(), lfs));
    }

    pub fn set_commit(&self, commit: &str) {
        self.lock().commit = commit.into();
    }

    /// Answer a new commit to every `revision` request.
    pub fn move_on_every_resolve(&self) {
        self.lock().moving = true;
    }

    pub fn fail_api(&self, status: u16) {
        self.lock().api_status = Some(status);
    }

    /// Serve `bytes` for `path`, while the listing still describes what [`put`](Self::put) put there.
    pub fn corrupt(&self, path: &str, bytes: &[u8]) {
        self.lock().serve_instead.insert(path.into(), bytes.to_vec());
    }

    pub fn fail_file(&self, path: &str, status: u16) {
        self.lock().file_status.insert(path.into(), status);
    }

    pub fn hang_after(&self, path: &str, bytes: usize) {
        self.lock().hang_after.insert(path.into(), bytes);
    }

    /// Serve `path` whole again after [`hang_after`](Self::hang_after).
    pub fn stop_hanging(&self, path: &str) {
        self.lock().hang_after.remove(path);
    }

    pub fn break_after(&self, path: &str, bytes: usize) {
        self.lock().break_after.insert(path.into(), bytes);
    }

    /// Wait `delay` before answering each download of `path`.
    pub fn delay(&self, path: &str, delay: Duration) {
        self.lock().delay.insert(path.into(), delay);
    }

    pub fn page_size(&self, size: usize) {
        self.lock().page_size = Some(size);
    }

    /// Name each next page of a listing on `base` rather than on this server.
    pub fn link_pages_to(&self, base: &str) {
        self.lock().base = base.into();
    }

    /// Every request's path, in order.
    pub fn requests(&self) -> Vec<String> {
        self.lock()
            .requests
            .iter()
            .map(|request| request.path.clone())
            .collect()
    }

    /// Every download request (`/resolve/`), in order.
    pub fn downloads(&self) -> Vec<Request> {
        self.lock()
            .requests
            .iter()
            .filter(|request| request.path.contains("/resolve/"))
            .cloned()
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn answer(mut stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut range_from = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
            break;
        }
        if let Some(range) = header.to_ascii_lowercase().strip_prefix("range: bytes=") {
            range_from = range.trim().trim_end_matches('-').parse().ok();
        }
    }

    let (reply, delay) = {
        let mut state = lock(state);
        state.requests.push(Request {
            path: path.clone(),
            range_from,
        });
        let delay = state
            .delay
            .iter()
            .find(|(file, _)| path.contains("/resolve/") && path.ends_with(file.as_str()))
            .map(|(_, delay)| *delay);
        (route(&mut state, &path, range_from), delay)
    };
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
    match reply {
        Reply::Whole(response) => {
            let _ = stream.write_all(&response);
        }
        Reply::Hang(response) => {
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

enum Reply {
    Whole(Vec<u8>),
    /// Sent, then the connection is held open.
    Hang(Vec<u8>),
}

fn route(state: &mut State, path: &str, range_from: Option<u64>) -> Reply {
    let api = format!("/api/models/{}/", state.repo);
    if let Some(rest) = path.strip_prefix(&api) {
        if let Some(status) = state.api_status {
            return Reply::Whole(status_only(status));
        }
        if rest.starts_with("revision/") {
            state.resolved += 1;
            let commit = if state.moving {
                format!("{}-{}", state.commit, state.resolved)
            } else {
                state.commit.clone()
            };
            return Reply::Whole(ok(
                &[],
                format!(r#"{{"id":"{}","sha":"{commit}"}}"#, state.repo).as_bytes(),
            ));
        }
        if let Some(rest) = rest.strip_prefix("tree/") {
            let (rest, cursor) = match rest.split_once("?cursor=") {
                Some((rest, cursor)) => (rest, cursor.parse().unwrap_or(0)),
                None => (rest, 0),
            };
            let (_commit, folder) = rest.split_once('/').unwrap_or((rest, ""));
            let page = format!("{}{api}tree/{rest}", state.base);
            return Reply::Whole(listing(state, folder, cursor, &page));
        }
        return Reply::Whole(status_only(404));
    }

    let resolve = format!("/{}/resolve/", state.repo);
    let Some(rest) = path.strip_prefix(&resolve) else {
        return Reply::Whole(status_only(404));
    };
    let (_commit, file) = rest.split_once('/').unwrap_or((rest, ""));
    if let Some(status) = state.file_status.get(file) {
        return Reply::Whole(status_only(*status));
    }
    let Some((bytes, _)) = state.files.get(file) else {
        return Reply::Whole(status_only(404));
    };
    let bytes = state.serve_instead.get(file).unwrap_or(bytes);
    let from = range_from
        .map_or(0, |from| usize::try_from(from).unwrap())
        .min(bytes.len());
    let body = &bytes[from..];
    let etag = format!("ETag: \"{}\"", crate::crypto::sha256_bytes(bytes));
    let mut response = if range_from.is_some() {
        let range = format!(
            "Content-Range: bytes {from}-{}/{}",
            bytes.len().saturating_sub(1),
            bytes.len()
        );
        head("206 Partial Content", &[&etag, &range], body.len())
    } else {
        head("200 OK", &[&etag], body.len())
    };
    if let Some(after) = state.hang_after.get(file) {
        response.extend_from_slice(&body[..(*after).min(body.len())]);
        return Reply::Hang(response);
    }
    // Only the first transfer breaks, so the retry, or the next run, can finish it.
    if let Some(after) = state.break_after.remove(file) {
        response.extend_from_slice(&body[..after.min(body.len())]);
        return Reply::Whole(response);
    }
    response.extend_from_slice(body);
    Reply::Whole(response)
}

/// The files directly in `folder`, from `cursor`, a page at a time. `page` is this listing's own URL, which the link to
/// the next page extends.
fn listing(state: &State, folder: &str, cursor: usize, page: &str) -> Vec<u8> {
    let folder = folder.trim_matches('/');
    let entries: Vec<String> = state
        .files
        .iter()
        .filter(|(path, _)| path.rsplit_once('/').is_some_and(|(parent, _)| parent == folder))
        .map(|(path, (bytes, lfs))| {
            let oid = crate::crypto::git_blob_sha1_bytes(bytes);
            let lfs = if *lfs {
                format!(
                    r#","lfs":{{"oid":"{}","size":{}}}"#,
                    crate::crypto::sha256_bytes(bytes),
                    bytes.len()
                )
            } else {
                String::new()
            };
            format!(
                r#"{{"type":"file","oid":"{oid}","size":{}{lfs},"path":"{path}"}}"#,
                bytes.len()
            )
        })
        .collect();
    if entries.is_empty() {
        return status_only(404);
    }

    let size = state.page_size.unwrap_or(usize::MAX);
    let body = format!(
        "[{}]",
        entries
            .iter()
            .skip(cursor)
            .take(size)
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    );
    let next = cursor.saturating_add(size);
    if next < entries.len() {
        // Absolute, as the Hub sends it.
        let link = format!("Link: <{page}?cursor={next}>; rel=\"next\"");
        return ok(&[&link], body.as_bytes());
    }
    ok(&[], body.as_bytes())
}

fn head(status: &str, headers: &[&str], len: usize) -> Vec<u8> {
    let extra: String = headers.iter().map(|header| format!("{header}\r\n")).collect();
    format!("HTTP/1.1 {status}\r\nContent-Length: {len}\r\n{extra}Connection: close\r\n\r\n").into_bytes()
}

fn ok(headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut response = head("200 OK", headers, body.len());
    response.extend_from_slice(body);
    response
}

fn status_only(status: u16) -> Vec<u8> {
    // `Retry-After: 0` skips the backoff of a retried status, so a test does not wait it out.
    format!("HTTP/1.1 {status} Status\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}
