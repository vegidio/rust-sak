use serde::Serialize;
use sha2::{Digest, Sha256};

/// Builds a cache key one part at a time, for parts that are not all the same type.
///
/// Order matters, and every part contributes: a value that cannot be serialized adds a marker derived from its type
/// rather than being dropped, so two keys that differ only in an unserializable part stay different.
///
/// ```
/// use rust_sak::memo::KeyBuilder;
///
/// let key = KeyBuilder::new().part("search").part(&42_u32).part(&true).finish();
///
/// assert_eq!(key.len(), 64);
/// assert_ne!(key, KeyBuilder::new().part(&42_u32).part("search").part(&true).finish());
/// ```
///
/// Prefer a struct, a tuple or a [`BTreeMap`](std::collections::BTreeMap) for map-shaped parts. A
/// [`HashMap`](std::collections::HashMap) serializes in its own randomized iteration order, so it produces a
/// different key on every run.
#[derive(Clone, Debug, Default)]
pub struct KeyBuilder {
    /// The running digest. Each part is fed in, followed by a single NUL separator.
    hasher: Sha256,
}

impl KeyBuilder {
    /// Starts an empty key.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one part. Calls accumulate, and **order matters**.
    pub fn part<T>(mut self, part: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
        match serde_json::to_vec(part) {
            Ok(bytes) => self.hasher.update(&bytes),
            // Skipping the part outright — separator and all — would make a key built from an unserializable value
            // equal to one built without it, so two different computations would share a cache entry. Fold in a
            // type-tagged marker instead, so the part still contributes.
            Err(_) => self
                .hasher
                .update(format!("!unserializable:{}", std::any::type_name::<T>()).as_bytes()),
        }

        // The separator is what makes the key order-sensitive: without it, ["ab", "c"] and ["a", "bc"] would hash
        // identically.
        self.hasher.update([0]);
        self
    }

    /// Finishes the key: the SHA-256 of every part, as a lowercase hex string.
    pub fn finish(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}
