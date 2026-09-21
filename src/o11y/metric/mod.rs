//! Typed metric instruments, created once and called many times.
//!
//! The three instruments — [`Counter`], [`Gauge`] and [`Histogram`] — are meant to live in a `static` behind a
//! [`LazyLock`](std::sync::LazyLock) and be called from anywhere:
//!
//! ```
//! use std::sync::LazyLock;
//! use rust_sak::o11y::metric::{self, Counter, Gauge, Histogram};
//!
//! static ORDERS: LazyLock<Counter> = LazyLock::new(|| metric::counter("orders_total"));
//! static ORDER_VALUE: LazyLock<Histogram> =
//!     LazyLock::new(|| metric::histogram("order_value_dollars").with_buckets(&[10.0, 50.0, 100.0, 500.0]));
//! static QUEUE_DEPTH: LazyLock<Gauge> = LazyLock::new(|| metric::gauge("queue_depth"));
//!
//! ORDERS.increment(1);
//! ORDERS.add_with_tags(1, &[("region", "eu-west-1")]);
//! ORDER_VALUE.record(129.5);
//! QUEUE_DEPTH.set(12);
//! ```
//!
//! Unlike logs and spans, **metrics are not gated and are not buffered**. An instrument accumulates into its own
//! atomics, and the exporter reads them on its own schedule. That has two consequences worth knowing:
//!
//! - A `static` instrument works before [`init`](super::init) has run, and in a process that never calls it. It is
//!   first touched by whatever code path reaches it, which for a `static` is not something the module can control.
//! - Recording costs the same whether telemetry is running or not — a single atomic operation, which is cheaper
//!   than the branch that would have skipped it.
//!
//! Values are reported **cumulatively**: a counter carries its running total since the process started, not the
//! change since the last export. A dropped export therefore loses nothing, which matters because this module drops
//! rather than retries when a collector is unreachable.

mod counter;
mod gauge;
mod histogram;
mod registry;
mod tags;

pub use counter::{Counter, counter};
pub use gauge::{Gauge, gauge};
pub use histogram::{Histogram, histogram};

pub(crate) use registry::{MetricData, MetricSnapshot, snapshot};

#[cfg(test)]
pub(crate) use tags::MAX_SERIES;

#[cfg(test)]
pub(crate) use registry::clear;
