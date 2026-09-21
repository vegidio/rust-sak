//! Trace and span identifier generation.

use std::cell::Cell;

use super::record::{SpanId, TraceId};

thread_local! {
    /// This thread's generator state.
    ///
    /// Span creation is on the hot path, and a span needs two identifiers. Drawing them from the operating system
    /// each time would cost a `getrandom` call per span; seeding a per-thread generator once from a v4 UUID and
    /// drawing from that afterwards costs a few nanoseconds instead. Identifiers only need to not collide, so a
    /// fast non-cryptographic generator with a random seed is the right tool.
    static STATE: Cell<u64> = Cell::new(seed());
}

/// Draws a fresh seed from the operating system, via `uuid`'s v4 generator.
///
/// Never returns zero, which is the one state xorshift cannot leave.
fn seed() -> u64 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let seed = u64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0; 8]));

    if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed }
}

/// The next value from this thread's generator — xorshift64\*, which passes the usual statistical batteries and
/// compiles to a handful of instructions.
fn next() -> u64 {
    STATE.with(|state| {
        let mut value = state.get();
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        state.set(value);

        value.wrapping_mul(0x2545_F491_4F6C_DD1D)
    })
}

/// A fresh 16-byte trace identifier.
///
/// The all-zero identifier is reserved by the W3C trace-context specification as "invalid", so it is regenerated on
/// the astronomically unlikely occasion it comes up.
pub(super) fn new_trace_id() -> TraceId {
    loop {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&next().to_ne_bytes());
        bytes[8..].copy_from_slice(&next().to_ne_bytes());

        if bytes != [0; 16] {
            return TraceId(bytes);
        }
    }
}

/// A fresh 8-byte span identifier. Zero is reserved, as for [`new_trace_id`].
pub(super) fn new_span_id() -> SpanId {
    loop {
        let value = next();

        if value != 0 {
            return SpanId(value.to_ne_bytes());
        }
    }
}
