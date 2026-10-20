//! Small, secret-free startup generation and launch-intent storage.
//!
//! One locked private directory is a server identity domain. Losing, deleting or
//! restoring that directory requires explicit recovery; it must not be silently
//! recreated as the same server. No operation bodies or launch arguments persist.

use crate::protocol::OperationKey;
use rustix::fs::{fcntl_fullfsync, flock, open, openat, renameat, FlockOperation, Mode, OFlags};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

const COUNTER_MAGIC: &[u8; 8] = b"PLGEN001";
const RECORD_MAGIC: &[u8; 8] = b"PLJNL001";
const RECORD_BYTES: usize = 56;
/// Maximum append-only launch journal size; exhaustion requires explicit repair.
pub const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
const MAX_JOURNAL_KEYS: usize = 4096;
const FILE_FLAGS: OFlags = OFlags::RDWR
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "server metadata is invalid")
}

fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "server storage is unsafe")
}

fn validate_file(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
    {
        return Err(denied());
    }
    Ok(())
}

fn private_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() || fs::canonicalize(path)? != path {
        return Err(denied());
    }
    let uid = rustix::process::geteuid().as_raw();
    let mut part = std::path::PathBuf::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(denied());
        }
        part.push(component.as_os_str());
        let m = fs::symlink_metadata(&part)?;
        if !m.is_dir()
            || (m.uid() != uid && m.uid() != 0)
            || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
        {
            return Err(denied());
        }
    }
    let directory = File::from(open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let m = directory.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if m.uid() != uid
        || m.mode() & 0o7777 != 0o700
        || (m.dev(), m.ino()) != (named.dev(), named.ino())
    {
        return Err(denied());
    }
    Ok(directory)
}

fn open_file(directory: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(directory, name, FILE_FLAGS, Mode::empty())?);
    validate_file(&file)?;
    Ok(file)
}

fn create_file(directory: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(
        directory,
        name,
        FILE_FLAGS | OFlags::CREATE | OFlags::EXCL,
        Mode::RUSR | Mode::WUSR,
    )?);
    validate_file(&file)?;
    Ok(file)
}

fn flush(file: &File) -> io::Result<()> {
    file.sync_all()?;
    fcntl_fullfsync(file)?;
    Ok(())
}

fn verify_named_lock(directory: &File, lock: &File) -> io::Result<()> {
    validate_file(lock)?;
    let metadata = lock.metadata()?;
    if metadata.len() != 24 {
        return Err(invalid());
    }
    let mut bytes = [0; 24];
    lock.read_exact_at(&mut bytes, 0)?;
    if &bytes[..8] != b"PLLOCK01"
        || u64::from_be_bytes(bytes[8..16].try_into().unwrap()) != metadata.dev()
        || u64::from_be_bytes(bytes[16..24].try_into().unwrap()) != metadata.ino()
    {
        return Err(invalid());
    }
    let named = open_file(directory, "server.lock")?;
    let a = lock.metadata()?;
    let b = named.metadata()?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(denied());
    }
    Ok(())
}

fn publish_generation(directory: &File, generation: u64) -> io::Result<()> {
    let mut next = create_file(directory, "generation.next")?;
    next.write_all(COUNTER_MAGIC)?;
    next.write_all(&generation.to_be_bytes())?;
    next.write_all(&(!generation).to_be_bytes())?;
    flush(&next)?;
    renameat(directory, "generation.next", directory, "generation")?;
    directory.sync_all()?;
    fcntl_fullfsync(&next)?;
    Ok(())
}

/// Holds the singleton lock for the entire server lifetime.
///
/// Startup increments and flushes the counter before returning a generation.
/// A leftover generation.next, malformed file or uncertain sync fails closed.
pub struct ServerStorage {
    directory: File,
    directory_path: PathBuf,
    _lock: File,
    generation: u64,
}

/// Held exclusive server identity lock for offline history maintenance.
/// Unlike `ServerStorage::open`, acquiring it does not advance the generation.
pub struct MaintenanceLock {
    _directory: File,
    _lock: File,
}

impl std::fmt::Debug for MaintenanceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MaintenanceLock { .. }")
    }
}

impl std::fmt::Debug for ServerStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerStorage { .. }")
    }
}

impl ServerStorage {
    /// Exclude both direct `serve` and zero-config startup while offline
    /// history is cleared. Missing, corrupt or unsafe metadata fails closed;
    /// this does not initialize storage or issue a new generation.
    pub fn lock_for_maintenance(path: &Path) -> io::Result<MaintenanceLock> {
        let directory = private_directory(path)?;
        let lock = open_file(&directory, "server.lock")?;
        drop(open_file(&directory, "generation")?);
        drop(open_file(&directory, "launch.journal")?);
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        verify_named_lock(&directory, &lock)?;
        Ok(MaintenanceLock {
            _directory: directory,
            _lock: lock,
        })
    }
    /// Explicitly initialize an empty private identity domain.
    ///
    /// A fresh OS-random seed avoids deterministic namespace reuse if an
    /// operator explicitly initializes a replacement directory. It is not an
    /// authentication secret or a collision-free identity proof across domains.
    /// Lost or restored metadata never triggers this operation automatically.
    pub fn initialize(path: &Path) -> io::Result<Self> {
        let mut seed = [0; 8];
        File::open("/dev/urandom")?.read_exact(&mut seed)?;
        let generation = u64::from_be_bytes(seed);
        if generation == 0 || generation == u64::MAX {
            return Err(invalid());
        }
        Self::initialize_at(path, generation)
    }

    #[cfg(test)]
    pub(crate) fn initialize_for_test(path: &Path) -> io::Result<Self> {
        Self::initialize_at(path, 1)
    }

    fn initialize_at(path: &Path, generation: u64) -> io::Result<Self> {
        let directory = private_directory(path)?;
        if fs::read_dir(path)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "server storage is not empty",
            ));
        }
        let mut lock = create_file(&directory, "server.lock")?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        let metadata = lock.metadata()?;
        lock.write_all(b"PLLOCK01")?;
        lock.write_all(&metadata.dev().to_be_bytes())?;
        lock.write_all(&metadata.ino().to_be_bytes())?;
        flush(&lock)?;
        let journal = create_file(&directory, "launch.journal")?;
        flush(&journal)?;
        publish_generation(&directory, generation)?;
        let storage = Self {
            directory,
            directory_path: path.to_path_buf(),
            _lock: lock,
            generation,
        };
        storage.verify_identity()?;
        Ok(storage)
    }

    /// Advance an established generation without recreating missing metadata.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = private_directory(path)?;
        let lock = open_file(&directory, "server.lock")?;
        // An initialization in progress must not lose its brand-new lock to a
        // second starter. Check required files exist before trying that lock.
        drop(open_file(&directory, "generation")?);
        let _journal = open_file(&directory, "launch.journal")?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        verify_named_lock(&directory, &lock)?;
        // Reopen after locking: a competing completed startup may have renamed
        // a newer counter while this process was waiting to acquire its lock.
        let mut counter = open_file(&directory, "generation")?;
        if counter.metadata()?.len() != 24 {
            return Err(invalid());
        }
        let mut bytes = [0; 24];
        counter.read_exact(&mut bytes)?;
        let previous = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
        let inverse = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
        if &bytes[..8] != COUNTER_MAGIC || previous == 0 || inverse != !previous {
            return Err(invalid());
        }
        let generation = previous.checked_add(1).ok_or_else(invalid)?;
        publish_generation(&directory, generation)?;
        let storage = Self {
            directory,
            directory_path: path.to_path_buf(),
            _lock: lock,
            generation,
        };
        storage.verify_identity()?;
        Ok(storage)
    }

    /// Check the protected directory and named lock still identify held inodes.
    pub fn verify_identity(&self) -> io::Result<()> {
        let named = private_directory(&self.directory_path)?;
        let a = named.metadata()?;
        let b = self.directory.metadata()?;
        if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
            return Err(denied());
        }
        verify_named_lock(&self.directory, &self._lock)
    }

    /// Nonzero, monotonically advanced generation for this storage domain.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Open the append-only journal while keeping the singleton lock borrowed.
    pub fn journal(&mut self) -> io::Result<LaunchJournal<'_>> {
        self.verify_identity()?;
        let file = open_file(&self.directory, "launch.journal")?;
        LaunchJournal::load(self, file)
    }
}

/// Secret-free lifecycle facts. Intent/Spawned recovered from disk are uncertain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LaunchState {
    /// A durable intent exists; no success may be inferred.
    Intent = 1,
    /// Spawn was observed, but result commitment is not yet known.
    Spawned = 2,
    /// Spawn failed before creating the managed child.
    Failed = 3,
    /// Runtime committed its managed child result.
    Applied = 4,
    /// An effect may have occurred; never automatically spawn a replacement.
    Unknown = 5,
}

/// Fixed metadata record; never includes argv, environment, terminal IO or PID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchRecord {
    /// Operation identity of the explicit launch.
    pub key: OperationKey,
    /// Managed session identity within that operation's generation.
    pub session: u64,
    /// Last persisted lifecycle fact.
    pub state: LaunchState,
}

/// Bounded journal borrowing the singleton owner so it cannot outlive its lock.
pub struct LaunchJournal<'a> {
    storage: &'a ServerStorage,
    file: File,
    records: BTreeMap<OperationKey, LaunchRecord>,
    bytes: u64,
    poisoned: bool,
}

impl LaunchJournal<'_> {
    fn load(storage: &ServerStorage, mut file: File) -> io::Result<LaunchJournal<'_>> {
        let bytes = file.metadata()?.len();
        if bytes > MAX_JOURNAL_BYTES || bytes % RECORD_BYTES as u64 != 0 {
            return Err(invalid());
        }
        let mut records = BTreeMap::<OperationKey, LaunchRecord>::new();
        let mut buffer = [0; RECORD_BYTES];
        for _ in 0..bytes / RECORD_BYTES as u64 {
            file.read_exact(&mut buffer)?;
            let record = decode_record(&buffer)?;
            if let Some(old) = records.get(&record.key) {
                if old.session != record.session || !transition(old.state, record.state) {
                    return Err(invalid());
                }
            } else if record.state != LaunchState::Intent || records.len() >= MAX_JOURNAL_KEYS {
                return Err(invalid());
            }
            records.insert(record.key, record);
        }
        Ok(LaunchJournal {
            storage,
            file,
            records,
            bytes,
            poisoned: false,
        })
    }

    /// Read recovered metadata. Incomplete persisted launches are Unknown.
    pub fn recovered(&self, key: OperationKey) -> Option<LaunchRecord> {
        self.records.get(&key).copied().map(|mut record| {
            if matches!(record.state, LaunchState::Intent | LaunchState::Spawned) {
                record.state = LaunchState::Unknown;
            }
            record
        })
    }

    /// Check a new launch's metadata capacity without admitting it or writing.
    ///
    /// Reserves nothing: the single owner must recheck immediately before its
    /// Intent write, without interleaving other journal mutations. Three full
    /// records cover Intent/Spawned/Applied; a failed spawn needs only two.
    /// This does not promise a later write or durability operation will succeed.
    pub fn preflight_launch(&self, key: OperationKey, session: u64) -> io::Result<()> {
        encode_record(LaunchRecord {
            key,
            session,
            state: LaunchState::Intent,
        })?;
        if self.poisoned
            || key.generation != self.storage.generation
            || self.records.contains_key(&key)
            || self
                .records
                .values()
                .any(|record| record.key.generation == key.generation && record.session == session)
        {
            return Err(invalid());
        }
        if self.records.len() >= MAX_JOURNAL_KEYS
            || self
                .bytes
                .checked_add(3 * RECORD_BYTES as u64)
                .is_none_or(|bytes| bytes > MAX_JOURNAL_BYTES)
        {
            return Err(io::Error::other("launch journal is full"));
        }
        Ok(())
    }

    /// Persist a new intent before spawn, or advance an existing lifecycle fact.
    ///
    /// Duplicate identical records do not append. A failure poisons this handle;
    /// no caller may use that failure to authorize spawn or replay a launch.
    pub fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        if self.poisoned || record.key.generation != self.storage.generation {
            return Err(invalid());
        }
        let encoded = encode_record(record)?;
        if let Some(old) = self.records.get(&record.key) {
            if *old == record {
                return Ok(());
            }
            if old.session != record.session || !transition(old.state, record.state) {
                return Err(invalid());
            }
        } else if record.state != LaunchState::Intent
            || self.records.len() >= MAX_JOURNAL_KEYS
            || self.records.values().any(|old| {
                old.key.generation == record.key.generation && old.session == record.session
            })
        {
            return Err(invalid());
        }
        if self.bytes + RECORD_BYTES as u64 > MAX_JOURNAL_BYTES {
            return Err(io::Error::other("launch journal is full"));
        }
        self.poisoned = true;
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&encoded)?;
        flush(&self.file)?;
        self.bytes += RECORD_BYTES as u64;
        self.records.insert(record.key, record);
        self.poisoned = false;
        Ok(())
    }
}

fn transition(old: LaunchState, next: LaunchState) -> bool {
    old == next
        || matches!(
            (old, next),
            (
                LaunchState::Intent,
                LaunchState::Spawned | LaunchState::Failed | LaunchState::Unknown
            ) | (
                LaunchState::Spawned,
                LaunchState::Applied | LaunchState::Unknown
            )
        )
}

fn encode_record(record: LaunchRecord) -> io::Result<[u8; RECORD_BYTES]> {
    if record.key.generation == 0
        || record.key.lease == 0
        || record.key.sequence == 0
        || record.session == 0
    {
        return Err(invalid());
    }
    let mut bytes = [0; RECORD_BYTES];
    bytes[..8].copy_from_slice(RECORD_MAGIC);
    for (i, value) in [
        record.key.generation,
        record.key.lease,
        record.key.sequence,
        record.session,
    ]
    .iter()
    .enumerate()
    {
        bytes[8 + i * 8..16 + i * 8].copy_from_slice(&value.to_be_bytes());
    }
    bytes[40] = record.state as u8;
    let check = record.key.generation
        ^ record.key.lease
        ^ record.key.sequence
        ^ record.session
        ^ u64::from(bytes[40]);
    bytes[48..56].copy_from_slice(&(!check).to_be_bytes());
    Ok(bytes)
}

fn decode_record(bytes: &[u8; RECORD_BYTES]) -> io::Result<LaunchRecord> {
    if &bytes[..8] != RECORD_MAGIC || bytes[41..48] != [0; 7] {
        return Err(invalid());
    }
    let number = |offset| u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let record = LaunchRecord {
        key: OperationKey {
            generation: number(8),
            lease: number(16),
            sequence: number(24),
        },
        session: number(32),
        state: match bytes[40] {
            1 => LaunchState::Intent,
            2 => LaunchState::Spawned,
            3 => LaunchState::Failed,
            4 => LaunchState::Applied,
            5 => LaunchState::Unknown,
            _ => return Err(invalid()),
        },
    };
    if encode_record(record)? != *bytes {
        return Err(invalid());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = PathBuf::from(format!(
                "/private/tmp/pl-storage-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Reopen a domain whose singleton lock this process just released by drop.
    ///
    /// `flock` is released when the holding descriptor closes, but another
    /// test's concurrent fork+exec inherits every open descriptor, and that
    /// inherited copy keeps the kernel lock alive until the child execs
    /// (CLOEXEC) or exits. A bounded retry bridges only that fork-to-exec
    /// window; a real competing starter keeps the lock and is still reported
    /// as WouldBlock after the deadline.
    fn reopen_after_drop(path: &std::path::Path) -> io::Result<ServerStorage> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match ServerStorage::open(path) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    #[test]
    fn generation_is_advanced_before_use_and_lock_excludes_second_server() {
        let fixture = Fixture::new();
        let first = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        assert_eq!(first.generation(), 1);
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(first);
        let second = reopen_after_drop(&fixture.0).unwrap();
        assert_eq!(second.generation(), 2);
        assert_eq!(
            fs::metadata(fixture.0.join("generation")).unwrap().mode() & 0o7777,
            0o600
        );
    }

    #[test]
    fn normal_open_never_initializes_a_lost_or_fresh_domain() {
        let fixture = Fixture::new();
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let first = ServerStorage::initialize(&fixture.0).unwrap();
        let generation = first.generation();
        assert_ne!(generation, 0);
        assert_ne!(generation, u64::MAX);
        assert_eq!(
            ServerStorage::initialize(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        drop(first);
        assert_eq!(
            reopen_after_drop(&fixture.0).unwrap().generation(),
            generation + 1
        );
    }

    #[test]
    fn removed_or_replaced_lock_and_lost_journal_fail_closed() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let lock = fixture.0.join("server.lock");
        fs::remove_file(&lock).unwrap();
        assert!(storage.verify_identity().is_err());
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::write(&lock, b"unrelated").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ServerStorage::open(&fixture.0).is_err());
        assert!(storage.journal().is_err());
        drop(storage);

        let another = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&another.0).unwrap();
        fs::remove_file(another.0.join("launch.journal")).unwrap();
        assert_eq!(
            storage.journal().err().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        drop(storage);
        assert_eq!(
            ServerStorage::open(&another.0).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!another.0.join("launch.journal").exists());
    }

    #[test]
    fn unsafe_directory_symlink_and_missing_old_counter_fail_closed() {
        let fixture = Fixture::new();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        drop(storage);
        fs::remove_file(fixture.0.join("generation")).unwrap();
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        symlink("server.lock", fixture.0.join("generation")).unwrap();
        assert!(ServerStorage::open(&fixture.0).is_err());
    }

    #[test]
    fn corrupt_counter_and_uncertain_next_file_never_roll_back() {
        let fixture = Fixture::new();
        drop(ServerStorage::initialize_for_test(&fixture.0).unwrap());
        fs::write(fixture.0.join("generation.next"), b"uncertain").unwrap();
        assert_eq!(
            reopen_after_drop(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_file(fixture.0.join("generation.next")).unwrap();
        fs::write(fixture.0.join("generation"), b"bad").unwrap();
        assert_eq!(
            reopen_after_drop(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn journal_crash_boundaries_recover_uncertain_without_another_spawn() {
        for state in [
            LaunchState::Intent,
            LaunchState::Spawned,
            LaunchState::Applied,
            LaunchState::Failed,
        ] {
            let fixture = Fixture::new();
            let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
            let key = OperationKey {
                generation: storage.generation(),
                lease: 1,
                sequence: 1,
            };
            let intent = LaunchRecord {
                key,
                session: 1,
                state: LaunchState::Intent,
            };
            let mut spawns = 0;
            {
                let mut journal = storage.journal().unwrap();
                journal.record(intent).unwrap();
                if matches!(state, LaunchState::Spawned | LaunchState::Applied) {
                    spawns += 1;
                    journal
                        .record(LaunchRecord {
                            state: LaunchState::Spawned,
                            ..intent
                        })
                        .unwrap();
                }
                if matches!(state, LaunchState::Applied | LaunchState::Failed) {
                    journal.record(LaunchRecord { state, ..intent }).unwrap();
                }
            }
            drop(storage);
            let mut restarted = reopen_after_drop(&fixture.0).unwrap();
            assert_ne!(restarted.generation(), key.generation);
            let mut recovered = restarted.journal().unwrap();
            let expected = if matches!(state, LaunchState::Intent | LaunchState::Spawned) {
                LaunchState::Unknown
            } else {
                state
            };
            assert_eq!(recovered.recovered(key).unwrap().state, expected);
            assert!(
                recovered.record(intent).is_err(),
                "old-generation launch must not restart"
            );
            assert_eq!(
                spawns,
                usize::from(matches!(state, LaunchState::Spawned | LaunchState::Applied))
            );
            let persisted = fs::read(fixture.0.join("launch.journal")).unwrap();
            assert!(persisted.len() <= RECORD_BYTES * 3);
            assert!(!persisted.windows(6).any(|x| x == b"secret"));
        }
    }

    #[test]
    fn journal_write_failure_poison_prevents_dispatch_and_later_writes() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let record = LaunchRecord {
            key: OperationKey {
                generation: storage.generation(),
                lease: 1,
                sequence: 1,
            },
            session: 1,
            state: LaunchState::Intent,
        };
        let mut journal = storage.journal().unwrap();
        journal.file = File::open(fixture.0.join("launch.journal")).unwrap();
        assert!(journal.record(record).is_err());
        assert!(journal.poisoned);
        assert!(journal.record(record).is_err());
        assert!(journal.recovered(record.key).is_none());
    }

    #[test]
    fn launch_preflight_is_read_only_and_rejects_spent_identity() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let key = OperationKey {
            generation: storage.generation(),
            lease: 1,
            sequence: 1,
        };
        let path = fixture.0.join("launch.journal");
        let mut journal = storage.journal().unwrap();
        assert!(journal.preflight_launch(key, 1).is_ok());
        assert!(journal.preflight_launch(key, 1).is_ok());
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        assert!(journal.records.is_empty());
        for invalid_key in [
            OperationKey {
                generation: 0,
                ..key
            },
            OperationKey {
                generation: key.generation + 1,
                ..key
            },
            OperationKey { lease: 0, ..key },
            OperationKey { sequence: 0, ..key },
        ] {
            assert!(journal.preflight_launch(invalid_key, 1).is_err());
        }
        assert!(journal.preflight_launch(key, 0).is_err());
        let record = LaunchRecord {
            key,
            session: 1,
            state: LaunchState::Intent,
        };
        journal.record(record).unwrap();
        journal
            .record(LaunchRecord {
                state: LaunchState::Failed,
                ..record
            })
            .unwrap();
        let next_key = OperationKey { sequence: 2, ..key };
        assert!(journal.preflight_launch(key, 2).is_err());
        assert!(journal.preflight_launch(next_key, 1).is_err());
        assert!(journal
            .record(LaunchRecord {
                key: next_key,
                ..record
            })
            .is_err());
        assert!(journal.preflight_launch(next_key, 2).is_ok());
        assert_eq!(fs::metadata(path).unwrap().len(), 2 * RECORD_BYTES as u64);
    }

    #[test]
    fn launch_preflight_checks_entire_success_path_and_global_key_capacity() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let key = OperationKey {
            generation: storage.generation(),
            lease: 1,
            sequence: 1,
        };
        let mut journal = storage.journal().unwrap();
        // Model boundary values without creating thousands of synced records.
        let headroom = 3 * RECORD_BYTES as u64;
        journal.bytes = MAX_JOURNAL_BYTES - headroom;
        assert!(journal.preflight_launch(key, 1).is_ok());
        journal.bytes += 1;
        assert!(journal.preflight_launch(key, 1).is_err());
        journal.bytes = u64::MAX;
        assert!(journal.preflight_launch(key, 1).is_err());
        journal.bytes = 0;
        journal.poisoned = true;
        assert!(journal.preflight_launch(key, 1).is_err());
        journal.poisoned = false;
        for sequence in 1..=MAX_JOURNAL_KEYS as u64 {
            let historical_key = OperationKey {
                generation: key.generation + 1,
                sequence,
                ..key
            };
            journal.records.insert(
                historical_key,
                LaunchRecord {
                    key: historical_key,
                    session: sequence,
                    state: LaunchState::Failed,
                },
            );
        }
        assert!(journal.preflight_launch(key, 1).is_err());
        assert_eq!(journal.file.metadata().unwrap().len(), 0);
    }

    #[test]
    fn post_intent_and_post_spawn_write_faults_recover_as_unknown() {
        for last_written in [LaunchState::Intent, LaunchState::Spawned] {
            let fixture = Fixture::new();
            let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
            let key = OperationKey {
                generation: storage.generation(),
                lease: 1,
                sequence: 1,
            };
            let record = LaunchRecord {
                key,
                session: 1,
                state: LaunchState::Intent,
            };
            {
                let mut journal = storage.journal().unwrap();
                journal.preflight_launch(key, 1).unwrap();
                journal.record(record).unwrap();
                if last_written == LaunchState::Spawned {
                    journal
                        .record(LaunchRecord {
                            state: last_written,
                            ..record
                        })
                        .unwrap();
                }
                // A genuine write error on the production record path.
                journal.file = File::open(fixture.0.join("launch.journal")).unwrap();
                let next = if last_written == LaunchState::Intent {
                    LaunchState::Spawned
                } else {
                    LaunchState::Applied
                };
                assert!(journal
                    .record(LaunchRecord {
                        state: next,
                        ..record
                    })
                    .is_err());
                assert!(journal.poisoned);
                assert!(journal
                    .preflight_launch(OperationKey { sequence: 2, ..key }, 2)
                    .is_err());
                assert_eq!(journal.recovered(key).unwrap().state, LaunchState::Unknown);
            }
            drop(storage);
            let mut reopened = reopen_after_drop(&fixture.0).unwrap();
            let recovered = reopened.journal().unwrap();
            assert_eq!(
                recovered.recovered(key).unwrap().state,
                LaunchState::Unknown
            );
            assert_ne!(reopened.generation(), key.generation);
        }
    }

    #[test]
    fn journal_rejects_truncated_or_invalid_records_and_overflow_counter() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        drop(storage.journal().unwrap());
        fs::write(fixture.0.join("launch.journal"), b"partial").unwrap();
        assert!(storage.journal().is_err());
        drop(storage);
        let mut bytes = *b"PLGEN001\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
        bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        fs::write(fixture.0.join("generation"), bytes).unwrap();
        assert_eq!(
            reopen_after_drop(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
