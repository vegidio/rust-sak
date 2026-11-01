use std::path::Path;
use std::time::Duration;

use super::*;
use crate::ai::test_support::Server;

/// Two small files, made with `bsdtar --format 7zip`.
const FIXTURE: &[u8] = include_bytes!("../testdata/runtime.7z");
const LIB: &str = "libonnxruntime.test.dylib";
const PLUGIN: &str = "libonnxruntime_providers_webgpu.test.dylib";

/// The library and 64 KiB that does not compress: large enough for an interrupted transfer to leave a partial, since
/// `fetch` writes through a buffer.
const LARGE: &[u8] = include_bytes!("../testdata/large.7z");

/// A Linux library's chain, `libcudart.so -> libcudart.so.13 -> libcudart.so.13.3.0`, and `sub/nested.txt`, made with
/// `bsdtar --format 7zip`.
const LINKS: &[u8] = include_bytes!("../testdata/links.7z");

/// The fixture's entries, the marker and the lock.
const INSTALLED: [&str; 3] = [".installed", LIB, PLUGIN];

const COMMIT: &str = "501d7b94e6d22762667e213994b0242ccdb04588";

/// The archive name the running platform picks from a folder of `name`.
fn asset(name: &str) -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    format!("{name}_{}_{arch}.7z", std::env::consts::OS)
}

struct Setup {
    server: Server,
    root: crate::fs::TempDir,
}

fn setup() -> Setup {
    Setup {
        server: Server::start("test/general", COMMIT),
        root: crate::fs::mk_temp_dir("rust-sak-ai-install").unwrap(),
    }
}

impl Setup {
    /// Publish the fixture in `deps/<name>/<version>/`, and return the folder.
    fn publish(&self, name: &str, version: &str) -> String {
        self.publish_bytes(name, version, FIXTURE)
    }

    fn publish_bytes(&self, name: &str, version: &str, bytes: &[u8]) -> String {
        let folder = format!("deps/{name}/{version}");
        self.server.put(&format!("{folder}/{}", asset(name)), bytes, true);
        folder
    }

    fn libraries(&self, name: &'static str, folder: &str) -> Dependency {
        Dependency::libraries(name, Source::parse(&self.server.tree_url(folder)).unwrap())
    }

    /// Where `folder` installs: its last two segments under the root.
    fn dir(&self, folder: &str) -> PathBuf {
        let mut segments = folder.rsplit('/');
        let version = segments.next().unwrap();
        let name = segments.next().unwrap();
        self.root.path().join(name).join(version)
    }

    /// Install without retries, and return the outcome and every event.
    async fn install(&self, dependencies: &[Dependency]) -> (Result<(), InstallError>, Vec<Event>) {
        install_into(self.root.path(), dependencies).await
    }
}

async fn install_into(root: &Path, dependencies: &[Dependency]) -> (Result<(), InstallError>, Vec<Event>) {
    let mut events = Vec::new();
    let result = match plan_retrying(root, dependencies, 0).await {
        Ok(plan) => plan.install(|event| events.push(event)).await,
        Err(error) => Err(error),
    };
    (result, events)
}

/// A folder's entries, sorted, staging included.
fn entries(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into())
        .collect();
    names.sort();
    names
}

/// Every file under `dir`, with its contents, sorted by path. A lock file is listed empty, as it always is: Windows
/// refuses to read one another handle holds locked.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.path().extension().is_some_and(|extension| extension == "lock") {
                files.push((entry.path(), Vec::new()));
            } else {
                files.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// The fractions `dependency` reported, in order.
fn fractions(events: &[Event], dependency: &str) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Progress {
                dependency: name,
                fraction,
                ..
            } if name == dependency => Some(*fraction),
            _ => None,
        })
        .collect()
}

fn names(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .map(|event| match event {
            Event::Progress { dependency, .. } | Event::Verifying { dependency } | Event::Current { dependency } => {
                dependency.as_ref()
            }
        })
        .collect()
}

fn current(dependency: &'static str) -> Event {
    Event::Current {
        dependency: dependency.into(),
    }
}

/// The hash the marker in `dir` records.
fn marker_of(dir: &Path) -> Option<String> {
    Marker::read(dir).map(|marker| marker.hash)
}

#[tokio::test]
async fn a_clean_install_leaves_exactly_the_archives_entries_and_the_marker() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let (result, events) = setup.install(&[setup.libraries("NVIDIA CUDA", &folder)]).await;
    assert_eq!(result, Ok(()));

    let fractions = fractions(&events, "NVIDIA CUDA");
    assert_eq!(fractions.len(), events.len(), "only progress: {events:?}");
    assert!(fractions.is_sorted(), "never decreasing: {fractions:?}");
    assert_eq!(fractions[0], 0.0);
    assert!(
        fractions.contains(&0.8),
        "the download fills the first 80%: {fractions:?}"
    );
    let (last, before) = fractions.split_last().unwrap();
    assert_eq!(*last, 1.0);
    assert!(
        before.iter().all(|fraction| *fraction < 1.0),
        "1 only at the end: {fractions:?}"
    );
    assert!(
        before.iter().any(|fraction| *fraction > 0.8),
        "the extraction reports too: {fractions:?}"
    );

    let dir = setup.dir(&folder);
    assert_eq!(dir, setup.root.path().join("cuda/13.3.0"));
    assert_eq!(entries(&dir), INSTALLED, "no staging left");
    let marker = Marker::read(&dir).unwrap();
    assert_eq!(marker.hash, crate::crypto::sha256_bytes(FIXTURE));
    assert_eq!(marker.size, FIXTURE.len() as u64);
    let recorded: Vec<&str> = marker.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        recorded,
        [LIB, PLUGIN],
        "the archive's entries, and none of the installer's own"
    );
    let lib = std::fs::read_to_string(dir.join(LIB)).unwrap();
    assert!(lib.starts_with("a stand-in for the ONNX Runtime library"), "{lib}");
    let downloads = setup.server.downloads();
    assert_eq!(downloads.len(), 1);
    assert_eq!(
        downloads[0].path,
        format!("/test/general/resolve/{COMMIT}/{folder}/{}", asset("cuda"))
    );
}

#[tokio::test]
async fn each_phase_counts_its_own_bytes_the_download_before_the_extraction() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let (result, events) = setup.install(&[setup.libraries("NVIDIA CUDA", &folder)]).await;
    assert_eq!(result, Ok(()));

    let progress: Vec<(Phase, u64, u64)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress {
                phase, bytes, total, ..
            } => Some((*phase, *bytes, *total)),
            _ => None,
        })
        .collect();
    let downloads = progress
        .iter()
        .take_while(|(phase, ..)| *phase == Phase::Download)
        .count();
    assert!(downloads > 0, "{progress:?}");
    assert!(
        progress[downloads..].iter().all(|(phase, ..)| *phase == Phase::Extract),
        "every download report comes first: {progress:?}"
    );
    let archive = FIXTURE.len() as u64;
    assert!(
        progress[..downloads]
            .iter()
            .all(|(_, bytes, total)| *total == archive && *bytes <= archive),
        "{progress:?}"
    );
    assert_eq!(progress[downloads - 1].1, archive, "the download ends whole");
    assert!(
        progress[downloads..].iter().all(|(_, bytes, total)| bytes <= total),
        "{progress:?}"
    );
    let (_, bytes, total) = progress.last().unwrap();
    assert_eq!(bytes, total, "the last report is the extraction whole");
}

#[tokio::test]
async fn an_intact_install_makes_no_request_and_reports_it_current() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    let requests = setup.server.requests().len();

    let (result, events) = setup.install(&[dependency]).await;
    assert_eq!(result, Ok(()));
    assert_eq!(events, [current("NVIDIA CUDA")]);
    assert_eq!(setup.server.requests().len(), requests, "not one request");
}

#[tokio::test]
async fn offline_an_intact_install_is_used_as_it_is() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    let before = snapshot(setup.root.path());

    setup.server.fail_api(503);
    let (result, events) = setup.install(std::slice::from_ref(&dependency)).await;
    assert_eq!((result, events), (Ok(()), vec![current("NVIDIA CUDA")]));

    let unreachable = Dependency::libraries(
        "NVIDIA CUDA",
        Source::parse(&format!("{}/test/general/tree/main/{folder}", Server::closed_base())).unwrap(),
    );
    let (result, events) = setup.install(&[unreachable]).await;
    assert_eq!(
        (result, events),
        (Ok(()), vec![current("NVIDIA CUDA")]),
        "no network at all"
    );
    assert_eq!(snapshot(setup.root.path()), before, "nothing touched");
}

#[tokio::test]
async fn a_deleted_or_resized_file_is_installed_again() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    let dir = setup.dir(&folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));

    std::fs::remove_file(dir.join(PLUGIN)).unwrap();
    assert!(!is_present(setup.root.path(), &dependency));
    let (result, events) = setup.install(std::slice::from_ref(&dependency)).await;
    assert_eq!(result, Ok(()));
    assert_eq!(fractions(&events, "NVIDIA CUDA").last(), Some(&1.0));
    assert_eq!(entries(&dir), INSTALLED);

    std::fs::write(dir.join(LIB), b"truncated").unwrap();
    assert!(!is_present(setup.root.path(), &dependency));
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    assert!(is_present(setup.root.path(), &dependency));
    assert_eq!(setup.server.downloads().len(), 3, "each damage cost one download");
}

#[cfg(unix)]
#[tokio::test]
async fn an_install_of_linked_libraries_reads_as_intact() {
    let setup = setup();
    let folder = setup.publish_bytes("cuda", "13.3.0", LINKS);
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));

    let dir = setup.dir(&folder);
    assert!(
        std::fs::symlink_metadata(dir.join("libcudart.so"))
            .unwrap()
            .is_symlink(),
        "the archive's links are links"
    );
    let marker = Marker::read(&dir).unwrap();
    let recorded: Vec<&str> = marker.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        recorded,
        [
            "libcudart.so",
            "libcudart.so.13",
            "libcudart.so.13.3.0",
            "sub/nested.txt"
        ]
    );
    assert!(is_present(setup.root.path(), &dependency));
    let (result, events) = setup.install(&[dependency]).await;
    assert_eq!((result, events), (Ok(()), vec![current("NVIDIA CUDA")]));
    assert_eq!(setup.server.downloads().len(), 1);
}

#[tokio::test]
async fn a_marker_of_another_format_is_installed_over() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dir = setup.dir(&folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libcudart.so.13.old"), b"from the earlier upload").unwrap();
    // The bare hash earlier releases wrote.
    std::fs::write(
        dir.join(MARKER),
        "50c3dc45b758105bec33f640c3db9f4efad97c99b3f99c2be52e00cfe89b6811",
    )
    .unwrap();

    let (result, events) = setup.install(&[setup.libraries("NVIDIA CUDA", &folder)]).await;
    assert_eq!(result, Ok(()));
    assert_eq!(fractions(&events, "NVIDIA CUDA").last(), Some(&1.0));
    assert_eq!(entries(&dir), INSTALLED, "none of the earlier files remain");
    assert_eq!(marker_of(&dir), Some(crate::crypto::sha256_bytes(FIXTURE)));
}

#[tokio::test]
async fn offline_without_an_install_fails() {
    let setup = setup();
    let unreachable = Dependency::libraries(
        "NVIDIA CUDA",
        Source::parse(&format!(
            "{}/test/general/tree/main/deps/cuda/13.3.0",
            Server::closed_base()
        ))
        .unwrap(),
    );
    let (result, _) = setup.install(&[unreachable]).await;
    assert_eq!(
        result,
        Err(InstallError {
            dependency: "NVIDIA CUDA".into(),
            failure: Failure::Network
        })
    );

    // A marker without its library is no install of the runtime.
    let dir = setup.root.path().join("onnx/1.30.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("README.txt"), "x").unwrap();
    Marker::record(&dir, "abc", 1).unwrap().write(&dir).unwrap();
    let runtime = Dependency::runtime(
        "ONNX Runtime",
        Source::parse(&format!(
            "{}/test/general/tree/main/deps/onnx/1.30.0",
            Server::closed_base()
        ))
        .unwrap(),
    );
    let (result, _) = setup.install(&[runtime]).await;
    assert_eq!(
        result,
        Err(InstallError {
            dependency: "ONNX Runtime".into(),
            failure: Failure::Network
        })
    );
}

#[tokio::test]
async fn a_hash_mismatch_deletes_the_archive_and_leaves_the_folder_alone() {
    let setup = setup();
    let folder = setup.publish("tensorrt", "10.14.1");
    let dir = setup.dir(&folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libnvinfer.so.old"), b"old").unwrap();
    setup
        .server
        .corrupt(&format!("{folder}/{}", asset("tensorrt")), b"not what the API reported");

    let (result, _) = setup.install(&[setup.libraries("NVIDIA TensorRT", &folder)]).await;
    let Err(InstallError {
        dependency,
        failure: Failure::Damaged { path, expected, actual },
    }) = result
    else {
        panic!("{result:?}");
    };
    assert_eq!(dependency, "NVIDIA TensorRT");
    assert_eq!(path, format!("{folder}/{}", asset("tensorrt")));
    assert_eq!(expected, crate::crypto::sha256_bytes(FIXTURE));
    assert_eq!(actual, crate::crypto::sha256_bytes(b"not what the API reported"));
    assert!(
        !dir.join(STAGING).join(asset("tensorrt")).exists(),
        "the damaged archive is deleted"
    );
    assert_eq!(marker_of(&dir), None);
    let outside: Vec<_> = entries(&dir).into_iter().filter(|name| name != STAGING).collect();
    assert_eq!(outside, ["libnvinfer.so.old"], "nothing outside staging changed");
}

#[tokio::test]
async fn a_staged_archive_that_matches_is_rehashed_and_installed_without_a_download() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let staging = setup.dir(&folder).join(STAGING);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join(asset("cuda")), FIXTURE).unwrap();

    let (result, events) = setup.install(&[setup.libraries("NVIDIA CUDA", &folder)]).await;
    assert_eq!(result, Ok(()));
    assert!(
        events.contains(&Event::Verifying {
            dependency: "NVIDIA CUDA".into()
        }),
        "{events:?}"
    );
    assert!(
        fractions(&events, "NVIDIA CUDA")[0] >= 0.8,
        "the staged archive counts whole"
    );
    assert!(setup.server.downloads().is_empty());
    assert_eq!(entries(&setup.dir(&folder)), INSTALLED);
}

#[tokio::test]
async fn a_staged_archive_that_does_not_match_is_downloaded_again() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let staging = setup.dir(&folder).join(STAGING);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join(asset("cuda")), b"an archive of another upload").unwrap();

    let (result, events) = setup.install(&[setup.libraries("NVIDIA CUDA", &folder)]).await;
    assert_eq!(result, Ok(()), "in the same run");
    assert_eq!(setup.server.downloads().len(), 1);
    assert!(fractions(&events, "NVIDIA CUDA").is_sorted());
    assert_eq!(entries(&setup.dir(&folder)), INSTALLED);
}

#[tokio::test]
async fn an_interrupted_download_resumes_reporting_its_partial_first() {
    let setup = setup();
    let folder = setup.publish_bytes("cuda", "13.3.0", LARGE);
    let path = format!("{folder}/{}", asset("cuda"));
    // Three quarters sent, of which at least half reaches the partial past the write buffer.
    setup.server.break_after(&path, LARGE.len() * 3 / 4);
    let dependency = setup.libraries("NVIDIA CUDA", &folder);

    let (result, _) = setup.install(std::slice::from_ref(&dependency)).await;
    assert!(result.is_err(), "the broken transfer fails without retries: {result:?}");
    assert_eq!(marker_of(&setup.dir(&folder)), None);
    // Tokio finishes a dropped file's last write in the background; let it land, as a next run would.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // `main` moved on in between, which moves the URL but not the archive's hash.
    setup.server.set_commit("ffffffffffffffffffffffffffffffffffffffff");
    let (result, events) = setup.install(&[dependency]).await;
    assert_eq!(result, Ok(()));
    let resumed = setup.server.downloads().last().unwrap().clone();
    assert!(resumed.path.contains("/ffffffff"), "{resumed:?}");
    let from = resumed.range_from.expect("only the remaining bytes are asked for");
    assert!(from >= LARGE.len() as u64 / 2, "at least half was staged: {from}");
    let first = fractions(&events, "NVIDIA CUDA")[0];
    #[allow(clippy::cast_precision_loss, reason = "a few kilobytes")]
    let staged = 0.8 * from as f64 / LARGE.len() as f64;
    assert!(
        (first - staged).abs() < 1e-9,
        "the partial counts from the first report: {first}, not {staged}"
    );
    assert!(first >= 0.4, "{first}");
    assert_eq!(
        entries(&setup.dir(&folder)),
        [".installed", "libonnxruntime.test.dylib", "padding.bin"]
    );
}

#[tokio::test]
async fn a_partial_of_another_upload_is_discarded() {
    let setup = setup();
    let folder = setup.publish_bytes("cuda", "13.3.0", b"an earlier upload, long enough to break off halfway");
    let path = format!("{folder}/{}", asset("cuda"));
    setup.server.break_after(&path, 20);
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert!(setup.install(std::slice::from_ref(&dependency)).await.0.is_err());

    setup.publish("cuda", "13.3.0");
    let (result, _) = setup.install(&[dependency]).await;
    assert_eq!(result, Ok(()));
    assert_eq!(
        setup.server.downloads().last().unwrap().range_from,
        None,
        "downloaded from the start"
    );
    assert_eq!(entries(&setup.dir(&folder)), INSTALLED);
}

#[tokio::test]
async fn a_dropped_install_keeps_the_partial_and_releases_the_lock() {
    let setup = setup();
    let folder = setup.publish_bytes("cuda", "13.3.0", LARGE);
    let path = format!("{folder}/{}", asset("cuda"));
    setup.server.hang_after(&path, LARGE.len() * 3 / 4);
    let dependency = setup.libraries("NVIDIA CUDA", &folder);

    let (sender, mut progress) = tokio::sync::mpsc::unbounded_channel();
    let root = setup.root.path().to_path_buf();
    let task = tokio::spawn(async move {
        let plan = plan_retrying(&root, &[dependency], 0).await?;
        plan.install(move |event| {
            let _ = sender.send(event);
        })
        .await
    });
    // The install's own first report, then the transfer's as the response's headers land; the body follows them.
    for _ in 0..2 {
        let event = tokio::time::timeout(Duration::from_secs(10), progress.recv())
            .await
            .expect("the download reports");
        assert!(matches!(event, Some(Event::Progress { .. })), "{event:?}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let dir = setup.dir(&folder);
    let part = std::fs::metadata(part_path(&dir.join(STAGING).join(asset("cuda")))).expect("the partial is kept");
    assert!(part.len() > 0, "it holds the bytes that arrived, to resume from");
    assert_eq!(marker_of(&dir), None);
    assert_eq!(entries(&dir), [STAGING], "nothing reached the folder");
    assert!(
        lock::try_acquire(&dir).unwrap().is_some(),
        "the lock went with the install"
    );
}

#[tokio::test]
async fn every_listing_and_download_uses_the_one_resolved_commit_and_each_installs_in_turn() {
    let setup = setup();
    setup.server.move_on_every_resolve();
    let cuda = setup.publish("cuda", "13.3.0");
    let cudnn = setup.publish("cudnn", "9.23.1");
    let dependencies = [
        setup.libraries("NVIDIA CUDA", &cuda),
        setup.libraries("NVIDIA cuDNN", &cudnn),
    ];

    let (result, events) = setup.install(&dependencies).await;
    assert_eq!(result, Ok(()));

    let requests = setup.server.requests();
    let resolves = requests.iter().filter(|path| path.contains("/revision/")).count();
    assert_eq!(resolves, 1, "one resolve per repository: {requests:?}");
    let listings = requests.iter().filter(|path| path.contains("/tree/")).count();
    assert_eq!(listings, 2, "one listing per folder, the plan's: {requests:?}");
    let commit = format!("{COMMIT}-1");
    assert!(
        requests
            .iter()
            .filter(|path| !path.contains("/revision/"))
            .all(|path| path.contains(&commit)),
        "every listing and download at {commit}: {requests:?}"
    );

    let names = names(&events);
    let first_cudnn = names
        .iter()
        .position(|name| *name == "NVIDIA cuDNN")
        .expect("cuDNN reports");
    assert!(
        names[..first_cudnn].iter().all(|name| *name == "NVIDIA CUDA"),
        "{names:?}"
    );
    assert!(
        names[first_cudnn..].iter().all(|name| *name == "NVIDIA cuDNN"),
        "{names:?}"
    );
    assert_eq!(
        fractions(&events, "NVIDIA CUDA").last(),
        Some(&1.0),
        "CUDA finished before cuDNN began"
    );
    assert_eq!(entries(&setup.dir(&cuda)), INSTALLED);
    assert_eq!(entries(&setup.dir(&cudnn)), INSTALLED);
}

#[tokio::test]
async fn the_plan_gives_each_dependency_its_state_and_size() {
    let setup = setup();
    let cuda = setup.publish("cuda", "13.3.0");
    let tensorrt = setup.publish_bytes("tensorrt", "10.14.1", LARGE);
    setup.server.put("deps/cudnn/9.23.1/cudnn_plan9_x64.7z", FIXTURE, true);
    let dependencies = [
        setup.libraries("NVIDIA CUDA", &cuda),
        setup.libraries("NVIDIA cuDNN", "deps/cudnn/9.23.1"),
        setup.libraries("NVIDIA TensorRT", &tensorrt),
    ];
    assert_eq!(setup.install(&dependencies[..1]).await.0, Ok(()));
    let requests = setup.server.requests().len();

    let plan = plan_retrying(setup.root.path(), &dependencies, 0).await.unwrap();
    let rows: Vec<Planned> = plan.dependencies().cloned().collect();
    assert_eq!(
        rows,
        [
            Planned {
                name: "NVIDIA CUDA",
                state: State::Present,
                size: FIXTURE.len() as u64
            },
            Planned {
                name: "NVIDIA cuDNN",
                state: State::Unavailable,
                size: 0
            },
            Planned {
                name: "NVIDIA TensorRT",
                state: State::Missing,
                size: LARGE.len() as u64
            },
        ]
    );
    let made = setup.server.requests().len() - requests;
    assert_eq!(
        made,
        3,
        "one resolve, and a listing for each missing folder: {:?}",
        setup.server.requests()
    );

    let mut events = Vec::new();
    assert_eq!(plan.install(|event| events.push(event)).await, Ok(()));
    assert_eq!(
        setup.server.requests().len() - requests,
        made + 1,
        "the install lists nothing again, and downloads one"
    );
    assert_eq!(events[0], current("NVIDIA CUDA"), "one current, first");
    let names = names(&events[1..]);
    assert!(names.iter().all(|name| *name == "NVIDIA TensorRT"), "{names:?}");
    assert!(fractions(&events, "NVIDIA TensorRT").is_sorted());
    assert!(
        !events.contains(&current("NVIDIA TensorRT")) && !names.contains(&"NVIDIA cuDNN"),
        "{events:?}"
    );
}

#[tokio::test]
async fn a_failure_names_its_dependency_and_leaves_the_ones_before_installed() {
    let setup = setup();
    let cuda = setup.publish("cuda", "13.3.0");
    let cudnn = setup.publish("cudnn", "9.23.1");
    let tensorrt = setup.publish("tensorrt", "10.14.1");
    setup.server.fail_file(&format!("{cudnn}/{}", asset("cudnn")), 404);
    let dependencies = [
        setup.libraries("NVIDIA CUDA", &cuda),
        setup.libraries("NVIDIA cuDNN", &cudnn),
        setup.libraries("NVIDIA TensorRT", &tensorrt),
    ];

    let (result, events) = setup.install(&dependencies).await;
    let error = result.unwrap_err();
    assert_eq!(
        error,
        InstallError {
            dependency: "NVIDIA cuDNN".into(),
            failure: Failure::Http(404)
        }
    );
    assert_eq!(
        error.to_string(),
        "could not install NVIDIA cuDNN: the server answered with HTTP 404"
    );
    assert_eq!(
        marker_of(&setup.dir(&cuda)),
        Some(crate::crypto::sha256_bytes(FIXTURE)),
        "CUDA stays installed"
    );
    assert!(
        !names(&events).contains(&"NVIDIA TensorRT"),
        "nothing after the failure is tried"
    );
    assert_eq!(marker_of(&setup.dir(&tensorrt)), None);
}

#[tokio::test]
async fn a_folder_with_no_archive_for_the_platform_skips_libraries_and_fails_the_runtime() {
    let setup = setup();
    setup.server.put("deps/cuda/13.3.0/cuda_plan9_x64.7z", FIXTURE, true);
    let (result, events) = setup
        .install(&[setup.libraries("NVIDIA CUDA", "deps/cuda/13.3.0")])
        .await;
    assert_eq!(result, Ok(()), "libraries the platform has no archive for are skipped");
    assert!(events.is_empty());
    assert!(!setup.root.path().join("cuda").exists(), "nothing is created");

    setup.server.put("deps/onnx/1.30.0/onnx_plan9_x64.7z", FIXTURE, true);
    let runtime = Dependency::runtime(
        "ONNX Runtime",
        Source::parse(&setup.server.tree_url("deps/onnx/1.30.0")).unwrap(),
    );
    let (result, _) = setup.install(&[runtime]).await;
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    assert_eq!(
        result,
        Err(InstallError {
            dependency: "ONNX Runtime".into(),
            failure: Failure::UnsupportedPlatform { os, arch }
        })
    );
    assert!(setup.server.downloads().is_empty());
}

#[tokio::test]
async fn two_roots_never_touch_each_others_files() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    let first = snapshot(setup.root.path());

    let other = crate::fs::mk_temp_dir("rust-sak-ai-other-root").unwrap();
    assert_eq!(install_into(other.path(), &[dependency]).await.0, Ok(()));
    assert_eq!(snapshot(setup.root.path()), first, "the first root is untouched");
    assert_eq!(entries(&other.path().join("cuda/13.3.0")), INSTALLED);
}

/// Another version of `name` installed under `setup`'s root, as another app would have left it.
fn other_version(setup: &Setup, name: &str, version: &str) -> PathBuf {
    let other = setup.root.path().join(name).join(version);
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("libonnxruntime.other.dylib"), b"another app's runtime").unwrap();
    Marker::record(&other, "abc", 1).unwrap().write(&other).unwrap();
    other
}

#[tokio::test]
async fn installing_a_version_removes_the_others() {
    let setup = setup();
    let older = other_version(&setup, "onnx", "1.29.0");
    let newer = other_version(&setup, "onnx", "1.31.0");
    let unrelated = other_version(&setup, "cuda", "13.3.0");

    let folder = setup.publish("onnx", "1.30.0");
    assert_eq!(
        setup.install(&[setup.libraries("ONNX Runtime", &folder)]).await.0,
        Ok(())
    );
    assert!(!older.exists(), "{}", older.display());
    assert!(!newer.exists(), "a newer version too: the build names the one it wants");
    // Each version's lock file stays beside it, so a process waiting on one never holds a lock no one else sees.
    assert_eq!(
        entries(&setup.root.path().join("onnx")),
        ["1.29.0.lock", "1.30.0", "1.30.0.lock", "1.31.0.lock"]
    );
    assert_eq!(entries(&setup.dir(&folder)), INSTALLED);
    assert!(unrelated.join(MARKER).is_file(), "another dependency is not touched");
}

#[tokio::test]
async fn a_version_another_process_holds_locked_is_left_in_place() {
    let setup = setup();
    let older = other_version(&setup, "onnx", "1.29.0");
    let held = lock::try_acquire(&older).unwrap().unwrap();
    let before = snapshot(&older);

    let folder = setup.publish("onnx", "1.30.0");
    assert_eq!(
        setup.install(&[setup.libraries("ONNX Runtime", &folder)]).await.0,
        Ok(())
    );
    assert_eq!(snapshot(&older), before, "the locked version survives");
    drop(held);
}

#[tokio::test]
async fn an_intact_install_does_not_remove_other_versions() {
    let setup = setup();
    let folder = setup.publish("onnx", "1.30.0");
    let dependency = setup.libraries("ONNX Runtime", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    let older = other_version(&setup, "onnx", "1.29.0");

    assert_eq!(setup.install(&[dependency]).await.0, Ok(()));
    assert!(older.join(MARKER).is_file(), "only an install replaces");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_installs_download_and_extract_once() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    // Long enough that the second install finds the first holding the lock.
    setup
        .server
        .delay(&format!("{folder}/{}", asset("cuda")), Duration::from_millis(500));
    let dependency = setup.libraries("NVIDIA CUDA", &folder);

    let root = setup.root.path().to_path_buf();
    let run = |dependency: Dependency| {
        let root = root.clone();
        tokio::spawn(async move { install_into(&root, &[dependency]).await })
    };
    let first = run(dependency.clone());
    let second = run(dependency.clone());
    let (first, second) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!((&first.0, &second.0), (&Ok(()), &Ok(())));

    assert_eq!(setup.server.downloads().len(), 1, "downloaded once");
    let installed = [&first.1, &second.1]
        .into_iter()
        .filter(|events| !fractions(events, "NVIDIA CUDA").is_empty())
        .count();
    assert_eq!(installed, 1, "extracted once: {first:?} {second:?}");
    assert!(
        first.1.contains(&current("NVIDIA CUDA")) || second.1.contains(&current("NVIDIA CUDA")),
        "the other used it: {first:?} {second:?}"
    );
    assert!(is_present(&root, &dependency));
}

/// Not a test of its own: [`a_lock_left_by_a_process_that_exited_does_not_block`] runs the test binary on just this,
/// with the folder to lock in `RUST_SAK_HOLD_LOCK`, and it exits holding the lock.
#[test]
fn hold_a_lock_and_exit() {
    let Some(dir) = std::env::var_os("RUST_SAK_HOLD_LOCK") else {
        return;
    };
    let held = lock::try_acquire(Path::new(&dir))
        .unwrap()
        .expect("nobody else holds it");
    std::mem::forget(held);
    std::process::exit(0);
}

#[tokio::test]
async fn a_lock_left_by_a_process_that_exited_does_not_block() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dir = setup.dir(&folder);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ai::install::tests::hold_a_lock_and_exit",
            "--test-threads=1",
        ])
        .env("RUST_SAK_HOLD_LOCK", &dir)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(lock::path(&dir).is_file(), "the child left its lock file");

    let dependencies = [setup.libraries("NVIDIA CUDA", &folder)];
    let (result, _) = tokio::time::timeout(Duration::from_secs(10), setup.install(&dependencies))
        .await
        .expect("the install does not wait on a process that is gone");
    assert_eq!(result, Ok(()));
    assert_eq!(entries(&dir), INSTALLED);
}

/// The runtime's install also needs its library, which the fixture's is named for on macOS.
#[cfg(target_os = "macos")]
mod runtime {
    use super::*;

    fn runtime(setup: &Setup, folder: &str) -> Dependency {
        Dependency::runtime("ONNX Runtime", Source::parse(&setup.server.tree_url(folder)).unwrap())
    }

    #[tokio::test]
    async fn a_marked_runtime_without_its_library_is_installed_again() {
        let setup = setup();
        let folder = setup.publish("onnx", "1.30.0");
        let dependency = runtime(&setup, &folder);
        assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
        assert!(is_present(setup.root.path(), &dependency));

        std::fs::remove_file(setup.dir(&folder).join(LIB)).unwrap();
        assert!(!is_present(setup.root.path(), &dependency));
        let (result, events) = setup.install(std::slice::from_ref(&dependency)).await;
        assert_eq!(result, Ok(()));
        assert_eq!(fractions(&events, "ONNX Runtime").last(), Some(&1.0));
        assert_eq!(entries(&setup.dir(&folder)), INSTALLED);
        assert_eq!(setup.server.downloads().len(), 2);
    }

    #[tokio::test]
    async fn a_current_runtime_reports_current() {
        let setup = setup();
        let folder = setup.publish("onnx", "1.30.0");
        let dependency = runtime(&setup, &folder);
        assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
        let (result, events) = setup.install(&[dependency]).await;
        assert_eq!((result, events), (Ok(()), vec![current("ONNX Runtime")]));
        assert_eq!(runtime_library(&setup.dir(&folder)), Some(setup.dir(&folder).join(LIB)));
    }
}
