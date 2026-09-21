//! `rust-sak` — a "Swiss Army Knife" of reusable Rust building blocks.
//!
//! The crate is organized into independent modules (e.g. [`fetch`], [`crypto`], [`fs`], [`image`], [`memo`],
//! [`o11y`], [`sysinfo`]), each gated behind its own Cargo feature so consumers compile only what they need:
//!
//! ```toml
//! rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["fetch"] }
//! ```
//!
//! The crate is not published to crates.io — it depends on its `o11y-macros` sibling by path, which
//! `cargo publish` rejects — so a git dependency is the supported way to take it.

// `o11y`'s `#[instrument]` macro expands to paths rooted at `::rust_sak`, which is how it must name this crate
// from a consumer's. Aliasing the crate to itself makes those same paths resolve from inside it too, so the macro
// works in this crate's own tests and doctests.
#[cfg(feature = "o11y")]
extern crate self as rust_sak;

#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "fetch")]
pub mod fetch;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "image")]
pub mod image;
#[cfg(feature = "memo")]
pub mod memo;
#[cfg(feature = "o11y")]
pub mod o11y;
#[cfg(feature = "sysinfo")]
pub mod sysinfo;
