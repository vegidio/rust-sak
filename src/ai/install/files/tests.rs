use std::time::{Duration, SystemTime};

use super::*;
use crate::ai::hf::Hub;
use crate::ai::test_support::Server;
use crate::fetch::partial::part_path;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const NAME: &str = "x";

/// The graph, its weights beside it, and a model whose name starts the same but is not selected.
const GRAPH: &[u8] = b"the graph of x";
const DATA: &[u8] = b"the external weights of x";
const OTHER: &[u8] = b"another model, xy";

struct Setup {
    server: Server,
    root: crate::fs::TempDir,
    folder: Folder,
}

fn setup() -> Setup {
    let server = Server::start("test/models", COMMIT);
    server.put("models/x.onnx", GRAPH, true);
    server.put("models/x.onnx.data", DATA, true);
    server.put("models/xy.onnx", OTHER, true);
    let folder = Folder::parse(&server.tree_url("models")).unwrap();
    Setup {
        server,
        root: crate::fs::mk_temp_dir("rust-sak-ai-files").unwrap(),
        folder,
    }
}

impl Setup {
    fn files(&self) -> Files {
        Files {
            name: NAME.into(),
            folder: self.folder.clone(),
            select: Select::Prefix("x.onnx".into()),
            dir: self.dir(),
            derived: vec![self.derived()],
            trust: Trust::Published,
        }
    }

    fn trusting(&self) -> Files {
        Files {
            trust: Trust::LocalFiles,
            ..self.files()
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.path().join("models/x")
    }

    fn derived(&self) -> PathBuf {
        self.root.path().join("engines/x")
    }

    async fn listing(&self) -> Listing {
        Listing::read_with(Hub::for_folder(&self.folder).retries(0), &self.folder)
            .await
            .unwrap()
    }

    /// Install against a fresh listing, without retries, and return the outcome and every event.
    async fn install(&self, files: &Files) -> (Result<Outcome, InstallError>, Vec<Event>) {
        let listing = self.listing().await;
        install_with(files, Some(&listing)).await
    }

    /// Put `bytes` at `name` in the install's folder.
    fn place(&self, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(self.dir()).unwrap();
        std::fs::write(self.dir().join(name), bytes).unwrap();
    }

    /// Put a compiled file in the derived folder.
    fn compile(&self) {
        std::fs::create_dir_all(self.derived()).unwrap();
        std::fs::write(self.derived().join("engine.bin"), b"compiled").unwrap();
    }

    fn compiled(&self) -> bool {
        self.derived().join("engine.bin").exists()
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir().join(name)).unwrap()
    }

    fn downloads(&self) -> Vec<String> {
        self.server
            .downloads()
            .into_iter()
            .map(|request| request.path.rsplit('/').next().unwrap().to_owned())
            .collect()
    }
}

async fn install_with(files: &Files, listing: Option<&Listing>) -> (Result<Outcome, InstallError>, Vec<Event>) {
    let mut events = Vec::new();
    let result = install_files(files, listing, |event| events.push(event)).await;
    (result, events)
}

/// Whether `result` failed as damaged.
fn damaged(result: &Result<Outcome, InstallError>) -> bool {
    matches!(
        result,
        Err(InstallError {
            failure: Failure::Damaged { .. },
            ..
        })
    )
}

fn failed(failure: Failure) -> Result<Outcome, InstallError> {
    Err(InstallError {
        dependency: NAME.into(),
        failure,
    })
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into())
        .collect();
    names.sort();
    names
}

fn fractions(events: &[Event]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Progress { fraction, .. } => Some(*fraction),
            _ => None,
        })
        .collect()
}

fn verifying(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::Verifying { .. }))
        .count()
}

const INSTALLED: [&str; 3] = [MARKER, "x.onnx", "x.onnx.data"];

#[test]
fn a_prefix_takes_the_graph_and_its_weights_and_names_take_exactly_themselves() {
    let prefix = Select::Prefix("x.onnx".into());
    assert!(prefix.matches("x.onnx") && prefix.matches("x.onnx.data"));
    assert!(!prefix.matches("xy.onnx") && !prefix.matches("y.onnx"));
    let names = Select::Names(vec!["config.json".into()]);
    assert!(names.matches("config.json") && !names.matches("config.json.bak"));
}

#[tokio::test]
async fn a_fresh_install_downloads_the_selected_files_and_records_them() {
    let setup = setup();
    let (result, events) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(
        entries(&setup.dir()),
        INSTALLED,
        "staging is gone and xy.onnx was not selected"
    );
    assert_eq!(
        (setup.read("x.onnx"), setup.read("x.onnx.data")),
        (GRAPH.to_vec(), DATA.to_vec())
    );
    assert_eq!(setup.downloads(), ["x.onnx", "x.onnx.data"]);
    assert!(
        setup
            .server
            .downloads()
            .iter()
            .all(|request| request.path.contains(&format!("/resolve/{COMMIT}/"))),
        "every download is at the listing's commit"
    );
    assert!(files_present(&setup.files()));

    let fractions = fractions(&events);
    assert!(fractions.is_sorted(), "{fractions:?}");
    assert_eq!(fractions.last(), Some(&1.0));
    assert_eq!(
        fractions.iter().filter(|fraction| **fraction == 1.0).count(),
        1,
        "1 only at the end"
    );
    let total = (GRAPH.len() + DATA.len()) as u64;
    assert!(events.iter().all(|event| matches!(
        event,
        Event::Progress { phase: Phase::Download, total: t, dependency, .. } if *t == total && dependency == NAME
    )));
}

#[tokio::test]
async fn a_current_install_makes_no_request_reports_nothing_and_leaves_derived_alone() {
    let setup = setup();
    let listing = setup.listing().await;
    assert_eq!(
        install_with(&setup.files(), Some(&listing)).await.0,
        Ok(Outcome::Installed)
    );
    setup.compile();
    let requests = setup.server.requests().len();

    let (result, events) = install_with(&setup.files(), Some(&listing)).await;
    assert_eq!(result, Ok(Outcome::Current));
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(setup.server.requests().len(), requests, "no request");
    assert!(setup.compiled());
}

#[tokio::test]
async fn a_republished_file_replaces_the_old_one_and_empties_derived() {
    let setup = setup();
    assert_eq!(setup.install(&setup.files()).await.0, Ok(Outcome::Installed));
    setup.compile();
    std::fs::create_dir_all(setup.derived().join("coreml")).unwrap();
    setup.server.put("models/x.onnx", b"the graph of x, republished", true);

    let downloads = setup.downloads().len();
    let (result, events) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(setup.read("x.onnx"), b"the graph of x, republished");
    assert_eq!(
        setup.downloads()[downloads..],
        ["x.onnx"],
        "the unchanged weights are kept"
    );
    assert_eq!(verifying(&events), 0, "a file its record vouches for is not hashed");
    assert!(setup.derived().is_dir(), "the derived folder itself is kept");
    assert!(entries(&setup.derived()).is_empty());
    assert_eq!(entries(&setup.dir()), INSTALLED);
}

#[tokio::test]
async fn a_file_dropped_from_the_listing_is_removed_and_a_stray_manifest_with_it() {
    let setup = setup();
    setup.place(".manifest.json", b"{\"schema\":1}");
    setup.place("x.onnx.part", b"an earlier release's partial");
    assert_eq!(setup.install(&setup.files()).await.0, Ok(Outcome::Installed));
    assert_eq!(entries(&setup.dir()), INSTALLED);

    let server = Server::start("test/models", COMMIT);
    server.put("models/x.onnx", GRAPH, true);
    let folder = Folder::parse(&server.tree_url("models")).unwrap();
    let files = Files {
        folder: folder.clone(),
        ..setup.files()
    };
    let listing = Listing::read_with(Hub::for_folder(&folder).retries(0), &folder)
        .await
        .unwrap();
    assert_eq!(install_with(&files, Some(&listing)).await.0, Ok(Outcome::Adopted));
    assert_eq!(entries(&setup.dir()), [MARKER, "x.onnx"]);
}

#[tokio::test]
async fn a_selection_the_listing_does_not_name_fails_with_nothing_transferred() {
    let setup = setup();
    let files = Files {
        select: Select::Prefix("z.onnx".into()),
        ..setup.files()
    };
    assert_eq!(setup.install(&files).await.0, failed(Failure::NotListed));

    let partly = Files {
        select: Select::Names(vec!["x.onnx".into(), "missing.onnx".into()]),
        ..setup.files()
    };
    assert_eq!(setup.install(&partly).await.0, failed(Failure::NotListed));
    assert!(setup.downloads().is_empty());
}

#[tokio::test]
async fn a_listing_of_another_folder_is_refused() {
    let setup = setup();
    let listing = setup.listing().await;
    let files = Files {
        folder: Folder::parse(&setup.server.tree_url("other")).unwrap(),
        ..setup.files()
    };
    assert!(matches!(
        install_with(&files, Some(&listing)).await.0,
        Err(InstallError {
            failure: Failure::WrongListing { .. },
            ..
        })
    ));
}

#[tokio::test]
async fn a_listed_file_named_like_the_installers_own_is_refused() {
    let setup = setup();
    setup.server.put("models/.installed", b"{}", false);
    let files = Files {
        select: Select::Prefix(String::new()),
        ..setup.files()
    };
    let (result, _) = setup.install(&files).await;
    assert!(
        matches!(
            result,
            Err(InstallError {
                failure: Failure::Malformed(_),
                ..
            })
        ),
        "{result:?}"
    );
    assert!(setup.downloads().is_empty());
    assert!(!files.dir.join(MARKER).exists());
}

#[tokio::test]
async fn a_damaged_download_fails_and_is_deleted() {
    let setup = setup();
    setup.server.corrupt("models/x.onnx", b"not the graph that was listed");
    let (result, _) = setup.install(&setup.files()).await;
    assert_eq!(
        result,
        failed(Failure::Damaged {
            path: "models/x.onnx".into(),
            expected: crate::crypto::sha256_bytes(GRAPH),
            actual: crate::crypto::sha256_bytes(b"not the graph that was listed"),
        })
    );
    assert_eq!(setup.downloads(), ["x.onnx"], "a clean download is not retried");
    assert!(!setup.dir().join("x.onnx").exists());
    assert!(
        entries(&setup.dir().join(STAGING)).is_empty(),
        "nothing is left to resume"
    );
    assert!(!files_present(&setup.files()));
}

/// Bytes enough for an interrupted transfer to leave a partial, since `fetch` writes through a buffer.
fn large() -> Vec<u8> {
    (0..256 * 1024).map(|i: u32| (i.wrapping_mul(31) % 251) as u8).collect()
}

#[tokio::test]
async fn a_dropped_install_keeps_its_partial_and_the_next_one_resumes() {
    let setup = setup();
    let weights = large();
    setup.server.put("models/x.onnx.data", &weights, true);
    setup.server.hang_after("models/x.onnx.data", weights.len() / 2);
    let listing = setup.listing().await;

    let files = setup.files();
    let task = tokio::spawn({
        let listing = listing.clone();
        async move { install_files(&files, Some(&listing), |_| {}).await }
    });
    // Wait until bytes of the weights are on disk.
    let part = part_path(&setup.dir().join(STAGING).join("x.onnx.data"));
    tokio::time::timeout(Duration::from_secs(10), async {
        while std::fs::metadata(&part).map_or(0, |meta| meta.len()) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the weights start downloading");
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(std::fs::metadata(&part).unwrap().len() > 0, "the partial is kept");
    assert!(!files_present(&setup.files()));
    assert!(
        lock::try_acquire(&setup.dir()).unwrap().is_some(),
        "the lock went with the install"
    );

    setup.server.stop_hanging("models/x.onnx.data");
    let (result, events) = install_with(&setup.files(), Some(&listing)).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(setup.read("x.onnx.data"), weights);
    let resumed = setup.server.downloads().last().unwrap().range_from;
    assert!(resumed.is_some_and(|from| from > 0), "it resumed: {resumed:?}");
    assert!(fractions(&events).is_sorted());
    assert_eq!(fractions(&events).last(), Some(&1.0));
}

#[tokio::test]
async fn files_already_in_the_folder_that_match_are_adopted_with_nothing_downloaded() {
    let setup = setup();
    setup.place("x.onnx", GRAPH);
    setup.place("x.onnx.data", DATA);
    setup.place(".manifest.json", b"{\"schema\":1}");

    let (result, events) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Adopted));
    assert!(setup.downloads().is_empty());
    assert_eq!(verifying(&events), 2, "each file is hashed");
    assert_eq!(fractions(&events).last(), Some(&1.0));
    assert_eq!(entries(&setup.dir()), INSTALLED);

    let (result, events) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Current));
    assert!(events.is_empty(), "the next acquisition hashes nothing");
}

#[tokio::test]
async fn a_file_already_in_the_folder_that_does_not_match_is_downloaded() {
    let setup = setup();
    setup.place("x.onnx", b"the graph of z");
    setup.place("x.onnx.data", DATA);

    let (result, _) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(setup.downloads(), ["x.onnx"]);
    assert_eq!(setup.read("x.onnx"), GRAPH);
}

#[tokio::test]
async fn offline_an_intact_install_is_used_with_no_request() {
    let setup = setup();
    assert_eq!(setup.install(&setup.files()).await.0, Ok(Outcome::Installed));
    setup.compile();
    let requests = setup.server.requests().len();

    let (result, events) = install_with(&setup.files(), None).await;
    assert_eq!(result, Ok(Outcome::Offline));
    assert!(events.is_empty());
    assert_eq!(setup.server.requests().len(), requests);
    assert!(setup.compiled());
}

#[tokio::test]
async fn offline_with_nothing_intact_fails_unlisted() {
    let setup = setup();
    assert_eq!(install_with(&setup.files(), None).await.0, failed(Failure::Unlisted));

    // Files with no record are not proof of anything without a listing to hash them against.
    setup.place("x.onnx", GRAPH);
    setup.place("x.onnx.data", DATA);
    assert!(!files_present(&setup.files()));
    assert_eq!(install_with(&setup.files(), None).await.0, failed(Failure::Unlisted));

    assert_eq!(setup.install(&setup.files()).await.0, Ok(Outcome::Adopted));
    std::fs::remove_file(setup.dir().join("x.onnx.data")).unwrap();
    assert!(!files_present(&setup.files()));
    assert_eq!(install_with(&setup.files(), None).await.0, failed(Failure::Unlisted));
    assert!(setup.downloads().is_empty());
}

#[tokio::test]
async fn trusted_files_are_used_as_they_are_even_when_they_do_not_match() {
    let setup = setup();
    setup.place("x.onnx", b"a re-exported graph, not yet published");
    setup.place("x.onnx.data", DATA);
    let requests = setup.server.requests().len();

    let (result, events) = install_with(&setup.trusting(), None).await;
    assert_eq!(result, Ok(Outcome::Trusted), "found in the folder with no listing");
    assert!(events.is_empty());
    assert_eq!(setup.server.requests().len(), requests, "no request");

    let (result, _) = setup.install(&setup.trusting()).await;
    assert_eq!(result, Ok(Outcome::Trusted), "and with one");
    assert!(setup.downloads().is_empty());
    assert_eq!(setup.read("x.onnx"), b"a re-exported graph, not yet published");
    assert!(
        !files_present(&setup.files()),
        "a trusted record is not an intact install"
    );
}

#[tokio::test]
async fn a_trusted_install_missing_a_file_installs_and_verifies() {
    let setup = setup();
    setup.place("x.onnx", b"a re-exported graph");

    let (result, _) = setup.install(&setup.trusting()).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(setup.downloads(), ["x.onnx", "x.onnx.data"]);
    assert_eq!(setup.read("x.onnx"), GRAPH);

    setup.server.corrupt("models/x.onnx.data", b"damaged weights");
    std::fs::remove_file(setup.dir().join("x.onnx.data")).unwrap();
    let result = setup.install(&setup.trusting()).await.0;
    assert!(damaged(&result), "trust changes nothing about a download: {result:?}");
}

#[tokio::test]
async fn touching_a_trusted_file_empties_derived_once() {
    let setup = setup();
    setup.place("x.onnx", b"a re-exported graph");
    setup.place("x.onnx.data", DATA);
    assert_eq!(install_with(&setup.trusting(), None).await.0, Ok(Outcome::Trusted));
    setup.compile();
    assert_eq!(install_with(&setup.trusting(), None).await.0, Ok(Outcome::Trusted));
    assert!(setup.compiled(), "an unmoved stamp keeps what was compiled");

    let file = std::fs::File::options()
        .write(true)
        .open(setup.dir().join("x.onnx"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
    drop(file);
    assert_eq!(install_with(&setup.trusting(), None).await.0, Ok(Outcome::Trusted));
    assert!(!setup.compiled(), "a moved stamp discards it");

    setup.compile();
    assert_eq!(install_with(&setup.trusting(), None).await.0, Ok(Outcome::Trusted));
    assert!(setup.compiled(), "once");
}

#[tokio::test]
async fn a_later_install_without_trust_reinstalls_from_the_listing() {
    let setup = setup();
    setup.place("x.onnx", b"a re-exported graph, not yet published");
    setup.place("x.onnx.data", DATA);
    assert_eq!(install_with(&setup.trusting(), None).await.0, Ok(Outcome::Trusted));
    assert_eq!(
        install_with(&setup.files(), None).await.0,
        failed(Failure::Unlisted),
        "a trusted record is never used unchecked"
    );

    let (result, _) = setup.install(&setup.files()).await;
    assert_eq!(result, Ok(Outcome::Installed));
    assert_eq!(setup.downloads(), ["x.onnx"], "the matching weights are adopted");
    assert_eq!(setup.read("x.onnx"), GRAPH);
}

#[tokio::test]
async fn a_resumed_download_that_does_not_match_is_downloaded_once_more_from_nothing() {
    let setup = setup();
    let weights = large();
    setup.server.put("models/x.onnx.data", &weights, true);
    setup.server.hang_after("models/x.onnx.data", weights.len() / 2);
    let listing = setup.listing().await;

    let files = setup.files();
    let task = tokio::spawn({
        let listing = listing.clone();
        async move { install_files(&files, Some(&listing), |_| {}).await }
    });
    let part = part_path(&setup.dir().join(STAGING).join("x.onnx.data"));
    tokio::time::timeout(Duration::from_secs(10), async {
        while std::fs::metadata(&part).map_or(0, |meta| meta.len()) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the weights start downloading");
    task.abort();
    let _ = task.await;

    // The rest of the file now differs, so what the resume produces cannot match.
    setup.server.stop_hanging("models/x.onnx.data");
    let other: Vec<u8> = weights.iter().map(|byte| byte.wrapping_add(1)).collect();
    setup.server.corrupt("models/x.onnx.data", &other);
    let before = setup.server.downloads().len();
    let (result, _) = install_with(&setup.files(), Some(&listing)).await;
    assert!(damaged(&result), "{result:?}");
    let retried: Vec<Option<u64>> = setup.server.downloads()[before..]
        .iter()
        .map(|request| request.range_from)
        .collect();
    assert!(
        matches!(retried.as_slice(), [Some(from), None] if *from > 0),
        "a resume, then one download from nothing: {retried:?}"
    );
    assert!(!part.exists(), "nothing is left to resume");
}
