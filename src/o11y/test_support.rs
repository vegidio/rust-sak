//! Test-only helpers for `o11y`.
//!
//! The HTTP collector is shared with the end-to-end integration test, which is a separate binary and cannot reach
//! an in-crate `#[cfg(test)]` module. Including its file here rather than restating it is what keeps the two from
//! drifting — see the notes in that file.

#[path = "../../tests/common/mod.rs"]
mod common;

pub(super) use common::*;

use std::sync::{Mutex, PoisonError};

/// Serialises the tests that touch the process globals.
///
/// `cargo nextest` gives every test its own process, but `cargo llvm-cov --html` — which `scripts/coverage.sh`
/// runs by default — goes through `cargo test`, where the whole suite shares one process and therefore one set of
/// `static`s. Without this the two runners would disagree about whether the suite passes.
///
/// Poisoning is recovered from rather than propagated, so one failing test does not cascade into every other.
pub(super) fn global_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());

    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}
