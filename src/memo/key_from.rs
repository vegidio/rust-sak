use serde::Serialize;

use super::KeyBuilder;

/// Builds a deterministic cache key from a sequence of serializable values.
///
/// Each part is serialized independently and separated by a NUL byte, so parameter order is preserved, and the
/// result is the lowercase hex SHA-256 of the lot. Use it to avoid hand-formatting keys out of several values.
///
/// ```
/// use rust_sak::memo::key_from;
///
/// let key = key_from(["search", "shoes", "page-2"]);
///
/// assert_eq!(key.len(), 64);
/// assert_eq!(key, key_from(["search", "shoes", "page-2"]));
/// assert_ne!(key, key_from(["search", "page-2", "shoes"]));
/// ```
///
/// Every part contributes even if it cannot be serialized, in which case a marker derived from its type stands in for
/// it. Prefer a struct, a tuple or a [`BTreeMap`](std::collections::BTreeMap) for map-shaped parts — a
/// [`HashMap`](std::collections::HashMap) serializes in its own randomized iteration order and so produces a
/// different key on every run.
///
/// For parts that are not all the same type, reach for [`KeyBuilder`] instead.
pub fn key_from<T, I>(parts: I) -> String
where
    I: IntoIterator<Item = T>,
    T: Serialize,
{
    parts
        .into_iter()
        .fold(KeyBuilder::new(), |builder, part| builder.part(&part))
        .finish()
}
