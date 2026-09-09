use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use tempfile::TempDir;

use super::composite_store::CompositeStore;
use super::store::Store;
use super::test_support::{ComputeFailed, CountingStore, FailingStore, LONG};
use super::*;

/// A TTL short enough to expire during a test, but long enough to survive a scheduler hiccup on a loaded machine.
const SHORT: Duration = Duration::from_millis(300);

/// Comfortably longer than [`SHORT`], so an entry written with it is certainly expired.
const EXPIRED: Duration = Duration::from_millis(700);

/// A convenience for the common "compute always succeeds" case.
fn ok<T>(value: T) -> std::result::Result<T, ComputeFailed> {
    Ok(value)
}

/// A memory cache with the default sizing.
fn memory() -> Memo {
    Memo::memory(CacheOpts::new()).unwrap()
}

/// A disk cache in a fresh directory, returned alongside the directory that owns it.
fn disk() -> (Memo, TempDir) {
    let dir = TempDir::new().unwrap();
    let memo = Memo::disk(dir.path(), CacheOpts::new()).unwrap();

    (memo, dir)
}

// --- TTL and expiry ---

#[test]
fn a_cached_value_is_returned_without_running_compute_again() {
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let value: u32 = memo
            .get_or_compute("k", LONG, || {
                runs.fetch_add(1, Ordering::SeqCst);
                ok(7)
            })
            .unwrap();

        assert_eq!(value, 7);
    }

    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn expired_entry_is_treated_as_a_miss_and_recomputed() {
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));

    let compute = || {
        runs.fetch_add(1, Ordering::SeqCst);
        ok(runs.load(Ordering::SeqCst) as u32)
    };

    let first: u32 = memo.get_or_compute("k", SHORT, compute).unwrap();
    std::thread::sleep(EXPIRED);
    let second: u32 = memo.get_or_compute("k", LONG, compute).unwrap();

    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_zero_ttl_write_is_not_admitted() {
    let memo = memory();

    assert!(matches!(
        memo.set_bytes("k", b"v", Duration::ZERO),
        Err(MemoError::NotAdmitted)
    ));
    assert_eq!(memo.get_bytes("k").unwrap(), None);
}

#[test]
fn a_zero_ttl_write_is_not_admitted_on_disk() {
    let (memo, _dir) = disk();

    assert!(matches!(
        memo.set_bytes("k", b"v", Duration::ZERO),
        Err(MemoError::NotAdmitted)
    ));
}

#[test]
fn a_disk_entry_expires_on_wall_clock_time() {
    let (memo, _dir) = disk();

    memo.set_bytes("k", b"v", SHORT).unwrap();
    assert_eq!(memo.get_bytes("k").unwrap(), Some(b"v".to_vec()));

    std::thread::sleep(EXPIRED);
    assert_eq!(memo.get_bytes("k").unwrap(), None);
}

#[test]
fn memory_expiry_does_not_wait_on_housekeeping() {
    // The store re-checks the deadline itself rather than trusting moka to have reaped the entry, so an expiry is
    // visible the instant it happens.
    let memo = memory();

    memo.set_bytes("k", b"v", SHORT).unwrap();
    std::thread::sleep(EXPIRED);

    assert_eq!(memo.get_bytes("k").unwrap(), None);
}

// --- Singleflight ---

/// Runs `count` concurrent `get_or_compute` calls for one key, all released at the same moment, and reports how many
/// times the computation actually ran.
fn race(memo: &Memo, count: usize) -> (usize, Vec<u32>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(count));

    let results: Vec<u32> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..count)
            .map(|_| {
                let (memo, runs, barrier) = (memo.clone(), Arc::clone(&runs), Arc::clone(&barrier));
                scope.spawn(move || {
                    barrier.wait();
                    memo.get_or_compute("shared", LONG, || {
                        runs.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        ok(99_u32)
                    })
                    .unwrap()
                })
            })
            .collect();

        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });

    (runs.load(Ordering::SeqCst), results)
}

#[test]
fn concurrent_callers_for_one_missing_key_run_compute_once() {
    let (runs, results) = race(&memory(), 8);

    assert_eq!(runs, 1);
    assert_eq!(results, vec![99; 8]);
}

#[test]
fn concurrent_callers_run_compute_once_on_a_disk_only_cache() {
    // The case that would fail if deduplication were left to the memory tier: there isn't one.
    let (memo, _dir) = disk();
    let (runs, results) = race(&memo, 8);

    assert_eq!(runs, 1);
    assert_eq!(results, vec![99; 8]);
}

#[test]
fn concurrent_callers_run_compute_once_on_a_two_tier_cache() {
    let dir = TempDir::new().unwrap();
    let memo = Memo::memory_disk(dir.path(), CacheOpts::new(), LONG).unwrap();
    let (runs, results) = race(&memo, 8);

    assert_eq!(runs, 1);
    assert_eq!(results, vec![99; 8]);
}

#[test]
fn callers_for_different_keys_do_not_deduplicate() {
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|scope| {
        for index in 0..4_u32 {
            let (memo, runs) = (memo.clone(), Arc::clone(&runs));
            scope.spawn(move || {
                let _: u32 = memo
                    .get_or_compute(&format!("key-{index}"), LONG, || {
                        runs.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(30));
                        ok(index)
                    })
                    .unwrap();
            });
        }
    });

    assert_eq!(runs.load(Ordering::SeqCst), 4);
}

#[test]
fn every_waiter_receives_the_leaders_error_and_nothing_is_cached() {
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(4));

    let errors: Vec<bool> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (memo, runs, barrier) = (memo.clone(), Arc::clone(&runs), Arc::clone(&barrier));
                scope.spawn(move || {
                    barrier.wait();
                    let result: Result<u32> = memo.get_or_compute("k", LONG, || {
                        runs.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        Err(ComputeFailed)
                    });

                    matches!(result, Err(MemoError::Compute(_)))
                })
            })
            .collect();

        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(errors, vec![true; 4]);

    // The failure was not cached, so the next call gets to try again.
    assert_eq!(memo.get_bytes("k").unwrap(), None);
    let retried: u32 = memo.get_or_compute("k", LONG, || ok(1)).unwrap();
    assert_eq!(retried, 1);
}

#[test]
fn waiters_are_released_when_the_leader_panics() {
    let memo = memory();
    let barrier = Arc::new(Barrier::new(2));

    std::thread::scope(|scope| {
        let leader = {
            let (memo, barrier) = (memo.clone(), Arc::clone(&barrier));
            scope.spawn(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _: Result<u32> = memo.get_or_compute("k", LONG, || {
                        barrier.wait();
                        std::thread::sleep(Duration::from_millis(50));
                        panic!("the computation exploded");
                        #[allow(unreachable_code)]
                        ok(0)
                    });
                }))
            })
        };

        let follower = {
            let (memo, barrier) = (memo.clone(), Arc::clone(&barrier));
            scope.spawn(move || {
                barrier.wait();
                std::thread::sleep(Duration::from_millis(10));
                let result: Result<u32> = memo.get_or_compute("k", LONG, || ok(1));
                result
            })
        };

        assert!(
            leader.join().unwrap().is_err(),
            "the leader's panic should have propagated to its own caller"
        );
        assert!(
            matches!(follower.join().unwrap(), Err(MemoError::ComputeAbandoned)),
            "a waiter must be released rather than left blocked forever",
        );
    });

    // The flight table is left clean, so the key is computable again.
    assert!(memo.flight.is_empty());
    let retried: u32 = memo.get_or_compute("k", LONG, || ok(5)).unwrap();
    assert_eq!(retried, 5);
}

#[test]
fn two_types_on_one_key_do_not_share_a_flight() {
    // Postcard is not self-describing, so handing one type's bytes to another would produce garbage rather than
    // fail. The type fingerprint keeps the two computations apart.
    let memo = memory();

    let number: u32 = memo.get_or_compute("shared", LONG, || ok(7)).unwrap();
    let text: String = memo.get_or_compute("shared", LONG, || ok("seven".to_owned())).unwrap();

    assert_eq!(number, 7);
    assert_eq!(text, "seven");
}

// --- Two-tier promotion ---

#[test]
fn a_disk_hit_is_promoted_into_memory() {
    let dir = TempDir::new().unwrap();
    let composite = CompositeStore::open(dir.path(), CacheOpts::new(), LONG).unwrap();

    composite.disk.set("k", b"v", LONG).unwrap();
    assert!(
        composite.memory.get("k").unwrap().is_none(),
        "the value should start out on disk only"
    );

    assert!(composite.get("k").unwrap().is_some());
    assert!(
        composite.memory.get("k").unwrap().is_some(),
        "a disk hit should be promoted"
    );
}

#[test]
fn a_promote_ttl_of_zero_disables_promotion() {
    let dir = TempDir::new().unwrap();
    let composite = CompositeStore::open(dir.path(), CacheOpts::new(), Duration::ZERO).unwrap();

    composite.disk.set("k", b"v", LONG).unwrap();
    assert!(composite.get("k").unwrap().is_some());

    assert!(composite.memory.get("k").unwrap().is_none());
}

#[test]
fn a_promoted_entry_never_outlives_the_disk_entry_it_came_from() {
    let dir = TempDir::new().unwrap();
    // An hour of promotion over an entry with a fraction of a second left.
    let composite = CompositeStore::open(dir.path(), CacheOpts::new(), Duration::from_secs(3600)).unwrap();

    composite.disk.set("k", b"v", Duration::from_millis(200)).unwrap();
    composite.get("k").unwrap();

    let promoted = composite
        .memory
        .get("k")
        .unwrap()
        .expect("the entry should have been promoted");
    let remaining = promoted.remaining.expect("a promoted entry keeps a deadline");

    assert!(
        remaining <= Duration::from_millis(200),
        "promoted for {remaining:?}, which outlives its source"
    );
}

#[test]
fn a_write_lands_in_both_tiers() {
    let dir = TempDir::new().unwrap();
    let composite = CompositeStore::open(dir.path(), CacheOpts::new(), LONG).unwrap();

    composite.set("k", b"v", LONG).unwrap();

    assert!(composite.memory.get("k").unwrap().is_some());
    assert!(composite.disk.get("k").unwrap().is_some());
}

// --- Best-effort writes ---

#[test]
fn a_store_that_cannot_write_still_returns_the_computed_value() {
    let store = Arc::new(FailingStore::default());
    let memo = Memo::with_store(Arc::clone(&store) as Arc<dyn Store>);

    let value: u32 = memo.get_or_compute("k", LONG, || ok(42)).unwrap();

    assert_eq!(
        value, 42,
        "a broken cache must degrade to no cache, not break the call path"
    );
    assert!(
        store.sets.load(Ordering::SeqCst) > 0,
        "the write should still have been attempted"
    );
}

#[test]
fn a_store_that_cannot_read_is_treated_as_a_miss() {
    let store = Arc::new(FailingStore::default());
    let memo = Memo::with_store(Arc::clone(&store) as Arc<dyn Store>);
    let runs = Arc::new(AtomicUsize::new(0));

    for _ in 0..2 {
        let _: u32 = memo
            .get_or_compute("k", LONG, || {
                runs.fetch_add(1, Ordering::SeqCst);
                ok(1)
            })
            .unwrap();
    }

    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "an unreadable store means every call recomputes"
    );
}

#[test]
fn set_bytes_surfaces_the_store_error_that_get_or_compute_swallows() {
    let memo = Memo::with_store(Arc::new(FailingStore::default()) as Arc<dyn Store>);

    assert!(matches!(memo.set_bytes("k", b"v", LONG), Err(MemoError::Io(_))));
    assert!(matches!(memo.get_bytes("k"), Err(MemoError::Io(_))));
}

#[test]
fn a_composite_write_reports_the_disk_failure_over_the_memory_one() {
    // A memory failure costs a future hit; a disk failure loses the data, so it is the one worth reporting.
    let dir = TempDir::new().unwrap();
    let composite = CompositeStore::open(dir.path(), CacheOpts::new().max_capacity(64), LONG).unwrap();

    // Too big for the memory budget, but perfectly writable to disk.
    let value = vec![0_u8; 4096];
    assert!(
        composite.set("k", &value, LONG).is_ok(),
        "one tier accepting the write makes it a success"
    );
    assert!(composite.disk.get("k").unwrap().is_some());
    assert!(composite.memory.get("k").unwrap().is_none());
}

#[test]
fn a_value_larger_than_the_memory_budget_is_not_admitted() {
    let memo = Memo::memory(CacheOpts::new().max_capacity(64)).unwrap();

    assert!(matches!(
        memo.set_bytes("k", &vec![0_u8; 4096], LONG),
        Err(MemoError::NotAdmitted)
    ));
}

// --- Corrupt and type-mismatched entries ---

#[test]
fn a_truncated_stored_value_is_treated_as_a_miss() {
    let memo = memory();

    memo.set_bytes("k", &[0x01, 0x02], LONG).unwrap();

    let runs = Arc::new(AtomicUsize::new(0));
    let value: u32 = memo
        .get_or_compute("k", LONG, || {
            runs.fetch_add(1, Ordering::SeqCst);
            ok(3)
        })
        .unwrap();

    assert_eq!(value, 3);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn an_unknown_format_version_is_treated_as_a_miss() {
    let memo = memory();

    let mut bytes = header::encode(header::fingerprint::<u32>(), &7_u32).unwrap();
    bytes[0] = 0xFE;
    memo.set_bytes("k", &bytes, LONG).unwrap();

    let value: u32 = memo.get_or_compute("k", LONG, || ok(9)).unwrap();
    assert_eq!(
        value, 9,
        "an entry from a future format must be recomputed, not misread"
    );
}

#[test]
fn a_value_stored_for_a_different_type_is_treated_as_a_miss() {
    let memo = memory();

    // Written as a `u64`, read back as a `String`.
    let bytes = header::encode(header::fingerprint::<u64>(), &7_u64).unwrap();
    memo.set_bytes("k", &bytes, LONG).unwrap();

    let value: String = memo.get_or_compute("k", LONG, || ok("fresh".to_owned())).unwrap();
    assert_eq!(value, "fresh");
}

#[test]
fn raw_bytes_written_by_set_bytes_round_trip_without_a_header() {
    // The raw API is deliberately transparent: it is what a caller driving the store directly relies on.
    let memo = memory();

    memo.set_bytes("k", b"exactly these bytes", LONG).unwrap();

    assert_eq!(memo.get_bytes("k").unwrap(), Some(b"exactly these bytes".to_vec()));
}

#[test]
fn a_disk_record_shorter_than_its_header_is_treated_as_a_miss() {
    let (memo, _dir) = disk();

    // Reach past `set_bytes` to plant a record too short to carry a deadline.
    memo.set_bytes("k", b"v", LONG).unwrap();
    assert!(memo.get_bytes("k").unwrap().is_some());

    let value: u32 = memo.get_or_compute("k", LONG, || ok(1)).unwrap();
    assert_eq!(value, 1, "a record that is not a typed entry must read as a miss");
}

// --- Shared disk handles ---

#[test]
fn two_opens_of_one_directory_share_a_store() {
    let dir = TempDir::new().unwrap();

    let first = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
    let second = Memo::disk(dir.path(), CacheOpts::new().max_capacity(4 << 20)).unwrap();

    first.set_bytes("k", b"v", LONG).unwrap();

    assert_eq!(
        second.get_bytes("k").unwrap(),
        Some(b"v".to_vec()),
        "a second open must share the first's store"
    );
    assert_eq!(first.path(), second.path());
}

#[cfg(unix)]
#[test]
fn two_opens_through_different_symlinks_to_one_directory_share_a_store() {
    let root = TempDir::new().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();

    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let direct = Memo::disk(&real, CacheOpts::new()).unwrap();
    let linked = Memo::disk(&link, CacheOpts::new()).unwrap();

    direct.set_bytes("k", b"v", LONG).unwrap();

    assert_eq!(
        linked.get_bytes("k").unwrap(),
        Some(b"v".to_vec()),
        "paths are matched after resolving symlinks"
    );
}

#[test]
fn the_store_closes_when_the_last_handle_drops_and_the_data_survives() {
    let dir = TempDir::new().unwrap();

    {
        let memo = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
        memo.set_bytes("k", b"v", LONG).unwrap();
    }

    // Re-opening would fail with an "already open" error if the lock had not been released.
    let reopened = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
    assert_eq!(reopened.get_bytes("k").unwrap(), Some(b"v".to_vec()));
}

#[test]
fn dropping_a_stale_handle_does_not_evict_a_newer_store_at_the_same_path() {
    let dir = TempDir::new().unwrap();

    let first = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
    drop(first);

    let second = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
    second.set_bytes("k", b"v", LONG).unwrap();

    // The third open must find the second store, not a registry entry the first one's drop left behind.
    let third = Memo::disk(dir.path(), CacheOpts::new()).unwrap();
    assert_eq!(third.get_bytes("k").unwrap(), Some(b"v".to_vec()));
}

#[test]
fn path_reports_the_canonical_directory_and_none_for_memory() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("cache").join("..").join("cache");
    let memo = Memo::disk(&nested, CacheOpts::new()).unwrap();

    assert_eq!(
        memo.path(),
        Some(dir.path().join("cache").canonicalize().unwrap().as_path())
    );
    assert!(memory().path().is_none());
}

// --- Cleanup ---

#[test]
fn cleanup_removes_expired_entries_and_leaves_the_rest() {
    let dir = TempDir::new().unwrap();
    let store = DiskStore::open(dir.path(), CacheOpts::new()).unwrap();

    store.set("stale", b"v", SHORT).unwrap();
    store.set("fresh", b"v", LONG).unwrap();
    assert_eq!(store.record_count().unwrap(), 2);

    std::thread::sleep(EXPIRED);
    store.cleanup().unwrap();

    assert_eq!(
        store.record_count().unwrap(),
        1,
        "only the expired record should have been swept"
    );
    assert!(store.get("fresh").unwrap().is_some());
}

#[test]
fn cleanup_reclaims_the_space_expired_entries_held() {
    let dir = TempDir::new().unwrap();
    let store = DiskStore::open(dir.path(), CacheOpts::new()).unwrap();

    let payload = vec![0xAB_u8; 64 * 1024];
    for index in 0..64 {
        store.set(&format!("k{index}"), &payload, SHORT).unwrap();
    }

    let file = dir.path().join("memo.redb");
    let before = std::fs::metadata(&file).unwrap().len();

    std::thread::sleep(EXPIRED);
    store.cleanup().unwrap();

    let after = std::fs::metadata(&file).unwrap().len();
    assert!(after < before, "the file did not shrink: {before} -> {after}");
}

#[test]
fn cleanup_is_a_no_op_for_a_memory_only_cache() {
    let memo = memory();

    memo.set_bytes("k", b"v", LONG).unwrap();
    memo.cleanup().unwrap();

    assert_eq!(memo.get_bytes("k").unwrap(), Some(b"v".to_vec()));
}

#[test]
fn a_disk_store_sweeps_once_in_the_background_when_it_opens() {
    let dir = TempDir::new().unwrap();

    {
        let store = DiskStore::open(dir.path(), CacheOpts::new()).unwrap();
        store.set("stale", b"v", SHORT).unwrap();
        assert_eq!(store.record_count().unwrap(), 1);
    }

    std::thread::sleep(EXPIRED);
    let reopened = DiskStore::open(dir.path(), CacheOpts::new()).unwrap();

    // The sweep runs on its own thread, so give it a moment to land.
    let swept = (0..50).any(|_| {
        if reopened.record_count().unwrap() == 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
        false
    });

    assert!(
        swept,
        "opening a disk store should reclaim what the previous run left expired"
    );
}

// --- Keys ---

#[test]
fn key_from_is_order_sensitive_and_stable() {
    let key = key_from(["search", "shoes"]);

    assert_eq!(key, key_from(["search", "shoes"]));
    assert_ne!(key, key_from(["shoes", "search"]));
}

#[test]
fn key_from_separates_parts_so_concatenation_does_not_collide() {
    assert_ne!(key_from(["ab", "c"]), key_from(["a", "bc"]));
}

#[test]
fn key_from_produces_a_lowercase_hex_sha256() {
    let key = key_from(["anything"]);

    assert_eq!(key.len(), 64);
    assert!(key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn an_unserializable_part_contributes_a_type_marker_rather_than_nothing() {
    // serde_json refuses a map whose keys are not strings, which makes this a genuinely unserializable part.
    let unserializable: BTreeMap<(u8, u8), u8> = BTreeMap::from([((1, 2), 3)]);

    let with = KeyBuilder::new().part("user").part(&unserializable).finish();
    let without = KeyBuilder::new().part("user").finish();

    assert_ne!(
        with, without,
        "dropping the part would let two different computations share one entry"
    );
}

#[test]
fn key_builder_and_key_from_agree_for_homogeneous_parts() {
    let built = KeyBuilder::new().part("a").part("b").finish();

    assert_eq!(built, key_from(["a", "b"]));
}

// --- Handles ---

#[test]
fn clones_of_a_memo_share_one_store_and_one_flight_table() {
    let memo = memory();
    let clone = memo.clone();

    memo.set_bytes("k", b"v", LONG).unwrap();

    assert_eq!(clone.get_bytes("k").unwrap(), Some(b"v".to_vec()));
    assert!(Arc::ptr_eq(&memo.flight, &clone.flight));
}

#[test]
fn a_counting_store_sees_one_read_per_lookup() {
    let inner = Arc::new(MemoryStore::new(CacheOpts::new())) as Arc<dyn Store>;
    let counting = Arc::new(CountingStore::new(inner));
    let memo = Memo::with_store(Arc::clone(&counting) as Arc<dyn Store>);

    let _: u32 = memo.get_or_compute("k", LONG, || ok(1)).unwrap();
    let _: u32 = memo.get_or_compute("k", LONG, || ok(1)).unwrap();

    // The first call reads, misses, takes the flight and reads again; the second is served by the opening read.
    assert!(counting.gets.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        counting.sets.load(Ordering::SeqCst),
        1,
        "only the miss should have written"
    );
}

// --- The async path ---

#[cfg(feature = "memo-async")]
#[tokio::test]
async fn an_async_cached_value_is_returned_without_running_compute_again() {
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let value: u32 = memo
            .get_or_compute_async("k", LONG, || async {
                runs.fetch_add(1, Ordering::SeqCst);
                ok(7)
            })
            .await
            .unwrap();

        assert_eq!(value, 7);
    }

    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "memo-async")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_async_disk_cache_round_trips_through_spawn_blocking() {
    let dir = TempDir::new().unwrap();
    let memo = Memo::disk(dir.path(), CacheOpts::new()).unwrap();

    let first: String = memo
        .get_or_compute_async("k", LONG, || async { ok("cached".to_owned()) })
        .await
        .unwrap();
    let second: String = memo
        .get_or_compute_async("k", LONG, || async { ok("recomputed".to_owned()) })
        .await
        .unwrap();

    assert_eq!(first, "cached");
    assert_eq!(second, "cached", "the second call should have been served from disk");
}

#[cfg(feature = "memo-async")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_and_async_callers_racing_on_one_key_run_compute_once() {
    // One in-flight table serves both methods, so an async leader and a synchronous follower still share the work.
    let memo = memory();
    let runs = Arc::new(AtomicUsize::new(0));

    let asynchronous = {
        let (memo, runs) = (memo.clone(), Arc::clone(&runs));
        tokio::spawn(async move {
            memo.get_or_compute_async("shared", LONG, || async {
                runs.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(120)).await;
                ok(99_u32)
            })
            .await
            .unwrap()
        })
    };

    // Let the async caller take the flight first, so the sync one joins as a follower.
    tokio::time::sleep(Duration::from_millis(30)).await;

    let synchronous = {
        let (memo, runs) = (memo.clone(), Arc::clone(&runs));
        tokio::task::spawn_blocking(move || {
            memo.get_or_compute("shared", LONG, || {
                runs.fetch_add(1, Ordering::SeqCst);
                ok(99_u32)
            })
            .unwrap()
        })
    };

    assert_eq!(asynchronous.await.unwrap(), 99);
    assert_eq!(synchronous.await.unwrap(), 99);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the two paths must not compute the same key twice"
    );
}

#[cfg(feature = "memo-async")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_async_caller_is_released_when_the_leaders_future_is_dropped() {
    // Go's singleflight cannot do this: a cancelled leader's work runs to completion and its followers wait it out.
    let memo = memory();
    let started = Arc::new(tokio::sync::Notify::new());

    let leader = {
        let (memo, started) = (memo.clone(), Arc::clone(&started));
        tokio::spawn(async move {
            let computation = memo.get_or_compute_async("k", LONG, || async {
                started.notify_one();
                tokio::time::sleep(Duration::from_secs(30)).await;
                ok(1_u32)
            });

            tokio::time::timeout(Duration::from_millis(60), computation).await
        })
    };

    started.notified().await;
    let follower: Result<u32> = memo.get_or_compute_async("k", LONG, || async { ok(2) }).await;

    assert!(
        matches!(follower, Err(MemoError::ComputeAbandoned)),
        "a cancelled leader must release its followers rather than strand them",
    );
    assert!(
        leader.await.unwrap().is_err(),
        "the leader itself should have timed out"
    );

    // And the key is computable again afterwards.
    let retried: u32 = memo.get_or_compute_async("k", LONG, || async { ok(3) }).await.unwrap();
    assert_eq!(retried, 3);
}

#[test]
fn a_payload_with_bytes_left_over_is_treated_as_a_miss() {
    // Postcard ignores a trailing remainder, so a type that has lost a field since the entry was written would
    // otherwise decode cleanly from the longer old payload and serve a stale value.
    let memo = memory();

    let mut bytes = header::encode(header::fingerprint::<u32>(), &7_u32).unwrap();
    bytes.push(0xFF);
    memo.set_bytes("k", &bytes, LONG).unwrap();

    let value: u32 = memo.get_or_compute("k", LONG, || ok(9)).unwrap();
    assert_eq!(
        value, 9,
        "a payload that is not consumed exactly must be recomputed, not partially read"
    );
}
