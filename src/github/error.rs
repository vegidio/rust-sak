/// Errors returned by the [`github`](super) module.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    /// The API request failed: a network error, a non-success status (`404` when the repository does not exist or
    /// has no releases, `403` when the unauthenticated rate limit is exhausted), or a body that is not a release.
    #[error("GitHub API request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// A version — the one passed in, or the latest release's tag — is not a semantic version.
    #[error("`{version}` is not a valid semantic version: {source}")]
    InvalidVersion {
        /// The string that failed to parse, as it was given.
        version: String,
        /// Why it failed to parse.
        source: semver::Error,
    },
}
