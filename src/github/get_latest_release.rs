use crate::fetch::Fetch;

use super::{API_BASE_URL, GithubError, Release};

/// Fetches the latest published release of the GitHub repository `owner/repo`.
///
/// "Latest" is GitHub's definition: the most recent release that is neither a draft nor a pre-release. The request
/// is unauthenticated, so it counts against GitHub's limit of 60 requests per hour per IP address.
///
/// # Errors
///
/// Returns [`GithubError::Request`] if the request fails, including when the repository does not exist or has no
/// releases (`404`) and when the rate limit is exhausted (`403`).
pub async fn get_latest_release(owner: &str, repo: &str) -> Result<Release, GithubError> {
    get_latest_release_from(API_BASE_URL, owner, repo).await
}

/// [`get_latest_release`] against an arbitrary API root, so tests can point it at a local server.
pub(super) async fn get_latest_release_from(base_url: &str, owner: &str, repo: &str) -> Result<Release, GithubError> {
    // GitHub rejects requests without a `User-Agent`, and `reqwest` sends none by default.
    let fetch = Fetch::new()
        .header("User-Agent", concat!("rust-sak/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    let url = format!("{base_url}/repos/{owner}/{repo}/releases/latest");
    Ok(fetch.json(url).await?)
}
