//! The installer: each dependency checked locally first, and only one that is missing or damaged looked up on the
//! API, its archive downloaded to staging and verified against the API's hash, extracted there, and moved into the
//! dependency's folder in place of whatever was there, the marker written last. The folder is locked while it is
//! installed into, and once it is in place the dependency's other versions are removed.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::{self, UnboundedSender};

use super::hf::{self, Hash, Hub, RemoteFile};
use super::progress::{Event, Fraction, Phase};
use super::{Failure, InstallError, Source, asset_for, dirs, runtime_library};
use crate::fetch::{DigestAlgorithm, Download, DownloadError, Fetch, RequestOptions};
use crate::fs::ExtractOptions;

mod lock;
mod marker;

pub(super) use marker::MARKER;
use marker::{LOCK, Marker, STAGING};

/// The extraction folder, in staging.
const EXTRACT: &str = "extract";

/// What a dependency's folder holds, which decides what counts as installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The ONNX Runtime. Installed only while its library is in the folder too, and a platform its folder has no
    /// archive for fails the install.
    Runtime,
    /// Libraries no one file stands for, such as CUDA: the recorded files alone decide. A platform the folder has no
    /// archive for is skipped, with nothing installed.
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

/// Whether `dependency` is installed and intact under `root`: its marker is there, every file it records is present
/// at its recorded size, and, for the runtime, its library is a file in its folder. Local only, with no network
/// request.
pub fn is_present(root: &Path, dependency: &Dependency) -> bool {
    intact_in(&dirs::dir_for(root, &dependency.source), dependency.kind).is_some()
}

/// The marker of an intact install of a dependency of `kind` in `dir`.
fn intact_in(dir: &Path, kind: Kind) -> Option<Marker> {
    Marker::read(dir)
        .filter(|marker| marker.intact(dir))
        .filter(|_| kind == Kind::Libraries || runtime_library(dir).is_some())
}

/// Where a dependency stands before anything is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Installed and intact: nothing will be done for it.
    Present,
    /// Missing, or damaged: it will be downloaded and installed.
    Missing,
    /// Libraries whose folder has no archive for this platform: they are skipped.
    Unavailable,
}

/// One dependency's row of a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// The dependency's name, as the caller gave it.
    pub name: &'static str,
    /// Where it stands.
    pub state: State,
    /// Its archive's size in bytes: from its marker when it is present, from the API's listing when it is missing, and
    /// 0 when it is unavailable.
    pub size: u64,
}

/// What [`install`] will do for each dependency, in order, decided before anything moves. [`Plan::install`] carries
/// it out, reusing the listings the plan was made from.
#[derive(Debug)]
pub struct Plan {
    root: PathBuf,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    dependency: Dependency,
    planned: Planned,
    /// Where a missing dependency's archive is, at the commit the plan resolved.
    remote: Option<Remote>,
}

#[derive(Debug)]
struct Remote {
    hub: Hub,
    commit: String,
    asset: RemoteFile,
}

impl Plan {
    /// Each dependency's row, in the order given.
    pub fn dependencies(&self) -> impl Iterator<Item = &Planned> {
        self.entries.iter().map(|entry| &entry.planned)
    }

    /// Install every missing dependency, one at a time, in order, reporting [`Event`]s to `on_event`: an
    /// [`Event::Current`] for each present one, and progress for each one installed.
    ///
    /// Each folder is locked for the whole of its download, verification and extraction. One that another process
    /// is installing into is waited for and then checked again, and an install that process finished is used rather
    /// than repeated. Once a dependency's marker is written, its other version folders are removed, except one that
    /// another process holds locked.
    ///
    /// Dropping the future stops the install and releases the lock. A download keeps its partial to resume from; an
    /// extraction that has begun runs to its end on its blocking thread, so a folder's marker never stands over an
    /// incomplete install.
    ///
    /// # Errors
    ///
    /// [`InstallError`] for the first dependency that could not be installed, naming it. The ones before it stay
    /// installed, and the ones after it are not tried.
    pub async fn install(self, mut on_event: impl FnMut(Event) + Send) -> Result<(), InstallError> {
        for entry in &self.entries {
            let name = entry.dependency.name;
            let dir = dirs::dir_for(&self.root, &entry.dependency.source);
            match (&entry.remote, entry.planned.state) {
                (_, State::Present) => on_event(Event::Current { dependency: name }),
                (Some(remote), State::Missing) => install_one(remote, &dir, &entry.dependency, &mut on_event)
                    .await
                    .map_err(|failure| InstallError {
                        dependency: name,
                        failure,
                    })?,
                _ => {}
            }
        }
        Ok(())
    }
}

/// Decide what [`Plan::install`] will do for each of `dependencies` under `root`, each in
/// [`dirs::dir_for`]`(root, source)`.
///
/// A dependency that is installed and intact is decided locally, with no network request, so a launch on which every
/// dependency is installed makes none. For any other, its source's revision is resolved to its commit, once per
/// repository, and its folder is listed at that commit, which [`Plan::install`] then downloads from.
///
/// Needs a Tokio runtime, as [`fetch`](crate::fetch) does.
///
/// # Errors
///
/// [`InstallError`] naming the first missing dependency whose folder could not be listed, or the runtime when its
/// folder has no archive for this platform.
pub async fn plan(root: &Path, dependencies: &[Dependency]) -> Result<Plan, InstallError> {
    plan_retrying(root, dependencies, hf::RETRIES).await
}

pub(super) async fn plan_retrying(
    root: &Path,
    dependencies: &[Dependency],
    retries: u32,
) -> Result<Plan, InstallError> {
    let mut commits: HashMap<(&str, &str, &str), String> = HashMap::new();
    let mut entries = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let name = dependency.name;
        let failed = |failure: Failure| InstallError {
            dependency: name,
            failure,
        };
        let dir = dirs::dir_for(root, &dependency.source);
        if let Some(marker) = intact_in(&dir, dependency.kind) {
            entries.push(Entry {
                dependency: dependency.clone(),
                planned: Planned {
                    name,
                    state: State::Present,
                    size: marker.size,
                },
                remote: None,
            });
            continue;
        }

        let source = &dependency.source;
        let hub = Hub::for_source(source).retries(retries);
        let repo = (source.base(), source.repo(), source.revision());
        let commit = match commits.get(&repo) {
            Some(commit) => commit.clone(),
            None => {
                let commit = hub
                    .resolve(source.revision())
                    .await
                    .map_err(|error| failed(error.into()))?;
                commits.insert(repo, commit.clone());
                commit
            }
        };
        let listing = hub
            .list(&commit, source.folder())
            .await
            .map_err(|error| failed(error.into()))?;

        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        let remote = match asset_for(&listing, os, arch) {
            Some(asset) => Some(Remote {
                hub,
                commit,
                asset: asset.clone(),
            }),
            None if dependency.kind == Kind::Runtime => {
                return Err(failed(Failure::UnsupportedPlatform { os, arch }));
            }
            None => {
                tracing::info!("{name} has no archive for {os}/{arch}; skipping it");
                None
            }
        };
        entries.push(Entry {
            dependency: dependency.clone(),
            planned: Planned {
                name,
                state: if remote.is_some() {
                    State::Missing
                } else {
                    State::Unavailable
                },
                size: remote.as_ref().map_or(0, |remote| remote.asset.size),
            },
            remote,
        });
    }
    Ok(Plan {
        root: root.to_path_buf(),
        entries,
    })
}

/// [`plan`], then [`Plan::install`]: install every dependency in `dependencies` that is missing or damaged under
/// `root`, one at a time, in order, reporting [`Event`]s to `on_event`.
///
/// Needs a Tokio runtime, as [`fetch`](crate::fetch) does.
///
/// # Errors
///
/// [`InstallError`] for the first dependency that could not be planned or installed, naming it.
pub async fn install(
    root: &Path,
    dependencies: &[Dependency],
    on_event: impl FnMut(Event) + Send,
) -> Result<(), InstallError> {
    plan(root, dependencies).await?.install(on_event).await
}

/// Install `dependency` into `dir` from `remote`, under the folder's lock, unless another process installed it while
/// this one waited.
async fn install_one(
    remote: &Remote,
    dir: &Path,
    dependency: &Dependency,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<(), Failure> {
    let name = dependency.name;
    let held = lock::acquire(dir).await?;
    if intact_in(dir, dependency.kind).is_some() {
        on_event(Event::Current { dependency: name });
        return Ok(());
    }

    let asset = &remote.asset;
    let staging = dir.join(STAGING);
    std::fs::create_dir_all(&staging)?;
    let staged = staging.join(asset.file_name());
    let mut fraction = Fraction::default();

    let mut notify = |step: Step| match step {
        Step::OnDisk(bytes) => on_event(Event::Progress {
            dependency: name,
            phase: Phase::Download,
            bytes,
            total: asset.size,
            fraction: fraction.download(bytes, asset.size),
        }),
        Step::Verifying => on_event(Event::Verifying { dependency: name }),
    };
    fetch_verified(
        remote.hub.fetch(),
        remote.hub.url(&remote.commit, &asset.path),
        &staged,
        asset,
        &mut notify,
    )
    .await?;

    // One closure, so a dropped install, which cannot interrupt it, never stops it half-way through replacing the
    // folder's contents. Its progress comes back over a channel, which closes when it ends.
    let (sender, mut extracted) = mpsc::unbounded_channel();
    let (unpacking, hash, size) = (dir.to_path_buf(), asset.hash.hex().to_owned(), asset.size);
    let task = tokio::task::spawn_blocking(move || unpack(&unpacking, &staged, &hash, size, sender));
    let mut extract_total = 0;
    while let Some((done, total)) = extracted.recv().await {
        extract_total = total;
        on_event(Event::Progress {
            dependency: name,
            phase: Phase::Extract,
            bytes: done,
            total,
            fraction: fraction.extract(done, total),
        });
    }
    task.await.map_err(|cause| Failure::Io(cause.to_string()))??;

    if let Err(cause) = std::fs::remove_dir_all(&staging) {
        tracing::warn!("could not remove {}: {cause}", staging.display());
    }
    remove_other_versions(dir);
    drop(held);
    on_event(Event::Progress {
        dependency: name,
        phase: Phase::Extract,
        bytes: extract_total,
        total: extract_total,
        fraction: fraction.finish(),
    });
    tracing::info!("installed {name} at {}", dir.display());
    Ok(())
}

/// Remove every folder beside `dir`, the dependency's other versions, except one another process holds locked. Best
/// effort: a folder that cannot be removed is logged and left.
fn remove_other_versions(dir: &Path) {
    let Some(versions) = dir.parent() else { return };
    let Ok(entries) = std::fs::read_dir(versions) else {
        return;
    };
    for entry in entries.flatten() {
        let other = entry.path();
        if other == dir || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        match lock::try_acquire(&other) {
            Ok(Some(_held)) => {
                // The marker first, so a removal that stops part-way never leaves a folder that reads as installed.
                // The lock file goes with the folder while it is held, so no one is left waiting on a lock no one
                // else can see.
                let removed =
                    remove_if_present(&other.join(MARKER), false).and_then(|()| std::fs::remove_dir_all(&other));
                match removed {
                    Ok(()) => tracing::info!("removed {}, replaced by {}", other.display(), dir.display()),
                    Err(cause) => tracing::warn!("could not remove {}: {cause}", other.display()),
                }
            }
            Ok(None) => tracing::info!("{} is being installed by another process; leaving it", other.display()),
            Err(cause) => tracing::warn!("could not lock {} to remove it: {cause}", other.display()),
        }
    }
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
/// first, every other entry but the lock and staging removed, the extracted entries moved in, then the marker,
/// recording `hash`, `size` and every entry, written. The extraction's `(done, total)` bytes go to `progress`.
fn unpack(
    dir: &Path,
    staged: &Path,
    hash: &str,
    size: u64,
    progress: UnboundedSender<(u64, u64)>,
) -> Result<(), Failure> {
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

    remove_if_present(&dir.join(MARKER), false)?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != STAGING && name != LOCK {
            remove_if_present(&entry.path(), entry.file_type()?.is_dir())?;
        }
    }
    for entry in std::fs::read_dir(&extract)? {
        let entry = entry?;
        std::fs::rename(entry.path(), dir.join(entry.file_name()))?;
    }

    Marker::record(dir, hash, size)?.write(dir)?;
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
