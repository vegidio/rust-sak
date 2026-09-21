//! The export thread.

use std::sync::{Arc, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value as Json;

use super::pipeline::Shared;
use super::record::now_unix_nano;
use super::{O11yError, Signal, level, metric, otlp};

/// The longest the worker will wait between attempts after repeated failures.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Starts the export thread.
///
/// It is a plain OS thread rather than a task on the calling application's runtime. The kernel schedules it
/// independently, so serialising a batch and waiting on a TLS handshake never competes with the application's own
/// work — and the module stays usable from a program that has no async runtime at all.
pub(super) fn spawn(shared: Arc<Shared>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("rust-sak-o11y".to_string())
        .spawn(move || run(shared))
        .unwrap_or_else(|_| std::thread::spawn(|| {}))
}

/// The export loop.
fn run(shared: Arc<Shared>) {
    // If this thread dies — a panicking `on_export_error` callback is the realistic way — the gates shut rather
    // than leaving producers to fill buffers nobody will ever drain.
    let _guard = Sentinel;

    // Built here rather than in `init` so that every request happens on this thread. A `reqwest::blocking` client
    // owns a runtime of its own, and constructing or calling it from inside someone else's async context is the
    // documented way to deadlock.
    let client = reqwest::blocking::Client::builder()
        .timeout(shared.timeout)
        .build()
        .unwrap_or_default();

    let mut backoff = Duration::ZERO;

    loop {
        let shutting_down = wait(&shared, backoff);

        match export(&client, &shared) {
            true => backoff = Duration::ZERO,
            // An unreachable collector must not mean a full-timeout attempt every interval forever.
            false => backoff = next_backoff(backoff, shared.flush_interval),
        }

        {
            let mut control = shared.control.lock().unwrap_or_else(PoisonError::into_inner);
            control.flush_requested = false;
            control.generation += 1;
            shared.wake.notify_all();
        }

        if shutting_down {
            return;
        }
    }
}

/// Shuts the gates when the worker stops for any reason, including a panic.
struct Sentinel;

impl Drop for Sentinel {
    fn drop(&mut self) {
        level::silence();
    }
}

/// Waits for the next export, returning whether it was woken to shut down.
///
/// Three things end the wait: the flush interval elapsing, a producer pushing the record that reaches
/// `max_batch_size`, and [`shutdown`](super::shutdown).
fn wait(shared: &Shared, backoff: Duration) -> bool {
    let deadline = Instant::now() + shared.flush_interval + backoff;
    let mut control = shared.control.lock().unwrap_or_else(PoisonError::into_inner);

    loop {
        if control.shutdown {
            return true;
        }

        if control.flush_requested {
            return false;
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };

        let (guard, timeout) = shared
            .wake
            .wait_timeout(control, remaining)
            .unwrap_or_else(PoisonError::into_inner);

        control = guard;

        if timeout.timed_out() {
            return control.shutdown;
        }
    }
}

/// Drains the buffers and posts whatever is there. Returns whether every attempt succeeded.
pub(super) fn export(client: &reqwest::blocking::Client, shared: &Shared) -> bool {
    // Each buffer leaves the lock in one move, so serialising and sending below hold nothing at all.
    let (logs, logs_dropped) = shared.logs.take();
    let (spans, spans_dropped) = shared.spans.take();
    let metrics = metric::snapshot();

    report_drops(shared, Signal::Logs, logs_dropped);
    report_drops(shared, Signal::Traces, spans_dropped);

    let mut ok = true;

    if !logs.is_empty() {
        ok &= post(client, shared, Signal::Logs, otlp::logs(logs, &shared.resource));
    }

    if !spans.is_empty() {
        ok &= post(client, shared, Signal::Traces, otlp::traces(spans, &shared.resource));
    }

    if !metrics.is_empty() {
        let payload = otlp::metrics(
            metrics,
            &shared.resource,
            &shared.enrichment.attributes(),
            shared.start_unix_nano,
            now_unix_nano(),
        );

        ok &= post(client, shared, Signal::Metrics, payload);
    }

    ok
}

/// Posts one payload, reporting any failure through the callback. Returns whether it was accepted.
fn post(client: &reqwest::blocking::Client, shared: &Shared, signal: Signal, payload: Json) -> bool {
    let url = format!("{}{}", shared.endpoint.trim_end_matches('/'), signal.path());
    let mut request = client.post(url).json(&payload);

    for (name, value) in &shared.headers {
        request = request.header(name, value);
    }

    match request.send() {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            let status = response.status().as_u16();
            // Truncated because a collector that is unhappy sometimes says so at length, and this is destined for
            // someone else's logger.
            let body = response.text().unwrap_or_default().chars().take(512).collect();

            report(shared, &O11yError::ExportRejected { signal, status, body });
            false
        }
        Err(error) => {
            report(shared, &O11yError::Http(error));
            false
        }
    }
}

/// Reports discarded records, if there were any.
fn report_drops(shared: &Shared, signal: Signal, count: u64) {
    if count > 0 {
        report(shared, &O11yError::Dropped { signal, count });
    }
}

/// Hands an error to the configured callback.
fn report(shared: &Shared, error: &O11yError) {
    (shared.on_export_error)(error);
}

/// The next wait after a failure: double it, starting at the flush interval, capped at [`MAX_BACKOFF`].
fn next_backoff(current: Duration, flush_interval: Duration) -> Duration {
    if current.is_zero() {
        return flush_interval.min(MAX_BACKOFF);
    }

    (current * 2).min(MAX_BACKOFF)
}

/// Runs one export cycle against `shared`, building a client for it. Test-only.
#[cfg(test)]
pub(super) fn export_once(shared: &Shared) -> bool {
    let client = reqwest::blocking::Client::builder()
        .timeout(shared.timeout)
        .build()
        .unwrap_or_default();

    export(&client, shared)
}
