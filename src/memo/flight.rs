use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
#[cfg(feature = "memo-async")]
use std::task::Waker;

use super::MemoError;

/// What a finished computation hands to everyone waiting on it.
///
/// The leader takes its own result from here too, so leader and followers return byte-identical values and the very
/// same error instance.
#[derive(Clone, Debug)]
pub(super) enum Outcome {
    /// The encoded value — header and payload — exactly as it was offered to the store.
    Ready(Arc<[u8]>),
    /// The `compute` closure failed.
    Compute(Arc<dyn std::error::Error + Send + Sync + 'static>),
    /// The value could not be encoded. `postcard::Error` is [`Clone`], so every waiter gets an equal one.
    Encode(postcard::Error),
    /// The leader went away without publishing: it panicked, or its future was dropped.
    Abandoned,
}

impl Outcome {
    /// Turns everything but a successful result into the error the caller sees.
    pub(super) fn into_error(self) -> Option<MemoError> {
        match self {
            Outcome::Ready(_) => None,
            Outcome::Compute(err) => Some(MemoError::Compute(err)),
            Outcome::Encode(err) => Some(MemoError::Encode(err)),
            Outcome::Abandoned => Some(MemoError::ComputeAbandoned),
        }
    }
}

/// One in-flight computation, and everybody waiting on it.
#[derive(Debug)]
pub(super) struct Call {
    /// Published exactly once, by the leader. `None` while the computation is still running.
    outcome: Mutex<Option<Outcome>>,
    /// Wakes the callers blocked in [`Call::wait`].
    ready: Condvar,
    /// The wakers of the async callers parked on this call, drained when the outcome is published.
    ///
    /// Locked only *inside* the `outcome` lock, never the other way round. Only the async path parks anything here,
    /// so without `memo-async` this module is entirely synchronous.
    #[cfg(feature = "memo-async")]
    wakers: Mutex<Vec<Waker>>,
}

impl Call {
    fn new() -> Self {
        Call {
            outcome: Mutex::new(None),
            ready: Condvar::new(),
            #[cfg(feature = "memo-async")]
            wakers: Mutex::new(Vec::new()),
        }
    }

    /// Publishes `outcome` to every waiter, sync and async alike. Later calls are ignored.
    pub(super) fn publish(&self, outcome: Outcome) {
        let mut slot = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        *slot = Some(outcome);

        #[cfg(feature = "memo-async")]
        let wakers = std::mem::take(&mut *self.wakers.lock().unwrap_or_else(PoisonError::into_inner));
        drop(slot);

        self.ready.notify_all();

        #[cfg(feature = "memo-async")]
        for waker in wakers {
            waker.wake();
        }
    }

    /// Blocks until the leader publishes, then returns what it published.
    pub(super) fn wait(&self) -> Outcome {
        let mut slot = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = slot.as_ref() {
                return outcome.clone();
            }

            slot = self.ready.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The outcome if it has been published, otherwise parks `waker` to be woken when it is.
    ///
    /// Registration happens while the `outcome` lock is still held, which is what closes the window between seeing
    /// `None` and being on the list — a leader publishing in between would otherwise wake nobody.
    #[cfg(feature = "memo-async")]
    pub(super) fn poll(&self, waker: &Waker) -> Option<Outcome> {
        let slot = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(outcome) = slot.as_ref() {
            return Some(outcome.clone());
        }

        let mut wakers = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        if !wakers.iter().any(|parked| parked.will_wake(waker)) {
            wakers.push(waker.clone());
        }

        None
    }
}

/// The computations currently running, keyed by type fingerprint and cache key.
#[derive(Debug, Default)]
pub(super) struct Flight {
    /// Held only long enough to look up or install a [`Call`] — never across a computation.
    calls: Mutex<HashMap<Arc<str>, Arc<Call>>>,
}

impl Flight {
    /// Joins the computation already running for `key`, or installs one and becomes its leader.
    ///
    /// Takes the key by value, and shares one `Arc<str>` between the map and the leader's guard, so becoming a
    /// leader allocates the key once rather than once per holder.
    ///
    /// A leader gets a guard whose [`Drop`] releases every waiter, so a panic or a dropped future cannot strand them.
    pub(super) fn enter(&self, key: String) -> Entry<'_> {
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(call) = calls.get(key.as_str()) {
            return Entry::Follower(Arc::clone(call));
        }

        let key: Arc<str> = Arc::from(key);
        let call = Arc::new(Call::new());
        calls.insert(Arc::clone(&key), Arc::clone(&call));

        Entry::Leader(Leader {
            flight: self,
            key,
            call,
            published: false,
        })
    }

    /// Whether any computation is currently in flight. Test-only.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).is_empty()
    }
}

/// The outcome of [`Flight::enter`]: either the duty to compute, or a handle to somebody else's computation.
pub(super) enum Entry<'a> {
    /// This caller computes, and must publish through the guard.
    Leader(Leader<'a>),
    /// Somebody else is computing; wait on this.
    Follower(Arc<Call>),
}

/// The right — and the obligation — to compute one key.
pub(super) struct Leader<'a> {
    /// The table to deregister from once the computation settles.
    flight: &'a Flight,
    /// This computation's key in that table, sharing the map's allocation rather than copying it.
    key: Arc<str>,
    /// The call every follower is waiting on.
    call: Arc<Call>,
    /// Cleared once something has been published; still set at drop time means the leader vanished.
    published: bool,
}

impl Leader<'_> {
    /// Hands `outcome` to every waiter.
    pub(super) fn publish(&mut self, outcome: Outcome) {
        self.call.publish(outcome);
        self.published = true;
    }
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        self.flight
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(self.key.as_ref());

        // The leader unwound or was cancelled without publishing. Release the followers with something actionable
        // rather than leaving them blocked on a computation that will never finish.
        if !self.published {
            self.call.publish(Outcome::Abandoned);
        }
    }
}
