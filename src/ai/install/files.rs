//! The file installer: the files of a Hugging Face folder a selection names, installed into a folder the caller
//! chooses, verified against the hashes of a listing the caller read once, and replaced when the listing names other
//! bytes at the same path.
//!
//! Unlike an archive's version folder, which only ever holds one version, a file here may be republished at the same
//! path, so an install is current only while its record matches the listing. A file already in the folder at its
//! listed size is hashed and kept when it matches, rather than downloaded again.

use std::borrow::Cow;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::marker::{MARKER, STAGING, is_own};
use super::{Step, already_on_disk, fetch_hashed, lock, part_path, remove_if_present};
use crate::ai::hf::{self, Hub, Listing, RemoteFile};
use crate::ai::progress::{Event, Fraction, Phase};
use crate::ai::{Failure, Folder, InstallError};

/// One install of files from a Hugging Face folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    /// The name events and errors carry, as in `up_kyoto_4x_fp32`.
    pub name: Cow<'static, str>,
    /// The folder the files are in.
    pub folder: Folder,
    /// Which of the folder's files to install.
    pub select: Select,
    /// The folder they install into, created if missing. The install owns it: anything in it that is not a selected
    /// file is removed when the files are installed.
    pub dir: PathBuf,
    /// Folders computed from the files, such as an execution provider's compiled model. Each is emptied, the folder
    /// itself kept, by an install that does not find the files current, and left alone otherwise.
    pub derived: Vec<PathBuf>,
    /// Whether files already in `dir` are used as they are.
    pub trust: Trust,
}

/// Which of a folder's files an install takes, by file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Select {
    /// Exactly these, every one of which the listing must name.
    Names(Vec<String>),
    /// Every file whose name starts with this, as `x.onnx` takes `x.onnx` and `x.onnx.data`.
    Prefix(String),
}

impl Select {
    /// Whether the file named `name` is selected.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Select::Names(names) => names.iter().any(|selected| selected == name),
            Select::Prefix(prefix) => name.starts_with(prefix.as_str()),
        }
    }
}

/// Whether an install checks the files against the listing, or uses the ones already in its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trust {
    /// Every file is the listing's, verified by its hash.
    #[default]
    Published,
    /// A folder holding every selected file is used as it is, with no hash and no request. Its identity is a stamp over
    /// each file's name, size and modification time, so editing or replacing one empties the derived folders once. An
    /// install without this option never reads such a folder as current. A folder missing a file installs as under
    /// [`Trust::Published`].
    LocalFiles,
}

/// What [`install_files`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// At least one file was downloaded.
    Installed,
    /// The recorded files are the listing's and still on disk, so nothing was done.
    Current,
    /// Every file was already in the folder and matched the listing, so it was recorded with nothing downloaded.
    Adopted,
    /// No listing was given and the recorded files are on disk, so they are used as they are.
    Offline,
    /// [`Trust::LocalFiles`], and every selected file was in the folder, so it is used as it is.
    Trusted,
}

/// What a file install left in its folder, in the same `.installed` as an archive's, of another shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    /// The listing's files, verified.
    Files { files: Vec<Recorded> },
    /// Files used as they were found, by their stamp.
    Trusted { stamp: String },
}

/// One installed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Recorded {
    /// Its name in the folder.
    path: String,
    /// Its size in bytes.
    size: u64,
    /// The listing's hash it was verified against, in hex.
    hash: String,
}

impl Record {
    /// The record in `dir`, if there is one of this shape. An archive's marker, or an earlier release's file, reads as
    /// none.
    fn read(dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join(MARKER)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write the record into `dir`, whole: it is written beside and renamed into place.
    fn write(&self, dir: &Path) -> io::Result<()> {
        let pending = dir.join(format!("{MARKER}.tmp"));
        std::fs::write(&pending, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        std::fs::rename(&pending, dir.join(MARKER))
    }

    /// The recorded files, when this is a record of verified files that are all still in `dir` at their recorded
    /// sizes.
    fn intact(&self, dir: &Path) -> Option<&[Recorded]> {
        match self {
            Record::Files { files }
                if !files.is_empty() && files.iter().all(|file| present_at(dir, &file.path, file.size)) =>
            {
                Some(files)
            }
            _ => None,
        }
    }
}

/// Whether `files` has an intact install in its folder: a record of verified files, every one of them still there at
/// its recorded size. Local only, with no hash and no request: what [`install_files`] uses with no listing.
pub fn files_present(files: &Files) -> bool {
    Record::read(&files.dir).is_some_and(|record| record.intact(&files.dir).is_some())
}

/// Install `files` into `files.dir`, against `listing`, a listing of `files.folder` the caller read with
/// [`Listing::read`] and may pass to every install in that folder. Reports [`Event`]s to `on_event`: progress over the
/// sum of the files' sizes, and [`Event::Verifying`] while a file found in the folder is hashed. Nothing is reported
/// for an install that turns out current, offline or trusted.
///
/// Under the folder's lock:
///
/// - With a listing, the install is [`Outcome::Current`] when its record names exactly the selected files, each at the
///   listing's hash, and each is still on disk at its recorded size. Otherwise the record is deleted, the derived
///   folders emptied, and each file kept when it is already there at its listed size and hashes right (or its record
///   already said so), or else downloaded into `.staging/`, resuming a partial, verified and moved in. A file that does
///   not match after a resume is downloaded once more from nothing. Anything else in the folder is then removed, and
///   the record written last.
/// - With none, an intact install is [`Outcome::Offline`], and anything else fails as [`Failure::Unlisted`].
/// - Under [`Trust::LocalFiles`], a folder holding every selected file is [`Outcome::Trusted`] before either.
///
/// Dropping the future stops the install and releases the lock. A download keeps its partial to resume from.
///
/// Needs a Tokio runtime, as [`fetch`](crate::fetch) does.
///
/// # Errors
///
/// [`InstallError`] naming `files.name`: [`Failure::NotListed`] when the listing names no selected file, or not every
/// named one, [`Failure::Unlisted`] with no listing and no intact install, [`Failure::Damaged`] for a download that
/// does not match its hash, [`Failure::Malformed`] for a listing of another folder, and the transfer and file failures
/// of any install.
pub async fn install_files(
    files: &Files,
    listing: Option<&Listing>,
    on_event: impl FnMut(Event) + Send,
) -> Result<Outcome, InstallError> {
    install_files_retrying(files, listing, hf::RETRIES, on_event).await
}

pub(in crate::ai) async fn install_files_retrying(
    files: &Files,
    listing: Option<&Listing>,
    retries: u32,
    mut on_event: impl FnMut(Event) + Send,
) -> Result<Outcome, InstallError> {
    run(files, listing, retries, &mut on_event)
        .await
        .map_err(|failure| InstallError {
            dependency: files.name.clone(),
            failure,
        })
}

async fn run(
    files: &Files,
    listing: Option<&Listing>,
    retries: u32,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<Outcome, Failure> {
    if let Some(listing) = listing
        && listing.folder() != &files.folder
    {
        return Err(Failure::Malformed(format!(
            "the listing is of {}, not of {}",
            listing.folder(),
            files.folder
        )));
    }

    let dir = &files.dir;
    let _held = lock::acquire(dir).await?;
    let record = Record::read(dir);

    if files.trust == Trust::LocalFiles {
        let names = trusted_names(files, listing, record.as_ref());
        if !names.is_empty() && names.iter().all(|name| is_file(&dir.join(name))) {
            let trusted = Record::Trusted {
                stamp: stamp(dir, &names),
            };
            if record.as_ref() != Some(&trusted) {
                clear(dir, &files.derived).await?;
                trusted.write(dir)?;
                tracing::info!("trusting the files of {} at {}", files.name, dir.display());
            }
            return Ok(Outcome::Trusted);
        }
    }

    let Some(listing) = listing else {
        return match record.as_ref().and_then(|record| record.intact(dir)) {
            Some(_) => Ok(Outcome::Offline),
            None => Err(Failure::Unlisted),
        };
    };

    let wanted = wanted(&files.select, listing)?;
    if let Some(recorded) = record.as_ref().and_then(|record| record.intact(dir))
        && recorded.len() == wanted.len()
        && wanted.iter().all(|file| recorded_as(recorded, file))
    {
        return Ok(Outcome::Current);
    }
    install(files, listing, &wanted, record.as_ref(), retries, on_event).await
}

/// The listing's files `select` takes, every named one present for [`Select::Names`].
fn wanted(select: &Select, listing: &Listing) -> Result<Vec<RemoteFile>, Failure> {
    let wanted: Vec<RemoteFile> = listing
        .files()
        .iter()
        .filter(|file| select.matches(file.file_name()))
        .cloned()
        .collect();
    let complete = match select {
        Select::Names(names) => names
            .iter()
            .all(|name| wanted.iter().any(|file| file.file_name() == name)),
        Select::Prefix(_) => true,
    };
    if wanted.is_empty() || !complete {
        return Err(Failure::NotListed);
    }
    Ok(wanted)
}

/// Whether `recorded` holds `file` under its name, size and hash.
fn recorded_as(recorded: &[Recorded], file: &RemoteFile) -> bool {
    recorded
        .iter()
        .any(|entry| entry.path == file.file_name() && entry.size == file.size && file.hash.matches(&entry.hash))
}

/// The files a trusted install needs in its folder: the names given, else the listing's selection, else the record's
/// files, else the folder's own files the prefix takes.
fn trusted_names(files: &Files, listing: Option<&Listing>, record: Option<&Record>) -> Vec<String> {
    let names = match (&files.select, listing, record) {
        (Select::Names(names), _, _) => names.clone(),
        (select, Some(listing), _) => listing
            .files()
            .iter()
            .map(RemoteFile::file_name)
            .filter(|name| select.matches(name))
            .map(str::to_owned)
            .collect(),
        (_, None, Some(Record::Files { files })) => files.iter().map(|file| file.path.clone()).collect(),
        (select, None, _) => std::fs::read_dir(&files.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !is_own(name) && select.matches(name))
            .collect(),
    };
    names.into_iter().filter(|name| single(name)).collect()
}

/// The stamp of the files named `names` in `dir`: SHA-256 over each one's name, size and modification time in
/// nanoseconds, in name order. A file that cannot be read contributes `absent` rather than nothing.
fn stamp(dir: &Path, names: &[String]) -> String {
    let mut names: Vec<&String> = names.iter().collect();
    names.sort();
    let mut input = String::new();
    for name in names {
        input.push('\0');
        input.push_str(name);
        input.push('\0');
        match std::fs::symlink_metadata(dir.join(name)) {
            Ok(meta) => {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or_else(|| "?".to_owned(), |since| since.as_nanos().to_string());
                input.push_str(&format!("{}\0{modified}", meta.len()));
            }
            Err(_) => input.push_str("absent"),
        }
    }
    crate::crypto::sha256_string(&input)
}

/// Install `wanted` into the folder, from `listing`'s commit: see [`install_files`].
async fn install(
    files: &Files,
    listing: &Listing,
    wanted: &[RemoteFile],
    record: Option<&Record>,
    retries: u32,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<Outcome, Failure> {
    let (name, dir) = (&files.name, &files.dir);
    // Before a byte moves, so an install that stops anywhere leaves no record over it, and never new files beside
    // what was compiled from the old ones.
    clear(dir, &files.derived).await?;

    let hub = Hub::for_folder(listing.folder()).retries(retries);
    let staging = dir.join(STAGING);
    let total: u64 = wanted.iter().map(|file| file.size).sum();
    let mut fraction = Fraction::default();
    let mut done = 0;
    let mut downloaded = false;
    for file in wanted {
        let target = dir.join(file.file_name());
        if !keep(&target, file, record, name, on_event).await? {
            std::fs::create_dir_all(&staging)?;
            let staged = staging.join(file.file_name());
            let mut notify = |step: Step| match step {
                Step::OnDisk(bytes) => {
                    let bytes = done + bytes.min(file.size);
                    on_event(Event::Progress {
                        dependency: name.clone(),
                        phase: Phase::Download,
                        bytes,
                        total,
                        fraction: fraction.whole(bytes, total),
                    });
                }
                Step::Verifying => on_event(Event::Verifying {
                    dependency: name.clone(),
                }),
            };
            fetch_checked(&hub, &hub.url(listing.commit(), &file.path), &staged, file, &mut notify).await?;
            std::fs::rename(&staged, &target)?;
            downloaded = true;
        }
        done += file.size;
        on_event(Event::Progress {
            dependency: name.clone(),
            phase: Phase::Download,
            bytes: done,
            total,
            fraction: fraction.whole(done, total),
        });
    }

    let keeping: Vec<String> = wanted.iter().map(|file| file.file_name().to_owned()).collect();
    let recorded = Record::Files {
        files: wanted
            .iter()
            .map(|file| Recorded {
                path: file.file_name().to_owned(),
                size: file.size,
                hash: file.hash.hex().to_owned(),
            })
            .collect(),
    };
    let finishing = dir.clone();
    blocking(move || {
        for entry in std::fs::read_dir(&finishing)? {
            let entry = entry?;
            let entry_name = entry.file_name();
            let keep = entry_name
                .to_str()
                .is_some_and(|entry_name| is_own(entry_name) || keeping.iter().any(|kept| kept == entry_name));
            if !keep {
                remove_if_present(&entry.path(), entry.file_type()?.is_dir())?;
            }
        }
        recorded.write(&finishing)?;
        Ok(())
    })
    .await?;
    if let Err(cause) = remove_if_present(&staging, true) {
        tracing::warn!("could not remove {}: {cause}", staging.display());
    }

    on_event(Event::Progress {
        dependency: name.clone(),
        phase: Phase::Download,
        bytes: total,
        total,
        fraction: fraction.finish(),
    });
    if downloaded {
        tracing::info!("installed {name} at {}", dir.display());
        Ok(Outcome::Installed)
    } else {
        tracing::info!("adopted the files of {name} already at {}", dir.display());
        Ok(Outcome::Adopted)
    }
}

/// Whether the file already at `target` is `file`: at its listed size, and either recorded at its hash by the install
/// this one replaces, or hashing to it now (reporting [`Event::Verifying`]).
async fn keep(
    target: &Path,
    file: &RemoteFile,
    record: Option<&Record>,
    name: &str,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<bool, Failure> {
    if !is_file(target) || std::fs::symlink_metadata(target).map(|meta| meta.len())? != file.size {
        return Ok(false);
    }
    if let Some(Record::Files { files }) = record
        && recorded_as(files, file)
    {
        return Ok(true);
    }

    on_event(Event::Verifying {
        dependency: name.to_owned().into(),
    });
    let (path, hash) = (target.to_path_buf(), file.hash.clone());
    let matches = blocking(move || Ok(hash.verify_file(path)?)).await?;
    if !matches {
        tracing::info!("{} does not match the listing's hash; downloading it", target.display());
    }
    Ok(matches)
}

/// Download `url` to `staged` and check it against `file`'s hash. A download that resumed bytes already on disk and
/// does not match is downloaded once more from nothing; one that still does not match, or a clean one that does not,
/// is deleted, and the install fails as damaged.
async fn fetch_checked(
    hub: &Hub,
    url: &str,
    staged: &Path,
    file: &RemoteFile,
    notify: &mut (dyn FnMut(Step) + Send),
) -> Result<(), Failure> {
    let resumed = already_on_disk(staged) > 0;
    let (digest, _) = fetch_hashed(hub.fetch(), url.to_owned(), staged, &file.hash, notify).await?;
    if file.hash.matches(&digest) {
        return Ok(());
    }
    discard(staged)?;
    let digest = if resumed {
        tracing::warn!(
            "{} resumed to {digest}, not the listing's {}; downloading it again from nothing",
            file.path,
            file.hash.hex()
        );
        let (digest, _) = fetch_hashed(hub.fetch(), url.to_owned(), staged, &file.hash, notify).await?;
        if file.hash.matches(&digest) {
            return Ok(());
        }
        discard(staged)?;
        digest
    } else {
        digest
    };
    tracing::error!(
        "{} hashed to {digest}, not the listing's {}; deleting it",
        file.path,
        file.hash.hex()
    );
    Err(Failure::Damaged)
}

/// Delete `staged` and its partial and sidecar, so nothing of a download that did not match is resumed.
fn discard(staged: &Path) -> io::Result<()> {
    let part = part_path(staged);
    let mut sidecar = part.clone().into_os_string();
    sidecar.push(".json");
    for path in [staged.to_path_buf(), part, PathBuf::from(sidecar)] {
        remove_if_present(&path, false)?;
    }
    Ok(())
}

/// Delete the record in `dir`, then empty each of `derived` that exists, keeping the folder.
async fn clear(dir: &Path, derived: &[PathBuf]) -> Result<(), Failure> {
    let (dir, derived) = (dir.to_path_buf(), derived.to_vec());
    blocking(move || {
        remove_if_present(&dir.join(MARKER), false)?;
        for folder in derived.iter().filter(|folder| folder.is_dir()) {
            for entry in std::fs::read_dir(folder)? {
                let entry = entry?;
                remove_if_present(&entry.path(), entry.file_type()?.is_dir())?;
            }
        }
        Ok(())
    })
    .await
}

/// Run file work on a blocking thread.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, Failure> + Send + 'static) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|cause| Failure::Io(cause.to_string()))?
}

/// Whether `name`, a single plain component, is a regular file in `dir` of `size` bytes.
fn present_at(dir: &Path, name: &str, size: u64) -> bool {
    single(name) && std::fs::symlink_metadata(dir.join(name)).is_ok_and(|meta| meta.is_file() && meta.len() == size)
}

/// Whether `path` is a regular file, a link not followed.
fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Whether `name` is one plain path component, which a record this crate wrote always holds.
fn single(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

#[cfg(test)]
mod tests;
