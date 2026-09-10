use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::flight::{Call, Outcome};
use super::store::Entry;
use super::{
    Memo, Result, decode_entry, encode_or_publish, flight, flight_key, header, publish_compute_error, resolve,
};

impl Memo {
    /// The `async` counterpart of [`get_or_compute`](Memo::get_or_compute), for computations that are themselves
    /// futures.
    ///
    /// It shares one in-flight table with the synchronous method, so a sync and an async caller racing on the same
    /// key still compute it once. Blocking store operations are moved off the runtime with
    /// `tokio::task::spawn_blocking`; a memory-only cache is read inline, because a lookup there costs less than
    /// dispatching it would.
    ///
    /// Cancellation is honoured: dropping this future — under `tokio::time::timeout`, say — releases every caller
    /// waiting on it with [`MemoError::ComputeAbandoned`] rather than stranding them.
    ///
    /// ```
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use rust_sak::memo::{CacheOpts, Memo};
    ///
    /// let memo = Memo::memory(CacheOpts::new())?;
    ///
    /// let body: String = memo
    ///     .get_or_compute_async("home", Duration::from_secs(60), || async { fetch_home().await })
    ///     .await?;
    ///
    /// # async fn fetch_home() -> Result<String, std::convert::Infallible> {
    /// #     Ok("<html>".to_owned())
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`get_or_compute`](Memo::get_or_compute).
    ///
    /// # Panics
    ///
    /// Panics if a disk-backed cache is used outside a Tokio runtime, because the store is reached through
    /// `spawn_blocking`. A memory-only cache needs no runtime.
    pub async fn get_or_compute_async<T, F, Fut, E>(&self, key: &str, ttl: Duration, compute: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = std::result::Result<T, E>>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let fingerprint = header::fingerprint::<T>();

        if let Some((_, value)) = self.load_async(key, fingerprint).await {
            return Ok(value);
        }

        let mut leader = match self.flight.enter(flight_key(fingerprint, key)) {
            flight::Entry::Follower(call) => return resolve(Waiter { call }.await, fingerprint),
            flight::Entry::Leader(leader) => leader,
        };

        // Look again now that we hold the flight, as the synchronous path does.
        if let Some((bytes, value)) = self.load_async(key, fingerprint).await {
            leader.publish(Outcome::Ready(bytes));
            return Ok(value);
        }

        // The leader guard is held across this await, which is what lets a dropped future release the followers.
        let value = match compute().await {
            Ok(value) => value,
            Err(err) => return Err(publish_compute_error(&mut leader, err)),
        };

        let bytes = encode_or_publish(&mut leader, fingerprint, &value)?;

        self.store_set(key, Arc::clone(&bytes), ttl).await;

        leader.publish(Outcome::Ready(bytes));
        Ok(value)
    }

    /// Reads and decodes the entry under `key`, keeping a blocking store off the runtime.
    async fn load_async<T>(&self, key: &str, fingerprint: u64) -> Option<(Arc<[u8]>, T)>
    where
        T: DeserializeOwned,
    {
        decode_entry(fingerprint, self.store_get(key).await?.value)
    }

    /// Reads one raw entry. A store failure — or a `spawn_blocking` that could not run — is a miss.
    async fn store_get(&self, key: &str) -> Option<Entry> {
        // A store with no directory keeps everything in memory, so calling it inline beats a `spawn_blocking`
        // round-trip, which costs more than the moka lookup it would be wrapping.
        if self.store.path().is_none() {
            return self.store.get(key).ok().flatten();
        }

        let store = Arc::clone(&self.store);
        let key = key.to_owned();

        tokio::task::spawn_blocking(move || store.get(&key))
            .await
            .ok()?
            .ok()
            .flatten()
    }

    /// Writes one raw entry, best-effort. A store failure, or a task that could not run, costs a future hit.
    async fn store_set(&self, key: &str, value: Arc<[u8]>, ttl: Duration) {
        // As `store_get`: no directory means no blocking I/O to move off the runtime.
        if self.store.path().is_none() {
            let _ = self.store.set(key, value, ttl);
            return;
        }

        let store = Arc::clone(&self.store);
        let key = key.to_owned();

        // The `Arc` moves into the task, so handing the value to a blocking store costs a refcount bump rather
        // than a copy of the whole payload.
        let _ = tokio::task::spawn_blocking(move || store.set(&key, value, ttl)).await;
    }
}

/// Parks an async caller on a computation somebody else is running.
struct Waiter {
    /// The call being waited on.
    call: Arc<Call>,
}

impl Future for Waiter {
    type Output = Outcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Outcome> {
        match self.call.poll(cx.waker()) {
            Some(outcome) => Poll::Ready(outcome),
            None => Poll::Pending,
        }
    }
}
