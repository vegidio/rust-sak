//! The bounded, drop-oldest queue sitting between the emitting threads and the export thread.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// A bounded queue that discards its oldest entry rather than growing or blocking.
///
/// This is where "never slow the caller down" is actually enforced. A full buffer means the collector is slower than
/// the application is emitting, and there are only three ways out: grow without limit, make the caller wait, or throw
/// something away. Only the third keeps a telemetry failure from becoming an application failure, and dropping the
/// *oldest* entry means what survives is the most recent picture rather than a stale prefix.
#[derive(Debug)]
pub(super) struct Buffer<T> {
    /// The queue and its drop tally, behind one lock so a push is a single acquisition.
    state: Mutex<State<T>>,
    /// How many entries the queue holds before it starts discarding.
    capacity: usize,
}

/// The mutable half of a [`Buffer`].
#[derive(Debug)]
struct State<T> {
    /// The queued entries, oldest first.
    queue: VecDeque<T>,
    /// Entries discarded since the last [`Buffer::take`], reported once rather than per drop.
    dropped: u64,
}

impl<T> Buffer<T> {
    /// Creates an empty buffer holding at most `capacity` entries.
    ///
    /// A `capacity` of zero is raised to one: a buffer that discards everything would make the module silently
    /// useless, and is far more likely a mistake than an intent.
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                dropped: 0,
            }),
            capacity: capacity.max(1),
        }
    }

    /// Appends `item`, discarding the oldest entry if the buffer is already full.
    ///
    /// Returns the queue length afterwards, which the caller compares against the batch threshold to decide whether
    /// to wake the export thread early.
    pub(super) fn push(&self, item: T) -> usize {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if state.queue.len() >= self.capacity {
            state.queue.pop_front();
            state.dropped += 1;
        }

        state.queue.push_back(item);
        state.queue.len()
    }

    /// Removes everything queued, returning it alongside the number of entries discarded since the last call.
    ///
    /// The lock is held only for the swap — serialising and sending happen with nothing held, so an emitting thread
    /// never waits on the network.
    pub(super) fn take(&self) -> (Vec<T>, u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let dropped = std::mem::replace(&mut state.dropped, 0);

        (state.queue.drain(..).collect(), dropped)
    }

    /// How many entries are queued right now.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).queue.len()
    }
}
