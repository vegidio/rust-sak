//! `rust-sak` — a "Swiss Army Knife" of reusable Rust building blocks.
//!
//! The crate is organized into independent modules (e.g. [`fetch`], [`crypto`], [`fs`], [`image`], [`memo`],
//! [`o11y`], [`sysinfo`]), each gated behind its own Cargo feature so consumers compile only what they need:
//!
//! ```toml
//! rust-sak = { version = "2", features = ["fetch"] }
//! ```

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
