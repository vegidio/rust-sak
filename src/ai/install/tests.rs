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

/// The fixture's entries and the marker.
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
    let result = install_retrying(root, dependencies, 0, |event| events.push(event)).await;
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

/// Every file under `dir`, with its contents, sorted by path.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
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
            } if *name == dependency => Some(*fraction),
            _ => None,
        })
        .collect()
}

fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            Event::Progress { dependency, .. } | Event::Verifying { dependency } => *dependency,
        })
        .collect()
}

fn marker_of(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(MARKER)).ok()
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
    assert_eq!(marker_of(&dir), Some(crate::crypto::sha256_bytes(FIXTURE)));
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
async fn a_current_install_downloads_nothing_and_reports_nothing() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));

    let (result, events) = setup.install(&[dependency]).await;
    assert_eq!(result, Ok(()));
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(setup.server.downloads().len(), 1, "only the first install downloaded");
}

#[tokio::test]
async fn a_re_uploaded_archive_replaces_the_install() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dir = setup.dir(&folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libcudart.so.13.old"), b"from the earlier upload").unwrap();
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
async fn offline_an_existing_install_is_used_as_it_is() {
    let setup = setup();
    let folder = setup.publish("cuda", "13.3.0");
    let dependency = setup.libraries("NVIDIA CUDA", &folder);
    assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
    let before = snapshot(setup.root.path());

    setup.server.fail_api(503);
    let (result, events) = setup.install(std::slice::from_ref(&dependency)).await;
    assert_eq!(result, Ok(()), "a 5xx counts as unreachable");
    assert!(events.is_empty(), "{events:?}");

    let unreachable = Dependency::libraries(
        "NVIDIA CUDA",
        Source::parse(&format!("{}/test/general/tree/main/{folder}", Server::closed_base())).unwrap(),
    );
    let (result, events) = setup.install(&[unreachable]).await;
    assert_eq!(result, Ok(()), "no network at all");
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(snapshot(setup.root.path()), before, "nothing touched");
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
            dependency: "NVIDIA CUDA",
            failure: Failure::Network
        })
    );

    // A marker without its library is no install of the runtime.
    let dir = setup.root.path().join("onnx/1.30.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(MARKER), "abc").unwrap();
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
            dependency: "ONNX Runtime",
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
    assert_eq!(
        result,
        Err(InstallError {
            dependency: "NVIDIA TensorRT",
            failure: Failure::Damaged
        })
    );
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
            dependency: "NVIDIA CUDA"
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
async fn a_dropped_install_keeps_the_partial() {
    let setup = setup();
    let folder = setup.publish_bytes("cuda", "13.3.0", LARGE);
    let path = format!("{folder}/{}", asset("cuda"));
    setup.server.hang_after(&path, LARGE.len() * 3 / 4);
    let dependency = setup.libraries("NVIDIA CUDA", &folder);

    let (sender, mut progress) = tokio::sync::mpsc::unbounded_channel();
    let root = setup.root.path().to_path_buf();
    let task = tokio::spawn(async move {
        install_retrying(&root, &[dependency], 0, move |event| {
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
async fn only_what_is_missing_reports_progress() {
    let setup = setup();
    let cuda = setup.publish("cuda", "13.3.0");
    let tensorrt = setup.publish("tensorrt", "10.14.1");
    let dependencies = [
        setup.libraries("NVIDIA CUDA", &cuda),
        setup.libraries("NVIDIA TensorRT", &tensorrt),
    ];
    assert_eq!(setup.install(&dependencies[..1]).await.0, Ok(()));

    let (result, events) = setup.install(&dependencies).await;
    assert_eq!(result, Ok(()));
    assert!(!events.is_empty());
    assert!(
        names(&events).iter().all(|name| *name == "NVIDIA TensorRT"),
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
            dependency: "NVIDIA cuDNN",
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
            dependency: "ONNX Runtime",
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

#[tokio::test]
async fn installing_one_version_leaves_another_alone() {
    let setup = setup();
    let other = setup.root.path().join("onnx/1.31.0");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("libonnxruntime.1.31.0.dylib"), b"another app's runtime").unwrap();
    std::fs::write(other.join(MARKER), "abc").unwrap();
    let before = snapshot(&other);

    let folder = setup.publish("onnx", "1.30.0");
    assert_eq!(
        setup.install(&[setup.libraries("ONNX Runtime", &folder)]).await.0,
        Ok(())
    );
    assert_eq!(snapshot(&other), before);
    assert_eq!(entries(&setup.root.path().join("onnx/1.30.0")), INSTALLED);
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
    async fn a_current_runtime_reports_nothing() {
        let setup = setup();
        let folder = setup.publish("onnx", "1.30.0");
        let dependency = runtime(&setup, &folder);
        assert_eq!(setup.install(std::slice::from_ref(&dependency)).await.0, Ok(()));
        let (result, events) = setup.install(&[dependency]).await;
        assert_eq!((result, events), (Ok(()), Vec::new()));
        assert_eq!(runtime_library(&setup.dir(&folder)), Some(setup.dir(&folder).join(LIB)));
    }
}
