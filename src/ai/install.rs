//! The installer: each dependency's archive found in its folder's listing, downloaded to staging and verified against
//! the API's hash, extracted there, and moved into the dependency's folder in place of whatever was there, the marker
//! written last.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::{self, UnboundedSender};

use super::hf::{self, Hash, HfError, Hub, RemoteFile};
use super::progress::{Event, Fraction};
use super::{Failure, InstallError, Source, asset_for, dirs, runtime_library};
use crate::fetch::{DigestAlgorithm, Download, DownloadError, Fetch, RequestOptions};
use crate::fs::ExtractOptions;

/// The install marker, in the dependency's folder: the archive's verified hash.
pub(super) const MARKER: &str = ".installed";

/// The staging folder, in the dependency's folder.
const STAGING: &str = ".staging";

/// The extraction folder, in staging.
const EXTRACT: &str = "extract";

/// What a dependency's folder holds, which decides what counts as installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The ONNX Runtime. Installed only while its library is in the folder too, and a platform its folder has no
    /// archive for fails the install.
    Runtime,
    /// Libraries no one file stands for, such as CUDA: the marker alone decides. A platform the folder has no archive
    /// for is skipped, with nothing installed.
    Libraries,
}

/// One dependency to install: its display name, which events and errors carry, and its Hugging Face folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The name events and errors carry, as in `NVIDIA CUDA`.
    pub name: &'static str,
    /// The folder its archives are in.
    pub source: Source,
    /// What its folder holds.
    pub kind: Kind,
}

impl Dependency {
    /// The ONNX Runtime, named `name`, from `source`.
    pub fn runtime(name: &'static str, source: Source) -> Self {
        Self {
            name,
            source,
            kind: Kind::Runtime,
        }
    }

    /// Libraries, such as CUDA, named `name`, from `source`.
    pub fn libraries(name: &'static str, source: Source) -> Self {
        Self {
            name,
            source,
            kind: Kind::Libraries,
        }
    }
}

/// Whether `dependency` has been installed under `root`: its marker exists and, for the runtime, its library is a file
/// in its folder. Local only: it does not ask whether the install is still current. For deciding whether to load.
pub fn is_present(root: &Path, dependency: &Dependency) -> bool {
    present_in(&dirs::dir_for(root, &dependency.source), dependency.kind)
}

fn present_in(dir: &Path, kind: Kind) -> bool {
    marker(dir).is_some() && (kind == Kind::Libraries || runtime_library(dir).is_some())
}

/// The hash the marker in `dir` holds, if there is one.
fn marker(dir: &Path) -> Option<String> {
    let marker = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let marker = marker.trim();
    (!marker.is_empty()).then(|| marker.to_owned())
}

/// Install every dependency in `dependencies` that is missing or out of date under `root`, one at a time, in order,
/// each into [`dirs::dir_for`]`(root, source)`.
///
/// Each source's revision is resolved to its commit once per repository, and everything of that repository is listed
/// and downloaded at that commit. A dependency is current when its marker holds the hash the API reports now for its
/// archive (and, for the runtime, its library is in place); it is skipped, reporting nothing. When the API cannot be
/// reached, a dependency that [`is_present`] is used as it is, with a warning. Progress goes to `on_event`.
///
/// Dropping the future stops the install. A download keeps its partial to resume from; an extraction that has begun
/// runs to its end on its blocking thread, so a folder's marker never stands over an incomplete install.
///
/// Needs a Tokio runtime, as [`fetch`](crate::fetch) does.
///
/// # Errors
///
/// [`InstallError`] for the first dependency that could not be installed, naming it. The ones before it stay
/// installed, and the ones after it are not tried.
pub async fn install(
    root: &Path,
    dependencies: &[Dependency],
    on_event: impl FnMut(Event) + Send,
) -> Result<(), InstallError> {
    install_retrying(root, dependencies, hf::RETRIES, on_event).await
}

pub(super) async fn install_retrying(
    root: &Path,
    dependencies: &[Dependency],
    retries: u32,
    mut on_event: impl FnMut(Event) + Send,
) -> Result<(), InstallError> {
    let mut commits: HashMap<(&str, &str, &str), Result<String, HfError>> = HashMap::new();
    for dependency in dependencies {
        let source = &dependency.source;
        let hub = Hub::for_source(source).retries(retries);
        let repo = (source.base(), source.repo(), source.revision());
        let commit = match commits.get(&repo) {
            Some(commit) => commit.clone(),
            None => {
                let commit = hub.resolve(source.revision()).await;
                commits.insert(repo, commit.clone());
                commit
            }
        };
        let dir = dirs::dir_for(root, source);
        install_one(&hub, commit, &dir, dependency, &mut on_event)
            .await
            .map_err(|failure| InstallError {
                dependency: dependency.name,
                failure,
            })?;
    }
    Ok(())
}

/// Install `dependency` into `dir` from `hub` at `commit`, unless it is current there.
async fn install_one(
    hub: &Hub,
    commit: Result<String, HfError>,
    dir: &Path,
    dependency: &Dependency,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<(), Failure> {
    let offline = |error: HfError| {
        if error.is_unreachable() && present_in(dir, dependency.kind) {
            tracing::warn!(
                "Hugging Face could not be reached ({error}); using {} as installed at {}",
                dependency.name,
                dir.display()
            );
            Ok(())
        } else {
            Err(Failure::from(error))
        }
    };
    let commit = match commit {
        Ok(commit) => commit,
        Err(error) => return offline(error),
    };
    let listing = match hub.list(&commit, dependency.source.folder()).await {
        Ok(listing) => listing,
        Err(error) => return offline(error),
    };

    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let Some(asset) = asset_for(&listing, os, arch) else {
        return match dependency.kind {
            Kind::Runtime => Err(Failure::UnsupportedPlatform { os, arch }),
            Kind::Libraries => {
                tracing::info!("{} has no archive for {os}/{arch}; skipping it", dependency.name);
                Ok(())
            }
        };
    };
    if marker(dir).is_some_and(|marker| asset.hash.matches(&marker)) && present_in(dir, dependency.kind) {
        return Ok(());
    }

    let name = dependency.name;
    let staging = dir.join(STAGING);
    std::fs::create_dir_all(&staging)?;
    let staged = staging.join(asset.file_name());
    let mut fraction = Fraction::default();

    let mut notify = |step: Step| match step {
        Step::OnDisk(bytes) => on_event(Event::Progress {
            dependency: name,
            fraction: fraction.download(bytes, asset.size),
        }),
        Step::Verifying => on_event(Event::Verifying { dependency: name }),
    };
    fetch_verified(hub.fetch(), hub.url(&commit, &asset.path), &staged, asset, &mut notify).await?;

    // One closure, so a dropped install, which cannot interrupt it, never stops it half-way through replacing the
    // folder's contents. Its progress comes back over a channel, which closes when it ends.
    let (sender, mut extracted) = mpsc::unbounded_channel();
    let (unpacking, marker) = (dir.to_path_buf(), asset.hash.hex().to_owned());
    let task = tokio::task::spawn_blocking(move || unpack(&unpacking, &staged, &marker, sender));
    while let Some((done, total)) = extracted.recv().await {
        on_event(Event::Progress {
            dependency: name,
            fraction: fraction.extract(done, total),
        });
    }
    task.await.map_err(|cause| Failure::Io(cause.to_string()))??;

    if let Err(cause) = std::fs::remove_dir_all(&staging) {
        tracing::warn!("could not remove {}: {cause}", staging.display());
    }
    on_event(Event::Progress {
        dependency: name,
        fraction: fraction.finish(),
    });
    tracing::info!("installed {name} at {}", dir.display());
    Ok(())
}

/// What a transfer tells its caller.
enum Step {
    /// This many bytes of the file are on disk, a resumed partial included.
    OnDisk(u64),
    /// A complete file found in staging is being hashed.
    Verifying,
}

/// Download `url` to `staged` and check it against `file`'s hash. A file found already complete in staging is
/// re-hashed (reporting `Verifying`), and downloaded again once if it does not match. A file that still does not match
/// is deleted, and the install fails as damaged.
async fn fetch_verified(
    fetch: &Fetch,
    url: String,
    staged: &Path,
    file: &RemoteFile,
    notify: &mut (dyn FnMut(Step) + Send),
) -> Result<(), Failure> {
    let (mut digest, was_staged) = fetch_hashed(fetch, url.clone(), staged, &file.hash, notify).await?;
    if was_staged && !file.hash.matches(&digest) {
        // Left complete in staging by an earlier run, perhaps of another upload, and never downloaded by this one: it
        // is downloaded again once rather than failing the install.
        tracing::warn!(
            "the staged {} does not match the API's hash; downloading it again",
            file.path
        );
        std::fs::remove_file(staged)?;
        (digest, _) = fetch_hashed(fetch, url, staged, &file.hash, notify).await?;
    }
    if !file.hash.matches(&digest) {
        tracing::error!(
            "{} hashed to {digest}, not the API's {}; deleting it",
            file.path,
            file.hash.hex()
        );
        std::fs::remove_file(staged)?;
        return Err(Failure::Damaged);
    }
    Ok(())
}

/// Download `url` to `staged`, and return its hash, of `expected`'s kind, and whether it was already complete in
/// staging, so the transfer did not download it.
async fn fetch_hashed(
    fetch: &Fetch,
    url: String,
    staged: &Path,
    expected: &Hash,
    notify: &mut (dyn FnMut(Step) + Send),
) -> Result<(String, bool), Failure> {
    let was_staged = std::fs::metadata(staged).is_ok_and(|meta| meta.is_file());
    // The transfer's first update comes only once the response's headers land, so the bytes already on disk are
    // reported first, counted from the start.
    notify(Step::OnDisk(already_on_disk(staged)));

    // The API's hash identifies the partial: a partial of another upload is discarded rather than resumed, and one of
    // this upload is resumed even after the repository moved on to a new commit, which moves the URL.
    let mut options = RequestOptions::new()
        .resume_key(expected.hex())
        .resume_across_urls(true);
    if let Hash::Sha256(_) = expected {
        options = options.digest(DigestAlgorithm::Sha256);
    }
    let mut download = Cancelling(fetch.download_with_options(url, staged, options));
    if let Err(cause) = download
        .0
        .track(|_, downloaded, _| notify(Step::OnDisk(downloaded)))
        .await
    {
        tracing::error!("downloading {} failed: {cause}", staged.display());
        return Err(download_failure(cause));
    }
    if let Some(digest) = download.0.digest() {
        return Ok((digest, false));
    }

    if was_staged {
        notify(Step::Verifying);
    }
    let (path, expected) = (staged.to_path_buf(), expected.clone());
    let hash = tokio::task::spawn_blocking(move || expected.of_file(path))
        .await
        .map_err(|cause| Failure::Io(cause.to_string()))??;
    Ok((hash.hex().to_owned(), was_staged))
}

/// A download that is cancelled when dropped, as it is when the install is: `fetch`'s own handle leaves its task
/// running.
struct Cancelling(Download);

impl Drop for Cancelling {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// The bytes `fetch` will find for `path`: the complete file, or else its `.part`.
fn already_on_disk(path: &Path) -> u64 {
    let len = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).ok();
    len(path).or_else(|| len(&part_path(path))).unwrap_or(0)
}

/// Where `fetch` keeps the partial of `path`.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// What a failed download means.
fn download_failure(cause: DownloadError) -> Failure {
    match cause {
        DownloadError::Http(error) => match error.status() {
            Some(status) => Failure::Http(status.as_u16()),
            None => Failure::Network,
        },
        DownloadError::Io(error) => error.into(),
        // Only when the install is dropped, and then nobody reads it.
        DownloadError::Cancelled => Failure::Network,
    }
}

/// Extract the verified archive at `staged` into staging, then replace `dir`'s contents with it: the marker deleted
/// first, every other entry but staging removed, the extracted entries moved in, then the marker, holding `hash`,
/// written. The extraction's `(done, total)` bytes go to `progress`.
fn unpack(dir: &Path, staged: &Path, hash: &str, progress: UnboundedSender<(u64, u64)>) -> Result<(), Failure> {
    let extract = dir.join(STAGING).join(EXTRACT);
    // Left by an attempt that stopped while extracting.
    remove_if_present(&extract, true)?;
    let options = ExtractOptions::new().on_progress(move |extracted| {
        // An archive that declares no total holds the fraction at the download's share until the end.
        if let Some(total) = extracted.total_bytes {
            let _ = progress.send((extracted.bytes, total));
        }
    });
    crate::fs::extract(staged, &extract, &options).inspect_err(|cause| {
        tracing::error!("extracting {} failed: {cause}", staged.display());
    })?;

    let marker = dir.join(MARKER);
    remove_if_present(&marker, false)?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() != STAGING {
            remove_if_present(&entry.path(), entry.file_type()?.is_dir())?;
        }
    }
    for entry in std::fs::read_dir(&extract)? {
        let entry = entry?;
        std::fs::rename(entry.path(), dir.join(entry.file_name()))?;
    }

    // Written whole and renamed into place, so it is never seen half-written.
    let pending = dir.join(format!("{MARKER}.tmp"));
    std::fs::write(&pending, hash)?;
    std::fs::rename(&pending, &marker)?;
    Ok(())
}

/// Remove the folder (`dir`) or file at `path`, if there is one.
fn remove_if_present(path: &Path, dir: bool) -> io::Result<()> {
    let removed = if dir {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Err(cause) if cause.kind() != io::ErrorKind::NotFound => Err(cause),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
