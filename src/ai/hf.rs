//! A small Hugging Face Hub client: a branch resolved to its commit, a folder listed with each file's size and hash,
//! and the URL a file downloads from.
//!
//! The requests go through [`fetch`](crate::fetch), with its retries and timeouts.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::{Folder, Source};
use crate::fetch::{Fetch, FetchError};

/// Where the Hub lives.
pub const DEFAULT_BASE: &str = "https://huggingface.co";

/// Retries of a request that failed transiently (network, timeout, 5xx, 408, 429), with `fetch`'s Fibonacci backoff
/// (1 + 2 + 3 + 5 + 8 s), before the error is reported. Anything else, such as a 404, fails at once.
pub const RETRIES: u32 = 5;

/// The bound on establishing a connection, and on a stall between two reads.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// One repository on a Hub.
#[derive(Debug, Clone)]
pub struct Hub {
    base: String,
    repo: String,
    fetch: Fetch,
}

/// A file in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// The path in the repository, as in `deps/onnx/1.30.0/onnx_macos_arm64.7z`.
    pub path: String,
    /// The length in bytes.
    pub size: u64,
    /// The hash the file is verified by.
    pub hash: Hash,
}

impl RemoteFile {
    /// The path's last segment.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// A file's expected hash, as the API reports it, in lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Hash {
    /// The LFS SHA-256 of a file stored in LFS (`lfs.oid`).
    Sha256(String),
    /// The git blob SHA-1 of a file stored in git itself (`oid`): SHA-1 over `blob <length>\0` and the bytes.
    GitBlobSha1(String),
}

impl Hash {
    /// The hex digest.
    pub fn hex(&self) -> &str {
        match self {
            Hash::Sha256(hex) | Hash::GitBlobSha1(hex) => hex,
        }
    }

    /// The same kind of hash, of the file at `path`. Reads the whole file.
    ///
    /// # Errors
    ///
    /// Any error opening or reading the file.
    pub fn of_file(&self, path: impl AsRef<Path>) -> io::Result<Hash> {
        Ok(match self {
            Hash::Sha256(_) => Hash::Sha256(crate::crypto::sha256_file(path)?),
            Hash::GitBlobSha1(_) => Hash::GitBlobSha1(crate::crypto::git_blob_sha1_file(path)?),
        })
    }

    /// Whether the file at `path` hashes to this. Reads the whole file.
    ///
    /// # Errors
    ///
    /// Any error opening or reading the file.
    pub fn verify_file(&self, path: impl AsRef<Path>) -> io::Result<bool> {
        Ok(self.matches(self.of_file(path)?.hex()))
    }

    /// Whether `hex` is this digest, ignoring case.
    pub fn matches(&self, hex: &str) -> bool {
        self.hex().eq_ignore_ascii_case(hex.trim())
    }
}

/// Why a request to the Hub failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HfError {
    /// The Hub could not be reached, or the connection broke or timed out, after the retries.
    #[error("the network could not be reached")]
    Network,
    /// The Hub answered with this HTTP status, after the retries for a transient one.
    #[error("the server answered with HTTP {0}")]
    Http(u16),
    /// The Hub answered with something that is not what its API returns.
    #[error("the server's answer could not be read: {0}")]
    Malformed(String),
}

impl HfError {
    /// Whether the Hub could not be reached at all: no network, a timeout, or a 5xx after the retries. An install
    /// that finds this falls back to what it already has.
    pub fn is_unreachable(&self) -> bool {
        match self {
            HfError::Network => true,
            HfError::Http(status) => *status >= 500,
            HfError::Malformed(_) => false,
        }
    }
}

impl Default for Hub {
    /// `vegidio/general` on huggingface.co.
    fn default() -> Self {
        Self::new("vegidio/general")
    }
}

impl Hub {
    /// `repo`, as `<owner>/<name>`, on huggingface.co.
    pub fn new(repo: impl Into<String>) -> Self {
        Self::with_base(DEFAULT_BASE, repo)
    }

    /// `repo` on the Hub at `base`, a scheme and host such as `https://huggingface.co`.
    pub fn with_base(base: impl Into<String>, repo: impl Into<String>) -> Self {
        let fetch = Fetch::new()
            .retries(RETRIES)
            .connect_timeout(TIMEOUT)
            .read_timeout(TIMEOUT);
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            repo: repo.into(),
            fetch,
        }
    }

    /// The repository `source` is in, on its Hub.
    pub fn for_source(source: &Source) -> Self {
        Self::for_folder(source.as_folder())
    }

    /// The repository `folder` is in, on its Hub.
    pub fn for_folder(folder: &Folder) -> Self {
        Self::with_base(folder.base(), folder.repo())
    }

    /// The same Hub with `retries` retries of a transient failure instead of [`RETRIES`].
    #[must_use]
    pub fn retries(mut self, retries: u32) -> Self {
        self.fetch = self.fetch.retries(retries);
        self
    }

    /// The scheme and host.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The repository, as `<owner>/<name>`.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// The client the requests go through, for downloads that should retry and time out the same way.
    pub fn fetch(&self) -> &Fetch {
        &self.fetch
    }

    /// The commit `revision` (a branch, a tag or a commit) points at now.
    ///
    /// # Errors
    ///
    /// [`HfError`] when the Hub cannot be reached, answers with an error status, or answers with no commit.
    pub async fn resolve(&self, revision: &str) -> Result<String, HfError> {
        #[derive(Deserialize)]
        struct Revision {
            sha: String,
        }
        let url = format!("{}/api/models/{}/revision/{revision}", self.base, self.repo);
        let answer: Revision = self.fetch.json(url).await.map_err(|cause| failure(&cause))?;
        Ok(answer.sha)
    }

    /// The files directly inside `folder` at `commit`, with their sizes and hashes. Subfolders are left out.
    ///
    /// # Errors
    ///
    /// [`HfError`] when the Hub cannot be reached, answers with an error status (404 for a folder that does not
    /// exist), or answers with something that is not a listing.
    pub async fn list(&self, commit: &str, folder: &str) -> Result<Vec<RemoteFile>, HfError> {
        self.list_pages(commit, folder, Unhashed::Refuse).await
    }

    async fn list_pages(&self, commit: &str, folder: &str, unhashed: Unhashed) -> Result<Vec<RemoteFile>, HfError> {
        let mut url = format!(
            "{}/api/models/{}/tree/{commit}/{}",
            self.base,
            self.repo,
            folder.trim_matches('/')
        );
        let mut files = Vec::new();
        // A large folder is paged, each page naming the next in a `Link` header.
        loop {
            let page = self.fetch.text_response(&url).await.map_err(|cause| failure(&cause))?;
            files.extend(parse_page(&page.body, unhashed)?);
            match page.link("next") {
                Some(next) => url = next.to_owned(),
                None => return Ok(files),
            }
        }
    }

    /// Where the file at `path` downloads from at `commit`. The Hub redirects it to its CDN.
    pub fn url(&self, commit: &str, path: &str) -> String {
        format!(
            "{}/{}/resolve/{commit}/{}",
            self.base,
            self.repo,
            path.trim_start_matches('/')
        )
    }
}

/// A folder's files at one commit, read once and passed to every [`install_files`](super::install_files) in that
/// folder, so a caller with many installs there pays for one read, and every file of them comes from one revision.
/// It keeps the [`Hub`] it was read through, so those installs download through its client.
#[derive(Debug, Clone)]
pub struct Listing {
    hub: Arc<Hub>,
    folder: Folder,
    commit: String,
    files: Vec<RemoteFile>,
}

impl Listing {
    /// Resolve `folder`'s revision to its commit, then list every page of the folder at that commit. A file the API
    /// reports no hash for is left out, as though it were not there, since nothing could verify it.
    ///
    /// # Errors
    ///
    /// [`HfError`] when the Hub cannot be reached, answers with an error status (404 for a folder that does not
    /// exist), or answers with something that is not a listing.
    pub async fn read(folder: &Folder) -> Result<Self, HfError> {
        Self::read_with(Hub::for_folder(folder), folder).await
    }

    /// [`read`](Self::read) through `hub`, a [`Hub::for_folder`] configured as the caller wants, such as with no
    /// [`retries`](Hub::retries) for a test of an unreachable Hub.
    ///
    /// # Errors
    ///
    /// As [`read`](Self::read), and [`HfError::Malformed`] for a hub of another repository than `folder`'s.
    pub async fn read_with(hub: Hub, folder: &Folder) -> Result<Self, HfError> {
        if hub.base() != folder.base().trim_end_matches('/') || hub.repo() != folder.repo() {
            return Err(HfError::Malformed(format!(
                "the hub is for {}/{}, not for {folder}",
                hub.base(),
                hub.repo()
            )));
        }
        let commit = hub.resolve(folder.revision()).await?;
        let files = hub.list_pages(&commit, folder.path(), Unhashed::Skip).await?;
        Ok(Self {
            hub: Arc::new(hub),
            folder: folder.clone(),
            commit,
            files,
        })
    }

    /// The hub the listing was read through.
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// The folder listed.
    pub fn folder(&self) -> &Folder {
        &self.folder
    }

    /// The commit the folder's revision pointed at, which every download from this listing uses.
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The files directly inside the folder, with their sizes and hashes.
    pub fn files(&self) -> &[RemoteFile] {
        &self.files
    }
}

/// Two listings are equal when they list the same folder at the same commit with the same files, whatever hub read them.
impl PartialEq for Listing {
    fn eq(&self, other: &Self) -> bool {
        (&self.folder, &self.commit, &self.files) == (&other.folder, &other.commit, &other.files)
    }
}

impl Eq for Listing {}

/// What a page's entry with no hash does to the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unhashed {
    /// The page is malformed.
    Refuse,
    /// The entry is left out.
    Skip,
}

/// The files in one page of a listing.
#[cfg(test)]
pub(crate) fn parse_listing(body: &str) -> Result<Vec<RemoteFile>, HfError> {
    parse_page(body, Unhashed::Refuse)
}

fn parse_page(body: &str, unhashed: Unhashed) -> Result<Vec<RemoteFile>, HfError> {
    #[derive(Deserialize)]
    struct Entry {
        #[serde(rename = "type")]
        kind: String,
        path: String,
        #[serde(default)]
        oid: Option<String>,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        lfs: Option<Lfs>,
    }
    #[derive(Deserialize)]
    struct Lfs {
        oid: String,
    }

    let entries: Vec<Entry> =
        serde_json::from_str(body).map_err(|cause| HfError::Malformed(format!("not a folder listing: {cause}")))?;
    entries
        .into_iter()
        .filter(|entry| entry.kind == "file")
        .filter_map(|entry| {
            let hash = match (entry.lfs, entry.oid) {
                (Some(lfs), _) => Hash::Sha256(lfs.oid.to_ascii_lowercase()),
                (None, Some(oid)) => Hash::GitBlobSha1(oid.to_ascii_lowercase()),
                (None, None) if unhashed == Unhashed::Skip => return None,
                (None, None) => return Some(Err(HfError::Malformed(format!("{} has no hash", entry.path)))),
            };
            Some(Ok(RemoteFile {
                path: entry.path,
                size: entry.size,
                hash,
            }))
        })
        .collect()
}

/// What a failed request means.
fn failure(cause: &FetchError) -> HfError {
    match cause {
        FetchError::Http(error) => match error.status() {
            Some(status) => HfError::Http(status.as_u16()),
            None if error.is_decode() || error.is_body() => HfError::Malformed(error.to_string()),
            None => HfError::Network,
        },
        other => HfError::Malformed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::test_support::Server;

    #[test]
    fn urls_resolve_at_the_commit() {
        let hub = Hub::default();
        assert_eq!(
            hub.url(
                "501d7b94e6d22762667e213994b0242ccdb04588",
                "deps/onnx/1.30.0/onnx_macos_arm64.7z"
            ),
            "https://huggingface.co/vegidio/general/resolve/501d7b94e6d22762667e213994b0242ccdb04588/deps/onnx/1.30.0/onnx_macos_arm64.7z"
        );
        let source = Source::parse("http://127.0.0.1:9/a/b/tree/main/x/1").unwrap();
        assert_eq!(
            Hub::for_source(&source).url("c0ffee", "x/1/f.7z"),
            "http://127.0.0.1:9/a/b/resolve/c0ffee/x/1/f.7z"
        );
    }

    #[test]
    fn a_listing_takes_the_lfs_sha256_or_else_the_git_blob_sha1() {
        let listing = parse_listing(
            r#"[{"type":"file","oid":"7C1E0ED1759922FC4EB362D3B405958E829F364D","size":559,"path":"siglip2/config.json"},
                {"type":"directory","oid":"abc","size":0,"path":"siglip2/sub"},
                {"type":"file","oid":"8394","size":1707726472,"lfs":{"oid":"97040ec386ff9e49134b73ac5c835b493144ab9cc1169a5bee1b8c93b8000a5a","size":1707726472,"pointerSize":135},"path":"siglip2/model.onnx"}]"#,
        )
        .unwrap();
        assert_eq!(
            listing,
            [
                RemoteFile {
                    path: "siglip2/config.json".into(),
                    size: 559,
                    hash: Hash::GitBlobSha1("7c1e0ed1759922fc4eb362d3b405958e829f364d".into()),
                },
                RemoteFile {
                    path: "siglip2/model.onnx".into(),
                    size: 1_707_726_472,
                    hash: Hash::Sha256("97040ec386ff9e49134b73ac5c835b493144ab9cc1169a5bee1b8c93b8000a5a".into()),
                },
            ]
        );
        assert!(matches!(parse_listing("{}"), Err(HfError::Malformed(_))));
    }

    #[test]
    fn a_hash_verifies_a_file_by_its_own_kind() {
        let dir = crate::fs::mk_temp_dir("rust-sak-hf").unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, include_bytes!("../crypto/testdata/siglip2_config.json")).unwrap();
        assert!(
            Hash::GitBlobSha1("7c1e0ed1759922fc4eb362d3b405958e829f364d".into())
                .verify_file(&path)
                .unwrap()
        );
        assert!(
            !Hash::Sha256("7c1e0ed1759922fc4eb362d3b405958e829f364d".into())
                .verify_file(&path)
                .unwrap()
        );
        let sha256 = crate::crypto::sha256_file(&path).unwrap();
        assert!(Hash::Sha256(sha256.to_ascii_uppercase()).verify_file(&path).unwrap());
    }

    #[tokio::test]
    async fn main_resolves_to_its_commit() {
        let server = Server::start("test/repo", "0123abcd");
        let hub = Hub::with_base(server.base(), "test/repo");
        assert_eq!(hub.resolve("main").await, Ok("0123abcd".into()));
        assert!(
            server
                .requests()
                .contains(&"/api/models/test/repo/revision/main".to_owned())
        );
    }

    #[tokio::test]
    async fn a_folder_lists_its_lfs_and_non_lfs_files_at_the_commit() {
        let server = Server::start("test/repo", "0123abcd");
        server.put("models/a/config.json", b"{}", false);
        server.put("models/a/model.onnx", b"weights", true);
        server.put("models/b/other.onnx", b"other", true);
        let hub = Hub::with_base(server.base(), "test/repo");

        let listing = hub.list("0123abcd", "models/a").await.unwrap();
        assert_eq!(
            listing,
            [
                RemoteFile {
                    path: "models/a/config.json".into(),
                    size: 2,
                    hash: Hash::GitBlobSha1(crate::crypto::sha1_bytes(b"blob 2\0{}")),
                },
                RemoteFile {
                    path: "models/a/model.onnx".into(),
                    size: 7,
                    hash: Hash::Sha256(crate::crypto::sha256_bytes(b"weights")),
                },
            ]
        );
        assert!(
            server
                .requests()
                .contains(&"/api/models/test/repo/tree/0123abcd/models/a".to_owned())
        );
    }

    #[tokio::test]
    async fn statuses_and_an_unreachable_host_are_told_apart() {
        let server = Server::start("test/repo", "0123abcd");
        let hub = Hub::with_base(server.base(), "test/repo").retries(0);
        assert_eq!(hub.list("0123abcd", "missing/folder").await, Err(HfError::Http(404)));

        server.fail_api(503);
        let error = hub.resolve("main").await.unwrap_err();
        assert_eq!(error, HfError::Http(503));
        assert!(error.is_unreachable(), "a 5xx after the retries counts as unreachable");
        assert!(!HfError::Http(404).is_unreachable());

        let closed = Hub::with_base(Server::closed_base(), "test/repo").retries(0);
        let error = closed.resolve("main").await.unwrap_err();
        assert_eq!(error, HfError::Network);
        assert!(error.is_unreachable());
    }

    #[test]
    fn a_listing_leaves_out_a_file_with_no_hash() {
        let page = r#"[{"type":"file","size":3,"path":"models/bare.onnx"},
                       {"type":"file","oid":"aa","size":1,"path":"models/x.onnx"}]"#;
        let files = parse_page(page, Unhashed::Skip).unwrap();
        assert_eq!(files.iter().map(RemoteFile::file_name).collect::<Vec<_>>(), ["x.onnx"]);
        assert!(matches!(parse_listing(page), Err(HfError::Malformed(_))));
    }

    #[tokio::test]
    async fn a_listing_reads_every_page_at_the_resolved_commit() {
        let server = Server::start("test/repo", "0123abcd");
        for name in ["a", "b", "c", "d", "e"] {
            server.put(&format!("models/{name}.onnx"), name.as_bytes(), true);
        }
        server.put("models/sub/ignored.onnx", b"nested", true);
        server.page_size(2);
        let folder = Folder::parse(&server.tree_url("models")).unwrap();

        let listing = Listing::read_with(Hub::for_folder(&folder).retries(0), &folder)
            .await
            .unwrap();
        assert_eq!(listing.commit(), "0123abcd");
        assert_eq!(listing.folder(), &folder);
        let names: Vec<_> = listing.files().iter().map(RemoteFile::file_name).collect();
        assert_eq!(names, ["a.onnx", "b.onnx", "c.onnx", "d.onnx", "e.onnx"]);
        assert_eq!(listing.files()[0].hash, Hash::Sha256(crate::crypto::sha256_bytes(b"a")));
        let pages = server
            .requests()
            .iter()
            .filter(|path| path.starts_with("/api/models/test/repo/tree/0123abcd/models"))
            .count();
        assert_eq!(pages, 3, "five files, two to a page");
    }

    #[tokio::test]
    async fn a_listing_that_cannot_be_read_says_why() {
        let folder = Folder::parse(&format!("{}/test/repo/tree/main/models", Server::closed_base())).unwrap();
        assert_eq!(
            Listing::read_with(Hub::for_folder(&folder).retries(0), &folder).await,
            Err(HfError::Network)
        );
    }

    #[tokio::test]
    async fn a_paged_listing_follows_its_next_links() {
        let server = Server::start("test/repo", "0123abcd");
        for name in ["a", "b", "c"] {
            server.put(&format!("big/folder/{name}.bin"), name.as_bytes(), true);
        }
        server.page_size(2);
        let hub = Hub::with_base(server.base(), "test/repo");
        let names: Vec<_> = hub
            .list("0123abcd", "big/folder")
            .await
            .unwrap()
            .iter()
            .map(|file| file.file_name().to_owned())
            .collect();
        assert_eq!(names, ["a.bin", "b.bin", "c.bin"]);
    }
}
