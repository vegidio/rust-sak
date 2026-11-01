//! Loading the real ONNX Runtime 1.30.0 with `load_with`: the environment's name and the caller's logger. See
//! `ai_support` for the archive.
//!
//! A process loads one runtime, so this is a test binary of its own.

#![cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]

mod ai_support;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rust_sak::ai::ort::logging::LogLevel;
use rust_sak::ai::ort::session::Session;
use rust_sak::ai::{self, LoadOptions};

/// Every record the runtime sent the logger: its level, its logger id and its message.
static RECORDS: Mutex<Vec<(LogLevel, String, String)>> = Mutex::new(Vec::new());

#[tokio::test]
async fn the_environment_carries_the_name_and_its_records_reach_the_logger() {
    let Some((_root, dir)) = ai_support::installed().await else {
        return;
    };
    assert!(ai::environment().is_none(), "nothing before a load");

    let logger = Arc::new(
        |level: LogLevel, _category: &str, id: &str, _location: &str, message: &str| {
            RECORDS.lock().unwrap().push((level, id.to_owned(), message.to_owned()));
        },
    );
    ai::load_with(
        &dir,
        &LoadOptions {
            name: "io.vinicius.test",
            logger: Some(logger),
        },
    )
    .unwrap();

    // The runtime logs while it creates the environment, before the load clamps it to warning.
    let at_load = RECORDS.lock().unwrap().len();
    let environment = ai::environment().expect("reachable after the load");
    assert!(
        ai::webgpu_devices(&environment).count() > 0,
        "this Mac's GPU, through the plugin"
    );

    // An initializer no node reads, which the runtime removes with a warning.
    let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai/testdata/unused_initializer.onnx");
    Session::builder(&environment).unwrap().commit_from_file(model).unwrap();

    let records = RECORDS.lock().unwrap().clone();
    assert!(
        records
            .iter()
            .any(|(level, _, message)| *level == LogLevel::Warning && message.contains("unused")),
        "the warning reached the logger: {records:?}"
    );
    assert!(
        records.iter().any(|(_, id, _)| id == "io.vinicius.test"),
        "the environment's records carry its name: {records:?}"
    );
    assert!(
        records[at_load..].iter().all(|(level, ..)| *level >= LogLevel::Warning),
        "nothing below warning once loaded: {records:?}"
    );
}
