use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use redb::ReadableTableMetadata;
use redb::{Database, Durability, ReadableDatabase, TableDefinition, TableError};

use super::store::{Entry, Store};
use super::{CacheOpts, MemoError, Result};

/// The table holding the cache.
///
/// Versioned in the name so that a change to the record layout below starts a fresh table rather than misreading the
/// old one.
const ENTRIES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("memo_entries_v1");

/// The file the database lives in when the caller names only a directory.
const DATABASE_FILE: &str = "memo.redb";

/// The deadline prefix on every record: milliseconds since the Unix epoch, big-endian, `0` meaning no deadline.
const DEADLINE_LEN: usize = 8;

/// How long to keep retrying an open that reports the directory is already locked.
///
/// A store torn down moments ago releases its lock as its `Drop` runs, so a re-open arriving in that window is racing
/// a teardown rather than another process. These bound how long that race is waited out before the error is believed.
const OPEN_ATTEMPTS: u32 = 25;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(2);

/// redb's read cache is sized in bytes; these are the bounds a [`CacheOpts::max_capacity`] is clamped to.
const MIN_CACHE_SIZE: u64 = 1 << 20;
const MAX_CACHE_SIZE: u64 = 1 << 30;

/// The disk stores this process currently has open, keyed by the database file: its canonicalized directory joined
/// with its file name.
///
/// redb takes an exclusive file lock, so a second open of the same path fails with
/// `DatabaseError::DatabaseAlreadyOpen` — and that error reads exactly like a *different* process holding the lock.
/// Handing back the store this process already has open removes the question entirely, which is what the Go package
/// needs a separate `NewDiskShared` constructor for.
static OPEN: LazyLock<Mutex<HashMap<PathBuf, Registered>>> = LazyLock::new(Mutex::default);

/// One registry entry: the store's [`GENERATION`] stamp, and a handle that does not keep it alive.
type Registered = (u64, Weak<DiskStore>);

/// Stamps each store so a late [`Drop`] cannot evict a newer store that has since taken the same path.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A cache backed by a redb database file.
#[derive(Debug)]
pub(super) struct DiskStore {
    /// The database.
    ///
    /// Behind an [`RwLock`] because `Database::compact` needs `&mut`, while reads and writes need only a shared
    /// borrow — so a sweep can do its deletion pass without excluding readers, and take the exclusive lock only for
    /// the compaction itself. Behind an [`Option`] so [`Drop`] can close it while still holding [`OPEN`], which is
    /// what stops a re-open from racing the file lock this store has not released yet.
    db: RwLock<Option<Database>>,
    /// The canonicalized directory holding the database, for [`Store::path`].
    directory: PathBuf,
    /// The database file inside [`directory`](Self::directory): this store's key in [`OPEN`].
    file: PathBuf,
    /// This store's [`GENERATION`] stamp.
    generation: u64,
    /// Held for the length of a sweep, so only one runs at a time.
    sweeping: AtomicBool,
    /// Which writes are made durable, and how many are waiting to be.
    cadence: Cadence,
}

/// Decides which writes commit durably, from [`CacheOpts::flush_every`] and [`CacheOpts::flush_interval`].
///
/// Every write commits, so it is readable at once, but only a durable commit forces it to disk — and redb's durable
/// commit persists every non-durable one before it. The fields are only read and updated with the database's write
/// transaction held, which redb grants to one caller at a time, so exactly one write per window is the durable one.
/// They are atomics only because the store is shared across threads.
#[derive(Debug)]
struct Cadence {
    /// Make a write durable once this many have been committed since the last durable one. Never zero.
    every: u32,
    /// Also make a write durable once this long has passed since the last durable one. Zero means never.
    interval: Duration,
    /// The writes committed since the last durable commit.
    pending: AtomicU32,
    /// When the last durable commit landed, in nanoseconds since [`epoch`](Self::epoch).
    last_durable: AtomicU64,
    /// The reference point for [`last_durable`](Self::last_durable): monotonic, so a clock jump cannot fire a flush.
    epoch: Instant,
    /// How many writes have committed durably. Test-only.
    #[cfg(test)]
    durable_writes: AtomicU64,
}

impl Cadence {
    fn new(opts: CacheOpts) -> Self {
        Cadence {
            every: opts.flush_every.max(1),
            interval: opts.flush_interval,
            pending: AtomicU32::new(0),
            last_durable: AtomicU64::new(0),
            epoch: Instant::now(),
            #[cfg(test)]
            durable_writes: AtomicU64::new(0),
        }
    }

    /// Whether the write about to commit must be durable. Call it holding the write transaction.
    fn is_due(&self) -> bool {
        if self.pending.load(Ordering::Acquire).saturating_add(1) >= self.every {
            return true;
        }

        !self.interval.is_zero() && self.elapsed().saturating_sub(self.last_durable()) >= self.interval
    }

    /// Whether any committed write is still waiting to be made durable.
    fn has_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }

    /// Records a non-durable commit.
    fn deferred(&self) {
        self.pending.fetch_add(1, Ordering::AcqRel);
    }

    /// Records a durable commit, which has persisted every write before it.
    fn durable(&self) {
        let now = u64::try_from(self.elapsed().as_nanos()).unwrap_or(u64::MAX);

        self.last_durable.store(now, Ordering::Release);
        self.pending.store(0, Ordering::Release);
    }

    fn elapsed(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn last_durable(&self) -> Duration {
        Duration::from_nanos(self.last_durable.load(Ordering::Acquire))
    }
}

impl DiskStore {
    /// Opens the store in `directory`, in the default database file there. See [`DiskStore::open_file`].
    pub(super) fn open(directory: impl AsRef<Path>, opts: CacheOpts) -> Result<Arc<DiskStore>> {
        DiskStore::open_file(directory.as_ref().join(DATABASE_FILE), opts)
    }

    /// Opens the store in the database `file`, creating it and its directory if needed, or returns the one this
    /// process already has open there.
    ///
    /// `opts` is honoured only by whichever call opens the store; a caller handed an existing one gets its sizing and
    /// its cadence.
    pub(super) fn open_file(file: impl AsRef<Path>, opts: CacheOpts) -> Result<Arc<DiskStore>> {
        let file = file.as_ref();
        let name = file.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the database path does not name a file",
            )
        })?;

        // The file itself may not exist yet, so it is the directory that is canonicalized - and it is created first,
        // because `canonicalize` requires the path to exist. A bare file name lives in the current directory.
        let parent = file.parent().filter(|parent| !parent.as_os_str().is_empty());
        let parent = parent.unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let directory = parent.canonicalize()?;
        let file = directory.join(name);

        let cache_size = opts.max_capacity.clamp(MIN_CACHE_SIZE, MAX_CACHE_SIZE);
        let mut builder = Database::builder();
        builder.set_cache_size(usize::try_from(cache_size).unwrap_or(usize::MAX));

        for attempt in 0..OPEN_ATTEMPTS {
            if let Some(store) = DiskStore::registered(&file) {
                return Ok(store);
            }

            // Deliberately *not* holding OPEN here. A store whose last handle has just gone still holds the file
            // lock until its `Drop` reaches OPEN, and that `Drop` cannot make progress while we hold it.
            match builder.create(&file) {
                Ok(db) => return Ok(DiskStore::register(directory, file, db, opts)),
                // Either a teardown is still in flight, in which case waiting resolves it, or another process holds
                // the file, in which case the attempts run out and the caller gets a truthful error.
                Err(redb::DatabaseError::DatabaseAlreadyOpen) if attempt + 1 < OPEN_ATTEMPTS => {
                    std::thread::sleep(OPEN_RETRY_DELAY);
                }
                Err(err) => return Err(err.into()),
            }
        }

        Err(MemoError::Storage(redb::DatabaseError::DatabaseAlreadyOpen.into()))
    }

    /// The live store already open on `file`, if there is one.
    ///
    /// An entry whose store is mid-teardown is dropped on the way past: its `Weak` can no longer be upgraded, and
    /// leaving it would make every later caller consult a corpse.
    fn registered(file: &Path) -> Option<Arc<DiskStore>> {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);

        match open.get(file) {
            Some((_, weak)) => match weak.upgrade() {
                Some(store) => Some(store),
                None => {
                    open.remove(file);
                    None
                }
            },
            None => None,
        }
    }

    /// Registers `db` as the store for `file`, or discards it if another caller got there first.
    fn register(directory: PathBuf, file: PathBuf, db: Database, opts: CacheOpts) -> Arc<DiskStore> {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);

        // Someone may have finished opening this path while we were opening it too.
        if let Some((_, weak)) = open.get(&file)
            && let Some(store) = weak.upgrade()
        {
            return store;
        }

        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let store = Arc::new(DiskStore {
            db: RwLock::new(Some(db)),
            directory,
            file: file.clone(),
            generation,
            sweeping: AtomicBool::new(false),
            cadence: Cadence::new(opts),
        });

        open.insert(file, (generation, Arc::downgrade(&store)));
        drop(open);

        // Reclaim whatever the previous run left behind, in the background so that opening stays cheap. It holds a
        // `Weak`: an `Arc` here would keep the store alive past its last real handle.
        let pending = Arc::downgrade(&store);
        std::thread::spawn(move || {
            if let Some(store) = pending.upgrade() {
                // Best-effort: a failed sweep must not be able to fail startup.
                let _ = store.cleanup();
            }
        });

        store
    }

    /// The database behind the guard.
    ///
    /// It is only ever `None` inside [`Drop`], where nothing can reach this.
    fn database(db: &Option<Database>) -> &Database {
        db.as_ref().expect("the database is only taken while dropping")
    }

    /// Splits a stored record into its deadline and its value, or `None` if it is too short to be one.
    fn split(record: &[u8]) -> Option<(Option<SystemTime>, &[u8])> {
        let (deadline, value) = record.split_at_checked(DEADLINE_LEN)?;
        let millis = u64::from_be_bytes(deadline.try_into().ok()?);
        let expires_at = (millis != 0).then(|| UNIX_EPOCH + Duration::from_millis(millis));

        Some((expires_at, value))
    }

    /// How much longer a record may be served: `Some(None)` for one with no deadline, `None` once it has expired.
    fn remaining(expires_at: Option<SystemTime>) -> Option<Option<Duration>> {
        match expires_at {
            None => Some(None),
            Some(at) => match at.duration_since(SystemTime::now()) {
                Ok(remaining) if !remaining.is_zero() => Some(Some(remaining)),
                _ => None,
            },
        }
    }
}

/// Clears [`DiskStore::sweeping`] when a sweep ends, however it ends.
struct SweepGuard<'a>(&'a AtomicBool);

impl Drop for SweepGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Store for DiskStore {
    fn get(&self, key: &str) -> Result<Option<Entry>> {
        let db = self.db.read().unwrap_or_else(PoisonError::into_inner);
        let txn = DiskStore::database(&db).begin_read()?;

        let table = match txn.open_table(ENTRIES) {
            Ok(table) => table,
            // Nothing has ever been written here, so every key is a miss.
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        let Some(record) = table.get(key)? else {
            return Ok(None);
        };

        // A record too short to carry a deadline is corrupt; report it as a miss and let a sweep remove it.
        let Some((expires_at, value)) = DiskStore::split(record.value()) else {
            return Ok(None);
        };

        let Some(remaining) = DiskStore::remaining(expires_at) else {
            return Ok(None);
        };

        Ok(Some(Entry {
            value: Arc::from(value),
            remaining,
        }))
    }

    fn set(&self, key: &str, value: Arc<[u8]>, ttl: Duration) -> Result<()> {
        // A zero TTL expires the instant it lands, so the write is provably lost; say so instead of doing the I/O.
        if ttl.is_zero() {
            return Err(MemoError::NotAdmitted);
        }

        // Wall-clock, not `Instant`, because the deadline has to survive a restart and a monotonic clock does not.
        // The cost is that a clock jump can resurrect or prematurely expire an entry, which is inherent to any
        // persisted TTL.
        let millis = SystemTime::now()
            .checked_add(ttl)
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX));

        let mut record = Vec::with_capacity(DEADLINE_LEN + value.len());
        record.extend_from_slice(&millis.to_be_bytes());
        record.extend_from_slice(&value);

        let db = self.db.read().unwrap_or_else(PoisonError::into_inner);
        let mut txn = DiskStore::database(&db).begin_write()?;
        {
            let mut table = txn.open_table(ENTRIES)?;
            table.insert(key, record.as_slice())?;
        }

        // Decided with the write transaction held, so no other write can slip into this window.
        let durable = self.cadence.is_due();
        if !durable {
            txn.set_durability(Durability::None)?;
        }
        txn.commit()?;

        if durable {
            self.cadence.durable();

            #[cfg(test)]
            self.cadence.durable_writes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cadence.deferred();
        }

        Ok(())
    }

    fn flush(&self) -> Result<()> {
        let db = self.db.read().unwrap_or_else(PoisonError::into_inner);
        DiskStore::flush_into(DiskStore::database(&db), &self.cadence)
    }

    fn cleanup(&self) -> Result<()> {
        // A sweep already in flight is doing this work; joining it would only make the caller wait for it.
        if self.sweeping.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // The flag is cleared through a guard rather than by the line after the call, so it survives `sweep`
        // unwinding. Clearing it only on the success path would let one panicking sweep leave the flag set for the
        // rest of the process, and every later `cleanup` would return `Ok(())` having reclaimed nothing - the same
        // "one panic must not disable the subsystem" rule the `PoisonError::into_inner` handling here follows.
        let _guard = SweepGuard(&self.sweeping);

        self.sweep()
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.directory)
    }
}

impl DiskStore {
    /// Makes every deferred write durable with one empty durable commit. Nothing to do if no write is deferred.
    ///
    /// Takes the database and the cadence apart rather than `&self`, so [`Drop`] can call it on the database it has
    /// exclusive access to.
    fn flush_into(db: &Database, cadence: &Cadence) -> Result<()> {
        let txn = db.begin_write()?;

        // Checked with the write transaction held, so a write committing concurrently is either counted here or
        // has not started.
        if !cadence.has_pending() {
            txn.abort()?;
            return Ok(());
        }

        txn.commit()?;
        cadence.durable();

        Ok(())
    }

    /// Deletes expired entries, then compacts so the space they held is actually returned to the filesystem.
    ///
    /// Compaction is skipped when the pass deleted nothing. It rewrites the whole database file and is the only
    /// part that needs to exclude readers, so paying for it to reclaim zero bytes would make every open of a
    /// healthy cache cost a full-file read and write — see [`DiskStore::register`], which sweeps on every open.
    fn sweep(&self) -> Result<()> {
        let removed;
        {
            let db = self.db.read().unwrap_or_else(PoisonError::into_inner);
            let txn = DiskStore::database(&db).begin_write()?;
            {
                let mut table = match txn.open_table(ENTRIES) {
                    Ok(table) => table,
                    // Nothing written yet, so nothing to reclaim.
                    Err(TableError::TableDoesNotExist(_)) => return Ok(()),
                    Err(err) => return Err(err.into()),
                };

                // A record too short to split is corrupt: drop it rather than leave it to be re-read forever.
                let mut deleted = 0_u64;
                table.retain(|_key, record| {
                    let keep = DiskStore::split(record)
                        .and_then(|(expires_at, _)| DiskStore::remaining(expires_at))
                        .is_some();

                    deleted += u64::from(!keep);
                    keep
                })?;

                removed = deleted;
            }
            txn.commit()?;
        }

        if removed == 0 {
            return Ok(());
        }

        // Compaction is where the space actually comes back, and it is the only part that needs to exclude readers.
        let mut db = self.db.write().unwrap_or_else(PoisonError::into_inner);
        db.as_mut()
            .expect("the database is only taken while dropping")
            .compact()?;

        Ok(())
    }
}

#[cfg(test)]
impl DiskStore {
    /// How many writes have committed durably. Test-only.
    pub(super) fn durable_writes(&self) -> u64 {
        self.cadence.durable_writes.load(Ordering::Relaxed)
    }

    /// Whether any committed write is still waiting to be made durable. Test-only.
    pub(super) fn has_deferred(&self) -> bool {
        self.cadence.has_pending()
    }

    /// How many records the table physically holds, expired ones included. Test-only.
    pub(super) fn record_count(&self) -> Result<u64> {
        let db = self.db.read().unwrap_or_else(PoisonError::into_inner);
        let txn = DiskStore::database(&db).begin_read()?;

        match txn.open_table(ENTRIES) {
            Ok(table) => Ok(table.len()?),
            Err(TableError::TableDoesNotExist(_)) => Ok(0),
            Err(err) => Err(err.into()),
        }
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        // Persist what the cadence deferred, before taking the registry so a slow fsync does not hold up other
        // opens. `Drop` cannot report a failure, and redb recovers to the last durable commit regardless.
        if let Some(db) = self.db.get_mut().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = DiskStore::flush_into(db, &self.cadence);
        }

        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);

        // Only evict our own entry. Without the generation check, a `Drop` running after a new store had already
        // registered at this path would evict the live one and leave it unreachable.
        if open
            .get(&self.file)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            open.remove(&self.file);
        }

        // Close the database *before* releasing the registry, so that a caller who takes the lock next and finds
        // nothing registered is guaranteed the file lock has already gone with it.
        drop(self.db.get_mut().unwrap_or_else(PoisonError::into_inner).take());
        drop(open);
    }
}
