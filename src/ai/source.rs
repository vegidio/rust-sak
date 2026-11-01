//! A Hugging Face folder, and a dependency's folder, as the caller names them, and the archive in it for a platform.

use std::fmt;

use reqwest::Url;

use super::hf::RemoteFile;

/// A folder on Hugging Face, parsed from its URL: `https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`,
/// where the folder has one segment or more, as in `models` or `models/insightface`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Folder {
    base: String,
    repo: String,
    revision: String,
    path: String,
}

/// A URL that is not a Hugging Face folder URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{url} is not a Hugging Face folder URL of the form https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>"
)]
pub struct FolderError {
    /// The URL as given.
    pub url: String,
}

impl Folder {
    /// Parse a Hugging Face folder URL. The host is kept, so a mirror or a local test server works the same way.
    ///
    /// # Errors
    ///
    /// [`FolderError`] when `url` is not an `http(s)` URL with the path `/<owner>/<repo>/tree/<revision>/<folder>`,
    /// where the folder has at least one segment and none is `.` or `..`, or when it carries a query or a fragment.
    pub fn parse(url: &str) -> Result<Self, FolderError> {
        let error = || FolderError { url: url.to_owned() };
        let parsed = Url::parse(url).map_err(|_| error())?;
        if !matches!(parsed.scheme(), "https" | "http")
            || parsed.host_str().is_none()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(error());
        }

        let segments: Vec<&str> = parsed.path().split('/').filter(|segment| !segment.is_empty()).collect();
        let [owner, repo, "tree", revision, folder @ ..] = segments.as_slice() else {
            return Err(error());
        };
        if folder.is_empty() || folder.iter().any(|segment| matches!(*segment, "." | "..")) {
            return Err(error());
        }

        let mut base = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or_default());
        if let Some(port) = parsed.port() {
            base.push_str(&format!(":{port}"));
        }
        Ok(Self {
            base,
            repo: format!("{owner}/{repo}"),
            revision: (*revision).to_owned(),
            path: folder.join("/"),
        })
    }

    /// The scheme and host, as in `https://huggingface.co`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The repository, as in `vegidio/open-photo-ai`.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// The branch, tag or commit the URL names, as in `main`.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The folder inside the repository, as in `models`.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for Folder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/tree/{}/{}", self.base, self.repo, self.revision, self.path)
    }
}

/// A dependency's folder on Hugging Face, parsed from its URL:
/// `https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`. The folder's last two segments are the
/// dependency's name and version, as in `deps/cuda/13.3.0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Source {
    folder: Folder,
}

/// A URL that is not a Hugging Face folder URL with a name and a version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{url} is not a Hugging Face folder URL of the form https://huggingface.co/<owner>/<repo>/tree/<revision>/<name>/<version>"
)]
pub struct SourceError {
    /// The URL as given.
    pub url: String,
}

impl Source {
    /// Parse a Hugging Face folder URL. The host is kept, so a mirror or a local test server works the same way.
    ///
    /// # Errors
    ///
    /// [`SourceError`] when `url` is not a [`Folder`] URL whose folder has at least two segments.
    pub fn parse(url: &str) -> Result<Self, SourceError> {
        let error = || SourceError { url: url.to_owned() };
        let folder = Folder::parse(url).map_err(|_| error())?;
        if !folder.path.contains('/') {
            return Err(error());
        }
        Ok(Self { folder })
    }

    /// The folder this source is, as a [`Folder`].
    pub fn as_folder(&self) -> &Folder {
        &self.folder
    }

    /// The scheme and host, as in `https://huggingface.co`.
    pub fn base(&self) -> &str {
        self.folder.base()
    }

    /// The repository, as in `vegidio/general`.
    pub fn repo(&self) -> &str {
        self.folder.repo()
    }

    /// The branch, tag or commit the URL names, as in `main`.
    pub fn revision(&self) -> &str {
        self.folder.revision()
    }

    /// The folder inside the repository, as in `deps/cuda/13.3.0`.
    pub fn folder(&self) -> &str {
        self.folder.path()
    }

    /// The dependency's name: the folder's next-to-last segment, as in `cuda`.
    pub fn name(&self) -> &str {
        self.segment_from_end(1)
    }

    /// The dependency's version: the folder's last segment, as in `13.3.0`.
    pub fn version(&self) -> &str {
        self.segment_from_end(0)
    }

    fn segment_from_end(&self, n: usize) -> &str {
        self.folder().rsplit('/').nth(n).unwrap_or_default()
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.folder.fmt(f)
    }
}

/// The one archive in `listing` for `os` and `arch`, as `std::env::consts` spells them: the file named
/// `*_<macos|linux|windows>_<arm64|x64>.7z`. `None` when the platform has no archive there, or more than one.
pub fn asset_for<'a>(listing: &'a [RemoteFile], os: &str, arch: &str) -> Option<&'a RemoteFile> {
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return None,
    };
    if !matches!(os, "macos" | "linux" | "windows") {
        return None;
    }

    let suffix = format!("_{os}_{arch}.7z");
    let mut matching = listing.iter().filter(|file| file.file_name().ends_with(&suffix));
    match (matching.next(), matching.next()) {
        (Some(file), None) => Some(file),
        (Some(_), Some(_)) => {
            tracing::warn!("more than one archive for {os}/{arch} in the folder; using none of them");
            None
        }
        (None, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::hf::{Hash, parse_listing};

    #[test]
    fn a_folder_url_yields_its_repo_revision_and_folder() {
        let source = Source::parse("https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0").unwrap();
        assert_eq!(source.base(), "https://huggingface.co");
        assert_eq!(source.repo(), "vegidio/general");
        assert_eq!(source.revision(), "main");
        assert_eq!(source.folder(), "deps/cuda/13.3.0");
        assert_eq!((source.name(), source.version()), ("cuda", "13.3.0"));
        assert_eq!(
            source.to_string(),
            "https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0"
        );
    }

    #[test]
    fn a_future_version_parses_alike() {
        let source = Source::parse("https://huggingface.co/vegidio/general/tree/main/deps/cuda/14.0.0/").unwrap();
        assert_eq!(
            (source.folder(), source.name(), source.version()),
            ("deps/cuda/14.0.0", "cuda", "14.0.0")
        );
    }

    #[test]
    fn a_local_server_keeps_its_port() {
        let source = Source::parse("http://127.0.0.1:8080/test/repo/tree/main/onnx/1.30.0").unwrap();
        assert_eq!(source.base(), "http://127.0.0.1:8080");
        assert_eq!((source.name(), source.version()), ("onnx", "1.30.0"));
    }

    #[test]
    fn anything_but_a_tree_url_with_a_name_and_version_is_refused() {
        for url in [
            "not a url",
            "ftp://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0",
            "https://huggingface.co/vegidio/general/resolve/main/deps/cuda/13.3.0",
            "https://huggingface.co/vegidio/general/blob/main/deps/cuda/13.3.0",
            "https://huggingface.co/vegidio/general/tree/main",
            "https://huggingface.co/vegidio/general/tree/main/cuda",
            "https://huggingface.co/vegidio/general/tree/main/deps/../13.3.0",
            "https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0?x=1",
            "https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0#top",
            "https://huggingface.co/vegidio/tree/main/deps/cuda/13.3.0",
        ] {
            let error = Source::parse(url).expect_err(url);
            assert_eq!(error.url, url);
        }
    }

    #[test]
    fn a_folder_of_one_segment_or_more_parses() {
        let models = Folder::parse("https://huggingface.co/vegidio/open-photo-ai/tree/main/models").unwrap();
        assert_eq!(
            (models.base(), models.repo(), models.revision(), models.path()),
            ("https://huggingface.co", "vegidio/open-photo-ai", "main", "models")
        );
        assert_eq!(
            models.to_string(),
            "https://huggingface.co/vegidio/open-photo-ai/tree/main/models"
        );
        let nested = Folder::parse("https://huggingface.co/vegidio/general/tree/main/models/insightface/").unwrap();
        assert_eq!(nested.path(), "models/insightface");
        assert_eq!(
            Source::parse("https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0")
                .unwrap()
                .as_folder()
                .path(),
            "deps/cuda/13.3.0"
        );
    }

    #[test]
    fn a_folder_url_with_no_folder_is_refused() {
        for url in [
            "not a url",
            "https://huggingface.co/vegidio/general/tree/main",
            "https://huggingface.co/vegidio/general/tree/main/",
            "https://huggingface.co/vegidio/general/blob/main/models",
            "https://huggingface.co/vegidio/general/tree/main/models/..",
            "https://huggingface.co/vegidio/general/tree/main/models?x=1",
        ] {
            assert_eq!(Folder::parse(url).expect_err(url).url, url);
        }
    }

    /// The listing of `deps/onnx/1.30.0` as the API returned it while this was written.
    fn onnx_listing() -> Vec<RemoteFile> {
        parse_listing(include_str!("testdata/onnx_1.30.0.json")).unwrap()
    }

    #[test]
    fn the_archive_for_the_platform_is_picked_from_the_real_listing() {
        let listing = onnx_listing();
        let asset = asset_for(&listing, "macos", "aarch64").unwrap();
        assert_eq!(asset.path, "deps/onnx/1.30.0/onnx_macos_arm64.7z");
        assert_eq!(asset.size, 10_043_605);
        assert_eq!(
            asset.hash,
            Hash::Sha256("f58971f0f556567e9b7fcce6a0110c5072ff003d9a1bb1214060b188f5d11178".into())
        );
        for (os, arch, name) in [
            ("linux", "aarch64", "onnx_linux_arm64.7z"),
            ("linux", "x86_64", "onnx_linux_x64.7z"),
            ("windows", "aarch64", "onnx_windows_arm64.7z"),
            ("windows", "x86_64", "onnx_windows_x64.7z"),
        ] {
            assert_eq!(asset_for(&listing, os, arch).map(RemoteFile::file_name), Some(name));
        }
    }

    #[test]
    fn a_platform_with_no_archive_is_unsupported() {
        assert_eq!(
            asset_for(&onnx_listing(), "macos", "x86_64"),
            None,
            "no Intel Mac archive"
        );
        assert_eq!(asset_for(&onnx_listing(), "freebsd", "x86_64"), None);
        assert_eq!(asset_for(&onnx_listing(), "linux", "riscv64"), None);

        // A GPU folder publishes nothing for macOS.
        let cuda = parse_listing(
            r#"[{"type":"file","oid":"1","size":1,"lfs":{"oid":"aa","size":1},"path":"deps/cuda/13.3.0/cuda_linux_x64.7z"},
                {"type":"file","oid":"2","size":1,"lfs":{"oid":"bb","size":1},"path":"deps/cuda/13.3.0/cuda_windows_x64.7z"}]"#,
        )
        .unwrap();
        assert_eq!(asset_for(&cuda, "macos", "aarch64"), None);
        assert_eq!(asset_for(&cuda, "windows", "aarch64"), None);
        assert!(asset_for(&cuda, "windows", "x86_64").is_some());
    }
}
