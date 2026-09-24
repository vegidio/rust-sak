use semver::Version;

use super::get_latest_release::get_latest_release_from;
use super::{API_BASE_URL, GithubError};

/// Checks whether `version` is older than the latest release of the GitHub repository `owner/repo`.
///
/// The latest release is fetched with [`get_latest_release`](super::get_latest_release) and its `tag_name` is
/// compared to `version` by semantic versioning. Both sides are parsed leniently: a leading `v` is optional
/// (`v1.2.0` and `1.2.0` are equal) and missing components are zero (`1.2` is `1.2.0`). A pre-release is older than
/// its release (`1.2.0-rc.1` < `1.2.0`), and build metadata (`+…`) is ignored.
///
/// Returns `false` when `version` is equal to or newer than the latest release.
///
/// ```no_run
/// use rust_sak::github::is_outdated_release;
///
/// # async fn run() -> Result<(), rust_sak::github::GithubError> {
/// let outdated = is_outdated_release("vegidio", "mediasim", env!("CARGO_PKG_VERSION")).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`GithubError::Request`] if the latest release cannot be fetched, and [`GithubError::InvalidVersion`] if
/// `version` or the release's tag is not a semantic version.
pub async fn is_outdated_release(owner: &str, repo: &str, version: &str) -> Result<bool, GithubError> {
    is_outdated_release_from(API_BASE_URL, owner, repo, version).await
}

/// [`is_outdated_release`] against an arbitrary API root, so tests can point it at a local server.
pub(super) async fn is_outdated_release_from(
    base_url: &str,
    owner: &str,
    repo: &str,
    version: &str,
) -> Result<bool, GithubError> {
    // Parsed before the request, so a malformed input fails without spending a rate-limited call.
    let current = parse_version(version)?;
    let latest = get_latest_release_from(base_url, owner, repo).await?;

    Ok(is_newer(&parse_version(&latest.tag_name)?, &current))
}

/// Whether `latest` is strictly newer than `current`, ignoring build metadata as semver precedence requires.
pub(super) fn is_newer(latest: &Version, current: &Version) -> bool {
    latest.cmp_precedence(current).is_gt()
}

/// Parses `raw` as a semantic version, accepting an optional leading `v`/`V` and missing minor/patch components.
pub(super) fn parse_version(raw: &str) -> Result<Version, GithubError> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);

    // Pad only the numeric core: the pre-release and build suffixes follow the first `-` or `+`.
    let core_end = stripped.find(['-', '+']).unwrap_or(stripped.len());
    let (core, suffix) = stripped.split_at(core_end);
    let padding = match core.matches('.').count() {
        0 => ".0.0",
        1 => ".0",
        _ => "",
    };

    Version::parse(&format!("{core}{padding}{suffix}")).map_err(|source| GithubError::InvalidVersion {
        version: raw.to_owned(),
        source,
    })
}
