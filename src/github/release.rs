use serde::Deserialize;

/// A published GitHub release, as returned by the REST API (only the commonly useful fields).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// The git tag the release points at, e.g. `v1.2.0`. This is what version comparisons use.
    pub tag_name: String,
    /// The release title; free text, and `None` when the release was published without one.
    pub name: Option<String>,
    /// The release's page on github.com.
    pub html_url: String,
    /// When the release was published, as an ISO 8601 timestamp (e.g. `2026-09-01T12:00:00Z`).
    pub published_at: Option<String>,
    /// Whether the release is marked as a pre-release.
    pub prerelease: bool,
    /// Whether the release is an unpublished draft.
    pub draft: bool,
}
