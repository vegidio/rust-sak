use super::get_latest_release::get_latest_release_from;
use super::is_outdated_release::{is_newer, is_outdated_release_from, parse_version};
use super::*;

use crate::fetch::test_support::{read_request, write_response};
use tokio::net::TcpListener;

// --- version comparison ---
//
// Ported from go-sak's `TestIsOutdatedRelease_VersionComparison`, plus the cases the Rust parser adds.

fn outdated(latest: &str, current: &str) -> bool {
    is_newer(&parse_version(latest).unwrap(), &parse_version(current).unwrap())
}

#[test]
fn compares_versions() {
    let cases = [
        ("v1.2.0", "1.1.0", true),
        ("v1.2.0", "v1.1.0", true),
        ("v1.2.0", "1.2.0", false),
        ("v1.2.0", "1.3.0", false),
        ("1.2.0", "1.1.0", true),
        ("v1.2.3", "v1.2.2", true),
        ("v2.0.0", "v1.9.9", true),
        ("V1.2.0", "1.1.0", true),
        ("1.10.0", "1.9.0", true),
        ("1.2.0", "1.2.0-rc.1", true),
        ("1.2.0-rc.2", "1.2.0-rc.1", true),
        ("1.2.0+build.2", "1.2.0+build.1", false),
    ];

    for (latest, current, expected) in cases {
        assert_eq!(
            outdated(latest, current),
            expected,
            "latest {latest}, current {current}"
        );
    }
}

#[test]
fn pads_missing_components() {
    assert_eq!(parse_version("v1").unwrap(), parse_version("1.0.0").unwrap());
    assert_eq!(parse_version("1.2").unwrap(), parse_version("1.2.0").unwrap());
    assert_eq!(parse_version("1.2-beta").unwrap(), parse_version("1.2.0-beta").unwrap());
    assert_eq!(parse_version(" 1.2.0 ").unwrap(), parse_version("1.2.0").unwrap());
}

#[test]
fn rejects_invalid_versions() {
    for raw in ["", "v", "invalid-version", "1.2.3.4", "1.x.0"] {
        match parse_version(raw) {
            Err(GithubError::InvalidVersion { version, .. }) => assert_eq!(version, raw),
            other => panic!("{raw:?} should be invalid, got {other:?}"),
        }
    }
}

// --- API calls ---
//
// These point the functions at the throwaway local HTTP/1.1 server in `crate::fetch::test_support`, so they exercise
// the real request path without reaching the network.

fn release_json(tag: &str) -> String {
    format!(
        r#"{{"tag_name":"{tag}","name":"Release {tag}","html_url":"https://github.com/o/r/releases/tag/{tag}","published_at":"2026-09-01T12:00:00Z","prerelease":false,"draft":false,"id":1}}"#
    )
}

/// Serves one request with `status` and `body`, returning the base URL and a handle yielding the raw request.
async fn serve_once(status: &'static str, body: String) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        write_response(&mut stream, status, &body).await;
        request
    });

    (base, server)
}

#[tokio::test]
async fn get_latest_release_decodes_the_release() {
    let (base, server) = serve_once("200 OK", release_json("v1.2.0")).await;

    let release = get_latest_release_from(&base, "owner", "repo").await.unwrap();
    assert_eq!(
        release,
        Release {
            tag_name: "v1.2.0".into(),
            name: Some("Release v1.2.0".into()),
            html_url: "https://github.com/o/r/releases/tag/v1.2.0".into(),
            published_at: Some("2026-09-01T12:00:00Z".into()),
            prerelease: false,
            draft: false,
        }
    );

    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /repos/owner/repo/releases/latest "));
    assert!(request.contains("user-agent: rust-sak/"));
    assert!(request.contains("accept: application/vnd.github+json"));
}

#[tokio::test]
async fn is_outdated_release_is_true_for_an_older_version() {
    let (base, _server) = serve_once("200 OK", release_json("v1.2.0")).await;
    assert!(is_outdated_release_from(&base, "owner", "repo", "1.1.0").await.unwrap());
}

#[tokio::test]
async fn is_outdated_release_is_false_for_the_same_version() {
    let (base, _server) = serve_once("200 OK", release_json("v1.2.0")).await;
    assert!(
        !is_outdated_release_from(&base, "owner", "repo", "v1.2.0")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn missing_release_is_a_request_error() {
    let (base, _server) = serve_once("404 Not Found", r#"{"message":"Not Found"}"#.into()).await;

    match is_outdated_release_from(&base, "owner", "repo", "1.0.0").await {
        Err(GithubError::Request(err)) => assert_eq!(err.status(), Some(reqwest::StatusCode::NOT_FOUND)),
        other => panic!("expected a request error, got {other:?}"),
    }
}

#[tokio::test]
async fn non_semver_tag_is_an_invalid_version() {
    let (base, _server) = serve_once("200 OK", release_json("nightly")).await;

    match is_outdated_release_from(&base, "owner", "repo", "1.0.0").await {
        Err(GithubError::InvalidVersion { version, .. }) => assert_eq!(version, "nightly"),
        other => panic!("expected an invalid version, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_input_fails_before_any_request() {
    // Nothing listens on this base URL: reaching the network would surface as a `Request` error instead.
    let result = is_outdated_release_from("http://127.0.0.1:1", "owner", "repo", "not-a-version").await;
    assert!(matches!(result, Err(GithubError::InvalidVersion { .. })));
}
