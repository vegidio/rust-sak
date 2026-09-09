# `memo` module

Memoization with pluggable storage: keep the result of an expensive call around for a while — in memory, on disk, or both — and don't compute it twice. Everything here is **synchronous**; the optional `memo-async` feature adds an `async` variant of the one method that runs your computation.

## Enabling

The module is gated behind the `memo` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "1", features = ["memo"] }
```

Add `memo-async` for [`get_or_compute_async`](#the-async-variant). It enables `memo` for you, and pulls in Tokio:

```toml
rust-sak = { version = "1", features = ["memo-async"] }
```

> This feature needs no toolchain beyond a Rust compiler — no C compiler, no downloads. The disk tier is [`redb`](https://crates.io/crates/redb), which is pure Rust.

## What it does

Wrap a computation that is expensive to repeat — an HTTP request, a database query, a hardware probe that shells out — and `Memo` will:

- **Cache the result** under a key you choose, for as long as the TTL you give it.
- **Deduplicate concurrent calls.** If ten threads ask for the same missing key at once, the computation runs once and all ten get that result.
- **Persist between runs**, if you pick a disk-backed store.

## Public API

| Function            | Signature                                                                        | What it does                                          |
|---------------------|----------------------------------------------------------------------------------|-------------------------------------------------------|
| `Memo::memory`      | `fn(CacheOpts) -> Result<Memo>`                                                  | A cache held entirely in memory.                      |
| `Memo::disk`        | `fn(impl AsRef<Path>, CacheOpts) -> Result<Memo>`                                | A cache backed by a database in a directory.          |
| `Memo::memory_disk` | `fn(impl AsRef<Path>, CacheOpts, Duration) -> Result<Memo>`                      | Memory in front of disk, with promotion.              |
| `get_or_compute`    | `fn(&str, Duration, F) -> Result<T>`                                             | The cached value, or `compute`'s, cached.             |
| `get_bytes`         | `fn(&str) -> Result<Option<Vec<u8>>>`                                            | The raw bytes under a key.                            |
| `set_bytes`         | `fn(&str, &[u8], Duration) -> Result<()>`                                        | Writes raw bytes under a key.                         |
| `path`              | `fn() -> Option<&Path>`                                                          | The backing directory, or `None` for memory-only.     |
| `cleanup`           | `fn() -> Result<()>`                                                             | Reclaims the space expired entries hold.              |
| `key_from`          | `fn(impl IntoIterator<Item = impl Serialize>) -> String`                         | A deterministic key from several values.              |

## Choosing a store

|                     | Survives restart | Speed          | Use it when                                                     |
|---------------------|------------------|----------------|-----------------------------------------------------------------|
| `Memo::memory`      | No               | Fastest        | Data is hot, cheap to recompute, and one process owns it        |
| `Memo::disk`        | Yes              | Disk-bound     | Results are expensive and must outlive the process              |
| `Memo::memory_disk` | Yes              | Fast on repeat | The usual pick: memory in front, disk behind it                 |

### Opening the same directory twice is safe

The database takes an exclusive file lock, so a second open of a directory would normally fail — with an error indistinguishable from *another process* holding it. `Memo::disk` and `Memo::memory_disk` therefore hand back the store this process already has open rather than trying to open it again:

```rust
# use rust_sak::memo::{CacheOpts, Memo};
# fn run() -> Result<(), Box<dyn std::error::Error>> {
let first = Memo::disk("/var/cache/myapp", CacheOpts::new())?;
let second = Memo::disk("/var/cache/myapp", CacheOpts::new())?;   // the same store, not a second one
# Ok(())
# }
```

That matters for anything whose setup can run twice: a library re-initialised without being torn down, a desktop app whose UI reloads while the backend keeps running, a test that opens a fixture per case. Directories are matched **after resolving symlinks**, so two links to one directory collide as they should. Sizing comes from whichever call opens the store; a caller handed an existing one gets its sizing.

The store closes when the last handle is dropped — there is no `close`. Handles share the store but not the deduplication, so prefer cloning a `Memo` over constructing a second one for the same directory.

### Memory + disk

Reads try memory first and fall back to disk; a disk hit is copied back into memory for `promote_ttl`, or for the entry's own remaining lifetime if that is shorter. Writes go to both tiers.

```rust
# use std::time::Duration;
# use rust_sak::memo::{CacheOpts, Memo};
# fn run() -> Result<(), Box<dyn std::error::Error>> {
let memo = Memo::memory_disk("/var/cache/myapp", CacheOpts::new(), Duration::from_secs(3600))?;
# Ok(())
# }
```

`promote_ttl` is independent of the TTL passed to `get_or_compute`. Pass `Duration::ZERO` to disable promotion, in which case disk hits are served without being cached in memory.

## Types

### `CacheOpts`

A consuming builder. Both figures are hints, clamped to whatever the engine accepts, so no value here can stop a store from opening.

| Method           | Default | Notes                                                                                  |
|------------------|---------|-----------------------------------------------------------------------------------------|
| `max_entries`    | 10,000  | Sizes the memory tier. The disk tier has nothing to map it onto and **ignores it**.     |
| `max_capacity`   | 1 GiB   | A real byte ceiling in memory; on disk it sizes the read cache, **not the directory**.  |

Passing `0` to either restores its default. **The disk cache is not size-bounded** — use TTLs and `cleanup` to keep it in check, and don't read `max_capacity(512 << 20)` as "this directory stays under 512 MiB".

### `KeyBuilder` and `key_from`

`key_from` hashes a sequence of serializable values into one key, so you don't hand-format strings:

```rust
use rust_sak::memo::key_from;

let key = key_from(["search", "shoes", "page-2"]);

assert_eq!(key, key_from(["search", "shoes", "page-2"]));
assert_ne!(key, key_from(["shoes", "search", "page-2"]));
```

Order matters, and parts are NUL-separated so `["ab", "c"]` and `["a", "bc"]` do not collide. Use `KeyBuilder` when the parts are not all the same type:

```rust
use rust_sak::memo::KeyBuilder;

let key = KeyBuilder::new().part("user").part(&42_u32).part(&true).finish();
```

A part that cannot be serialized contributes a marker derived from its type rather than being dropped, so it still separates one key from another. Prefer a struct, a tuple or a `BTreeMap` for map-shaped parts — a `HashMap` serializes in its own randomized iteration order, so it produces a **different key on every run**.

## Errors

Fallible calls return `memo::Result<T>`, aliasing `Result<T, MemoError>`:

- `Compute` — the computation failed. Held behind an `Arc` so every caller that joined the same in-flight computation gets that exact error instance.
- `ComputeAbandoned` — the caller computing this key went away before publishing a usable result: it panicked, its future was dropped, or what it published could not be read back.
- `Encode` — the value could not be encoded for storage.
- `Io` — the cache directory could not be created or resolved.
- `NotAdmitted` — the cache declined the write. Nothing is lost but a future hit.
- `Storage` — the database could not be opened, read, written or compacted.

Note what is **not** an error. A store that fails to read is a miss; an entry that cannot be decoded is a miss; and a cache write that fails does not stop `get_or_compute` returning the value it just computed. A broken cache degrades to no cache rather than breaking the call path — so only `get_bytes`, `set_bytes`, `cleanup` and the constructors surface `Storage`.

## Usage

### The basic shape

```rust
use std::time::Duration;
use rust_sak::memo::{CacheOpts, Memo};

let memo = Memo::memory(CacheOpts::new())?;

// "probing..." prints once: the second call is served from cache until the TTL runs out.
for _ in 0..2 {
    let cores: usize = memo.get_or_compute("cpu:cores", Duration::from_secs(300), || {
        println!("probing...");
        Ok::<_, std::convert::Infallible>(probe_cores())
    })?;

    assert_eq!(cores, 8);
}

fn probe_cores() -> usize { 8 }
# Ok::<(), Box<dyn std::error::Error>>(())
```

The type comes from the computation, so you get a real `Vec<Product>` back rather than an `Any` to downcast.

### The async variant

With the `memo-async` feature, `get_or_compute_async` takes a computation that is itself a future:

```rust
# use std::time::Duration;
# use rust_sak::memo::{CacheOpts, Memo};
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
# async fn featured_products() -> Result<Vec<String>, std::convert::Infallible> { Ok(vec![]) }
let memo = Memo::memory(CacheOpts::new())?;

let products: Vec<String> = memo
    .get_or_compute_async("products:featured", Duration::from_secs(3600), || async {
        featured_products().await
    })
    .await?;
# Ok(())
# }
```

Both methods share one in-flight table, so a sync and an async caller racing on the same key still compute it once. Two things are worth knowing:

- **`get_or_compute` blocks.** Calling it from inside an async task can park a runtime worker for as long as the computation takes. Use `get_or_compute_async` there, or `tokio::task::spawn_blocking`.
- **Cancellation is honoured.** Dropping the future — under `tokio::time::timeout`, say — releases every caller waiting on it with `ComputeAbandoned` rather than stranding them. A disk-backed cache reaches its store through `spawn_blocking`, so it needs a Tokio runtime; a memory-only one does not.

### Raw bytes

`get_bytes` and `set_bytes` drive the store directly, for callers that have already encoded their own values. They are transparent: bytes written come back exactly, with none of the framing `get_or_compute` adds.

```rust
# use std::time::Duration;
# use rust_sak::memo::{CacheOpts, Memo};
# fn run() -> Result<(), Box<dyn std::error::Error>> {
let memo = Memo::memory(CacheOpts::new())?;

memo.set_bytes("thumbnail:42", b"\x89PNG...", Duration::from_secs(600))?;
let cached: Option<Vec<u8>> = memo.get_bytes("thumbnail:42")?;
# Ok(())
# }
```

### TTL, expiry and disk space

Expired entries are never served: once the TTL passes, the entry reads as a miss and the computation runs again. Reclaiming the *space* on disk is a separate matter — a disk-backed cache sweeps once in the background when it opens, and `cleanup` forces one at a moment of your choosing:

```rust
# use rust_sak::memo::Memo;
# fn run(memo: &Memo) {
if let Err(err) = memo.cleanup() {
    eprintln!("cache cleanup failed: {err}");
}
# }
```

It is safe to call at any time, is a no-op on a memory-only cache, and a call made while another sweep is running returns immediately rather than queueing. Prefer a quiet moment: the sweep briefly excludes readers while it compacts.

## Things worth knowing

- **Values are encoded with [`postcard`](https://crates.io/crates/postcard),** so a cached type needs `Serialize` and `Deserialize`. Postcard is compact and fast but **not self-describing**, so each entry carries a small header with a fingerprint of the type it was written for. An entry whose fingerprint doesn't match is a miss rather than garbage — which also means two `get_or_compute` calls of different types can share a cache key safely.
- **The cache is not a data format.** Change a type's shape, move it between modules, or upgrade the compiler, and its entries stop matching and are recomputed. That is the right trade for a cache — a miss costs one recomputation — but don't treat the database file as storage. Raw `set_bytes` entries carry no header and are unaffected.
- **One schema change is not caught: reordering fields of the same type.** The fingerprint is derived from the type's *name*, so swapping two `u64` fields leaves it unchanged while changing what the bytes mean, and a persistent cache written before the swap would decode into the new order silently. Adding or removing a field is caught and recomputed; reordering is not. If you reorder same-typed fields in something you cache on disk, version the key — `key_from(["user", "v2", id])` — so the old entries are simply never asked for again.
- **Errors aren't cached.** If the computation fails, the error goes to every caller waiting on that key and nothing is written. The next call retries.
- **Cache writes are best-effort.** If the store fails to persist a result, `get_or_compute` still returns it.
- **A declined write isn't a failed one.** `NotAdmitted` means the cache dropped the write — a zero TTL, or a value larger than the whole memory budget — rather than that it broke. Nothing is lost but a future hit.
- **Disk deadlines are wall-clock.** They have to survive a restart, which a monotonic clock cannot. A clock jump can therefore expire an entry early or resurrect one briefly; the memory tier uses a monotonic clock and is immune.
- **`path` reports the canonical directory,** so it may differ from the string you passed in, and is `None` for a memory-only cache.
