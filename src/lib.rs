//! `rust-sak` — a "Swiss Army Knife" of reusable Rust building blocks.
//!
//! The crate is organized into independent modules (e.g. [`fetch`]), each gated behind its own Cargo feature so
//! consumers compile only what they need:
//!
//! ```toml
//! rust-sak = { version = "0.1", features = ["fetch"] }
//! ```

#[cfg(feature = "fetch")]
pub mod fetch;
