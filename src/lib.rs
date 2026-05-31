//! `rust-sak` — a "Swiss Army Knife" of reusable Rust building blocks.
//!
//! The crate is organized into independent modules (e.g. [`fetch`], [`crypto`]), each gated behind its own Cargo
//! feature so consumers compile only what they need:
//!
//! ```toml
//! rust-sak = { version = "0.1", features = ["fetch"] }
//! ```

#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "fetch")]
pub mod fetch;
