# `github` module

Helpers for the public GitHub REST API: fetch a repository's latest release, and check whether a given version is older than it — the usual "is there an update?" check. Both functions are **async** (built on the [`fetch`](../fetch/README.md) module, so they need a Tokio runtime).

## Enabling

The module is gated behind the `github` Cargo feature, which also enables `fetch`:

```toml
[dependencies]
rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["github"] }
```

```rust
use rust_sak::github::{get_latest_release, is_outdated_release, GithubError, Release};
```

## Public API

| Item                  | Signature                                                                                           | What it does                                                                                                  |
|-----------------------|-----------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------|
| `get_latest_release`  | `async fn get_latest_release(owner: &str, repo: &str) -> Result<Release, GithubError>`              | Fetches the latest published release (GitHub's definition: newest that is neither a draft nor a pre-release). |
| `is_outdated_release` | `async fn is_outdated_release(owner: &str, repo: &str, version: &str) -> Result<bool, GithubError>` | `true` when `version` is older than the latest release's `tag_name`; `false` when equal or newer.             |
| `Release`             | `struct Release { tag_name, name, html_url, published_at, prerelease, draft }`                      | The commonly useful fields of a release.                                                                      |
| `GithubError`         | `enum GithubError { Request(reqwest::Error), InvalidVersion { version, source } }`                  | Why a call failed.                                                                                            |

### Version comparison

`is_outdated_release` compares the release's **`tag_name`** (not its free-text `name`) against `version` using semantic versioning, parsing both leniently:

- A leading `v` or `V` is optional — `v1.2.0` equals `1.2.0`.
- Missing components are zero — `1.2` is `1.2.0`, `1` is `1.0.0`.
- A pre-release is older than its release — `1.2.0-rc.1` < `1.2.0`.
- Build metadata (`+…`) is ignored.

## Usage

```rust,no_run
use rust_sak::github::{get_latest_release, is_outdated_release};

# async fn run() -> Result<(), rust_sak::github::GithubError> {
if is_outdated_release("vegidio", "mediasim", "1.0.0").await? {
    let latest = get_latest_release("vegidio", "mediasim").await?;
    println!("Version {} is available: {}", latest.tag_name, latest.html_url);
}
# Ok(())
# }
```

An update check usually should not stop the application when GitHub is unreachable; `.unwrap_or(false)` treats any failure as "not outdated":

```rust,no_run
# async fn run() {
let outdated = rust_sak::github::is_outdated_release("vegidio", "mediasim", "1.0.0")
    .await
    .unwrap_or(false);
# }
```

## Errors

- `GithubError::Request` — the request failed: network error, `404` (the repository does not exist or has no published release), `403` (rate limit exhausted), or a body that is not a release.
- `GithubError::InvalidVersion` — `version` or the release's tag is not a semantic version. `version` is validated before any request is sent.

## Rate limits

Requests are unauthenticated, so GitHub allows **60 per hour per IP address**. Check once per launch rather than in a loop.
