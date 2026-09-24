//! GitHub release utilities.
//!
//! This module queries the public GitHub REST API for a repository's releases:
//!
//! - [`get_latest_release`] fetches the latest published [`Release`] (drafts and pre-releases excluded, as GitHub
//!   defines "latest").
//! - [`is_outdated_release`] tells whether a given version is older than that release's tag, by semantic version
//!   comparison — the usual "is there an update?" check.
//!
//! Both are async and report failures as a [`GithubError`].
//!
//! ```no_run
//! use rust_sak::github::is_outdated_release;
//!
//! # async fn run() -> Result<(), rust_sak::github::GithubError> {
//! if is_outdated_release("vegidio", "mediasim", "1.0.0").await? {
//!     println!("A newer version is available");
//! }
//! # Ok(())
//! # }
//! ```

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod error;
mod get_latest_release;
mod is_outdated_release;
mod release;

pub use error::GithubError;
pub use get_latest_release::get_latest_release;
pub use is_outdated_release::is_outdated_release;
pub use release::Release;

/// Root of the public GitHub REST API.
const API_BASE_URL: &str = "https://api.github.com";

#[cfg(test)]
mod tests;
