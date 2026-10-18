//! Durable, metadata-only operation sequence allocation for a reusable client.
//!
//! A [`ContextFile`] keeps an exclusive advisory lock until it is dropped. A
//! reserved operation is written and fully flushed, including its uncertainty
//! marker, before its key is returned. This module never stores request bodies,
//! targets, environment values, output, or credentials, and never resends input.

use crate::protocol::{OperationKey, OperationStatus};
use rustix::fs::{self, AtFlags, FlockOperation, Mode, OFlags};
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::thread;
use std::time::{Duration, Instant};

const RECORD_MAGIC: &[u8; 4] = b"PLCX";
const RECORD_SCHEMA: u16 = 1;
const SLOT_BYTES: usize = 56;
const RECORD_BYTES: usize = SLOT_BYTES * 2;
const FILE_MODE: u16 = 0o600;
const DIRECTORY_MODE: u16 = 0o700;
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// In-memory metadata for one reusable server generation and client lease.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ClientContext {
    generation: u64,
    lease: u64,
    next_sequence: u64,
    uncertain: Option<OperationKey>,
    revision: u64,
}

impl fmt::Debug for ClientContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientContext { .. }")
    }
}

impl ClientContext {
    /// Construct an empty client context with initial sequence 1.
    pub fn new(generation: u64, lease: u64) -> Result<Self, ContextError> {
        if generation == 0 || lease == 0 {
            return Err(ContextError::InvalidIdentity);
        }
        Ok(Self {
            generation,
            lease,
            next_sequence: 1,
            uncertain: None,
            revision: 1,
        })
    }

    /// The server generation bound to this context.
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// The client lease bound to this context.
    pub const fn lease(self) -> u64 {
        self.lease
    }

    /// The next sequence that a durable reservation would allocate.
    pub const fn next_sequence(self) -> u64 {
        self.next_sequence
    }

    /// An operation whose outcome still requires an exact response or reconciliation.
    pub const fn uncertain(self) -> Option<OperationKey> {
        self.uncertain
    }
}

/// Outcome of observing the server's status for an uncertain operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileResult {
    /// The server still reports work in progress; the context remains blocked.
    StillInProgress,
    /// A terminal result or definitive non-admission cleared the uncertainty marker.
    Cleared,
}

/// Fixed, non-user-data errors from the persistent context owner.
#[derive(Debug)]
pub enum ContextError {
    /// Generation, lease, sequence, or operation-key identity is zero or invalid.
    InvalidIdentity,
    /// The context file's parent is not an owned, private directory.
    UnsafeParent,
    /// The context file is not an owned regular file with exact mode 0600.
    UnsafeFile,
    /// The on-disk record is truncated, malformed, ambiguous, or has bad checksums.
    InvalidRecord,
    /// The stored generation or lease does not match the requested identity.
    IdentityMismatch,
    /// An exclusive lock was unavailable before the absolute deadline.
    LockTimeout,
    /// A sequence value cannot be incremented without reuse or overflow.
    SequenceExhausted,
    /// The record revision cannot be incremented without overflow.
    RevisionExhausted,
    /// An uncertain operation must be acknowledged or reconciled first.
    UncertainOperation,
    /// No uncertain operation exists to acknowledge or reconcile.
    NoUncertainOperation,
    /// The supplied key is not the exact uncertain operation.
    OperationKeyMismatch,
    /// The file guard encountered a durable-storage failure and is poisoned.
    Poisoned,
    /// An operating-system operation failed.
    Io(io::Error),
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity => f.write_str("client context identity is invalid"),
            Self::UnsafeParent => f.write_str("client context parent is not safely private"),
            Self::UnsafeFile => f.write_str("client context file is not safely private"),
            Self::InvalidRecord => f.write_str("client context record is invalid"),
            Self::IdentityMismatch => f.write_str("client context identity does not match"),
            Self::LockTimeout => f.write_str("client context lock deadline expired"),
            Self::SequenceExhausted => f.write_str("client context sequence is exhausted"),
            Self::RevisionExhausted => f.write_str("client context revision is exhausted"),
            Self::UncertainOperation => f.write_str("client context has an uncertain operation"),
            Self::NoUncertainOperation => f.write_str("client context has no uncertain operation"),
            Self::OperationKeyMismatch => {
                f.write_str("operation key does not match client context")
            }
            Self::Poisoned => f.write_str("client context guard is poisoned"),
            Self::Io(error) => write!(f, "client context storage failed: {error}"),
        }
    }
}

impl Error for ContextError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ContextError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rustix::io::Errno> for ContextError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

/// Locked handle to one fixed-size context record.
///
/// The file descriptor retains an exclusive `flock` through response handling.
/// Call [`ContextFile::ack`] only after validating the exact operation response;
/// if the client exits first, reopen and query status with [`ContextFile::reconcile`].
pub struct ContextFile {
    file: File,
    parent: File,
    filename: OsString,
    active_slot: usize,
    context: ClientContext,
    poisoned: bool,
    #[cfg(test)]
    fail_next_flush: bool,
}

impl fmt::Debug for ContextFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextFile")
            .field("locked", &true)
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

impl ContextFile {
    /// Exclusively create a new context file and its initial durable record.
    ///
    /// The path must be absolute, its existing parent must be owned by this
    /// effective user with mode 0700, and the final file is created with mode
    /// 0600 using `O_NOFOLLOW|O_EXCL`. An existing file is never reset.
    pub fn create(
        path: &Path,
        generation: u64,
        lease: u64,
        deadline: Instant,
    ) -> Result<Self, ContextError> {
        if Instant::now() >= deadline {
            return Err(ContextError::LockTimeout);
        }
        let context = ClientContext::new(generation, lease)?;
        let (parent, filename) = open_private_parent(path)?;
        let fd = fs::openat(
            &parent,
            Path::new(&filename),
            OFlags::RDWR
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC
                | OFlags::NONBLOCK,
            Mode::from_raw_mode(FILE_MODE),
        )?;
        let file = File::from(fd);
        lock_until(&file, deadline)?;
        fs::fchmod(&file, Mode::from_raw_mode(FILE_MODE))?;
        validate_private_file(&file)?;
        file.set_len(RECORD_BYTES as u64)?;
        let slot = encode_slot(context);
        let mut guard = Self {
            file,
            parent,
            filename,
            active_slot: 0,
            context,
            poisoned: false,
            #[cfg(test)]
            fail_next_flush: false,
        };
        guard.file.seek(SeekFrom::Start(0))?;
        guard.file.write_all(&slot)?;
        guard.full_flush()?;
        fs::fsync(&guard.parent)?;
        guard.full_flush()?;
        guard.ensure_path_identity()?;
        Ok(guard)
    }

    /// Open and lock an existing context file for its exact generation and lease.
    ///
    /// Missing, unsafe, malformed, truncated, or stale files fail closed; this
    /// method never initializes or replaces a record.
    pub fn open(
        path: &Path,
        expected_generation: u64,
        expected_lease: u64,
        deadline: Instant,
    ) -> Result<Self, ContextError> {
        if expected_generation == 0 || expected_lease == 0 {
            return Err(ContextError::InvalidIdentity);
        }
        let guard = Self::open_existing(path, deadline)?;
        if guard.context.generation != expected_generation || guard.context.lease != expected_lease
        {
            return Err(ContextError::IdentityMismatch);
        }
        Ok(guard)
    }

    /// Load a locked context locator without inventing or resetting its identity.
    /// The caller must resume this exact generation/lease with the server; these
    /// file values are metadata, never authentication or permission grants.
    pub fn open_existing(path: &Path, deadline: Instant) -> Result<Self, ContextError> {
        let (parent, filename) = open_private_parent(path)?;
        let fd = fs::openat(
            &parent,
            Path::new(&filename),
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        let file = File::from(fd);
        lock_until(&file, deadline)?;
        validate_private_file(&file)?;
        let (context, active_slot) = read_context(&file)?;
        let guard = Self {
            file,
            parent,
            filename,
            active_slot,
            context,
            poisoned: false,
            #[cfg(test)]
            fail_next_flush: false,
        };
        guard.ensure_path_identity()?;
        Ok(guard)
    }

    /// Return a copy of the currently loaded metadata.
    pub const fn context(&self) -> ClientContext {
        self.context
    }

    /// Durably reserve the next key and mark its outcome uncertain before returning it.
    ///
    /// A returned key may identify a submitted request; only server admission
    /// authorizes its effects.
    /// Any write or full-flush failure poisons this guard and returns no key.
    pub fn reserve(&mut self) -> Result<OperationKey, ContextError> {
        self.ensure_usable()?;
        if self.context.uncertain.is_some() {
            return Err(ContextError::UncertainOperation);
        }
        let sequence = self.context.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(ContextError::SequenceExhausted)?;
        let revision = self
            .context
            .revision
            .checked_add(1)
            .ok_or(ContextError::RevisionExhausted)?;
        let key = OperationKey {
            generation: self.context.generation,
            lease: self.context.lease,
            sequence,
        };
        let candidate = ClientContext {
            next_sequence,
            uncertain: Some(key),
            revision,
            ..self.context
        };
        self.persist_candidate(candidate)?;
        Ok(key)
    }

    /// Clear uncertainty after the caller has validated the exact response key.
    ///
    /// The metadata clear is fully flushed before this method succeeds. The lock
    /// remains held until this guard is dropped.
    pub fn ack(&mut self, key: OperationKey) -> Result<(), ContextError> {
        self.ensure_usable()?;
        self.ensure_uncertain_key(key)?;
        let revision = self
            .context
            .revision
            .checked_add(1)
            .ok_or(ContextError::RevisionExhausted)?;
        let candidate = ClientContext {
            uncertain: None,
            revision,
            ..self.context
        };
        self.persist_candidate(candidate)
    }

    /// Observe an uncertain key's status and clear it only when status is terminal
    /// or definitively not admitted. `InProgress` remains blocked and is never resent.
    pub fn reconcile(
        &mut self,
        key: OperationKey,
        status: OperationStatus,
    ) -> Result<ReconcileResult, ContextError> {
        self.ensure_usable()?;
        self.ensure_uncertain_key(key)?;
        match status {
            OperationStatus::InProgress => Ok(ReconcileResult::StillInProgress),
            OperationStatus::NotAdmitted | OperationStatus::Completed(_) => {
                self.ack(key)?;
                Ok(ReconcileResult::Cleared)
            }
        }
    }

    fn ensure_usable(&self) -> Result<(), ContextError> {
        if self.poisoned {
            Err(ContextError::Poisoned)
        } else {
            Ok(())
        }
    }

    fn ensure_uncertain_key(&self, key: OperationKey) -> Result<(), ContextError> {
        if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
            return Err(ContextError::InvalidIdentity);
        }
        match self.context.uncertain {
            None => Err(ContextError::NoUncertainOperation),
            Some(uncertain) if uncertain != key => Err(ContextError::OperationKeyMismatch),
            Some(_) => Ok(()),
        }
    }

    fn persist_candidate(&mut self, candidate: ClientContext) -> Result<(), ContextError> {
        if let Err(error) = self.write_candidate(candidate) {
            self.poisoned = true;
            return Err(error);
        }
        self.context = candidate;
        self.active_slot = 1 - self.active_slot;
        Ok(())
    }

    fn write_candidate(&mut self, candidate: ClientContext) -> Result<(), ContextError> {
        self.ensure_path_identity()?;
        let slot = encode_slot(candidate);
        let offset = (1 - self.active_slot) * SLOT_BYTES;
        self.file.seek(SeekFrom::Start(offset as u64))?;
        self.file.write_all(&slot)?;
        self.full_flush()?;
        Ok(())
    }

    fn full_flush(&mut self) -> Result<(), ContextError> {
        #[cfg(test)]
        if self.fail_next_flush {
            self.fail_next_flush = false;
            return Err(io::Error::other("injected full-flush failure").into());
        }
        fs::fcntl_fullfsync(&self.file)?;
        Ok(())
    }

    fn ensure_path_identity(&self) -> Result<(), ContextError> {
        let path_stat = fs::statat(
            &self.parent,
            Path::new(&self.filename),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|_| ContextError::UnsafeFile)?;
        let file_metadata = self.file.metadata().map_err(ContextError::Io)?;
        if path_stat.st_dev as u64 != file_metadata.dev()
            || path_stat.st_ino as u64 != file_metadata.ino()
        {
            return Err(ContextError::UnsafeFile);
        }
        Ok(())
    }
}

fn open_private_parent(path: &Path) -> Result<(File, OsString), ContextError> {
    if !path.is_absolute() || path.as_os_str().as_bytes().last() == Some(&b'/') {
        return Err(ContextError::UnsafeParent);
    }
    let filename = path.file_name().ok_or(ContextError::UnsafeParent)?;
    let parent_path = path.parent().ok_or(ContextError::UnsafeParent)?;
    if filename.is_empty()
        || parent_path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(ContextError::UnsafeParent);
    }
    let fd = fs::open(
        parent_path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| ContextError::UnsafeParent)?;
    let parent = File::from(fd);
    validate_private_directory(&parent)?;
    Ok((parent, filename.to_os_string()))
}

fn validate_private_directory(directory: &File) -> Result<(), ContextError> {
    let metadata = directory
        .metadata()
        .map_err(|_| ContextError::UnsafeParent)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != u32::from(DIRECTORY_MODE)
    {
        return Err(ContextError::UnsafeParent);
    }
    Ok(())
}

fn validate_private_file(file: &File) -> Result<(), ContextError> {
    let metadata = file.metadata().map_err(|_| ContextError::UnsafeFile)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != u32::from(FILE_MODE)
        || metadata.nlink() != 1
    {
        return Err(ContextError::UnsafeFile);
    }
    Ok(())
}

fn lock_until(file: &File, deadline: Instant) -> Result<(), ContextError> {
    loop {
        if Instant::now() >= deadline {
            return Err(ContextError::LockTimeout);
        }
        match fs::flock(file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ContextError::LockTimeout);
                }
                thread::sleep(remaining.min(LOCK_POLL_INTERVAL));
            }
            Err(error) => return Err(ContextError::Io(error.into())),
        }
    }
}

fn read_context(file: &File) -> Result<(ClientContext, usize), ContextError> {
    let metadata = file.metadata()?;
    if metadata.len() != RECORD_BYTES as u64 {
        return Err(ContextError::InvalidRecord);
    }
    let mut bytes = [0_u8; RECORD_BYTES];
    let mut reader = file;
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut bytes)?;
    let first_bytes = &bytes[..SLOT_BYTES];
    let second_bytes = &bytes[SLOT_BYTES..];
    let first = decode_slot(first_bytes);
    let second = decode_slot(second_bytes);
    if let (Some(first), Some(second)) = (first, second) {
        if first.generation != second.generation || first.lease != second.lease {
            return Err(ContextError::InvalidRecord);
        }
        let (older, newer) = if first.revision <= second.revision {
            (first, second)
        } else {
            (second, first)
        };
        if older.revision != newer.revision {
            if older.revision.checked_add(1) != Some(newer.revision) {
                return Err(ContextError::InvalidRecord);
            }
            let reserved = older.uncertain.is_none()
                && older.next_sequence.checked_add(1) == Some(newer.next_sequence)
                && newer.uncertain.is_some();
            let acknowledged = older.uncertain.is_some()
                && newer.uncertain.is_none()
                && older.next_sequence == newer.next_sequence;
            if !reserved && !acknowledged {
                return Err(ContextError::InvalidRecord);
            }
        }
    }
    match (first, second) {
        (None, None) => Err(ContextError::InvalidRecord),
        (Some(context), None)
            if second_bytes.iter().all(|byte| *byte == 0)
                && context.revision == 1
                && context.next_sequence == 1
                && context.uncertain.is_none() =>
        {
            Ok((context, 0))
        }
        (None, Some(_)) | (Some(_), None) => Err(ContextError::InvalidRecord),
        (Some(first), Some(second)) if first.revision > second.revision => Ok((first, 0)),
        (Some(first), Some(second)) if second.revision > first.revision => Ok((second, 1)),
        (Some(first), Some(second)) if first == second => Ok((first, 0)),
        (Some(_), Some(_)) => Err(ContextError::InvalidRecord),
    }
}

fn encode_slot(context: ClientContext) -> [u8; SLOT_BYTES] {
    let mut bytes = [0_u8; SLOT_BYTES];
    bytes[0..4].copy_from_slice(RECORD_MAGIC);
    bytes[4..6].copy_from_slice(&RECORD_SCHEMA.to_be_bytes());
    bytes[8..16].copy_from_slice(&context.revision.to_be_bytes());
    bytes[16..24].copy_from_slice(&context.generation.to_be_bytes());
    bytes[24..32].copy_from_slice(&context.lease.to_be_bytes());
    bytes[32..40].copy_from_slice(&context.next_sequence.to_be_bytes());
    let uncertain_sequence = context.uncertain.map_or(0, |key| key.sequence);
    bytes[40..48].copy_from_slice(&uncertain_sequence.to_be_bytes());
    let checksum = checksum64(&bytes[..48]);
    bytes[48..56].copy_from_slice(&checksum.to_be_bytes());
    bytes
}

fn decode_slot(bytes: &[u8]) -> Option<ClientContext> {
    if bytes.len() != SLOT_BYTES
        || &bytes[0..4] != RECORD_MAGIC
        || u16::from_be_bytes([bytes[4], bytes[5]]) != RECORD_SCHEMA
        || bytes[6..8] != [0, 0]
        || u64::from_be_bytes(bytes[48..56].try_into().ok()?) != checksum64(&bytes[..48])
    {
        return None;
    }
    let revision = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
    let generation = u64::from_be_bytes(bytes[16..24].try_into().ok()?);
    let lease = u64::from_be_bytes(bytes[24..32].try_into().ok()?);
    let next_sequence = u64::from_be_bytes(bytes[32..40].try_into().ok()?);
    let uncertain_sequence = u64::from_be_bytes(bytes[40..48].try_into().ok()?);
    if revision == 0 || generation == 0 || lease == 0 || next_sequence == 0 {
        return None;
    }
    let uncertain = if uncertain_sequence == 0 {
        None
    } else if next_sequence.checked_sub(1) == Some(uncertain_sequence) {
        Some(OperationKey {
            generation,
            lease,
            sequence: uncertain_sequence,
        })
    } else {
        return None;
    };
    Some(ClientContext {
        generation,
        lease,
        next_sequence,
        uncertain,
        revision,
    })
}

fn checksum64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Outcome;
    use std::fs::{self as std_fs, Permissions};
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from(format!(
                "/private/tmp/panelink-context-{}-{serial}",
                std::process::id()
            ));
            std_fs::DirBuilder::new()
                .mode(u32::from(DIRECTORY_MODE))
                .create(&path)
                .expect("create private test directory");
            Self(path)
        }

        fn context_path(&self) -> PathBuf {
            self.0.join("context.bin")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std_fs::remove_file(self.context_path());
            let _ = std_fs::remove_file(self.0.join("link.bin"));
            let _ = std_fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn valid_checksums_cannot_hide_cross_identity_or_rolled_back_slots() {
        for variant in 0..3 {
            let fixture = Fixture::new();
            let path = fixture.context_path();
            let context = ContextFile::create(&path, 7, 11, deadline()).unwrap();
            let old = context.context();
            drop(context);
            let mut next = ClientContext {
                revision: 2,
                next_sequence: 2,
                uncertain: Some(OperationKey {
                    generation: 7,
                    lease: 11,
                    sequence: 1,
                }),
                ..old
            };
            match variant {
                0 => {
                    next.generation = 8;
                    next.uncertain.as_mut().unwrap().generation = 8;
                }
                1 => next.revision = 3,
                _ => {
                    next.next_sequence = 1;
                    next.uncertain = None;
                }
            }
            let mut file = std_fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(SLOT_BYTES as u64)).unwrap();
            file.write_all(&encode_slot(next)).unwrap();
            assert!(matches!(
                ContextFile::open(&path, 7, 11, deadline()),
                Err(ContextError::InvalidRecord)
            ));
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    #[test]
    fn metadata_record_is_fixed_bounded_and_contains_only_context_fields() {
        let context = ClientContext::new(7, 11).unwrap();
        let first = encode_slot(context);
        assert_eq!(first.len(), SLOT_BYTES);
        assert_eq!(RECORD_BYTES, 112);
        assert_eq!(decode_slot(&first), Some(context));
        let with_uncertain = ClientContext {
            next_sequence: 2,
            uncertain: Some(OperationKey {
                generation: 7,
                lease: 11,
                sequence: 1,
            }),
            revision: 2,
            ..context
        };
        assert_eq!(
            decode_slot(&encode_slot(with_uncertain)),
            Some(with_uncertain)
        );
        assert_eq!(format!("{context:?}"), "ClientContext { .. }");
        assert_eq!(
            format!("{:?}", ContextError::InvalidRecord),
            "InvalidRecord"
        );
    }

    #[test]
    fn uncertain_reservation_survives_reopen_and_never_reuses_sequence() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let first_key = {
            let mut context = ContextFile::create(&path, 7, 11, deadline()).unwrap();
            let key = context.reserve().unwrap();
            assert_eq!(key.sequence, 1);
            assert!(matches!(
                context.reserve(),
                Err(ContextError::UncertainOperation)
            ));
            key
        };

        let mut context = ContextFile::open(&path, 7, 11, deadline()).unwrap();
        assert_eq!(context.context().uncertain(), Some(first_key));
        assert!(matches!(
            context.reconcile(first_key, OperationStatus::InProgress),
            Ok(ReconcileResult::StillInProgress)
        ));
        assert_eq!(context.context().uncertain(), Some(first_key));
        assert!(matches!(
            context.reconcile(first_key, OperationStatus::NotAdmitted),
            Ok(ReconcileResult::Cleared)
        ));
        let second_key = context.reserve().unwrap();
        assert_eq!(second_key.sequence, 2);
        context.ack(second_key).unwrap();
        drop(context);

        let context = ContextFile::open(&path, 7, 11, deadline()).unwrap();
        assert_eq!(context.context().next_sequence(), 3);
        assert_eq!(context.context().uncertain(), None);
    }

    #[test]
    fn completed_reconciliation_clears_only_the_exact_uncertain_key() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let mut context = ContextFile::create(&path, 1, 2, deadline()).unwrap();
        let key = context.reserve().unwrap();
        let wrong = OperationKey { lease: 3, ..key };
        assert!(matches!(
            context.ack(wrong),
            Err(ContextError::OperationKeyMismatch)
        ));
        assert!(matches!(
            context.reconcile(key, OperationStatus::Completed(Outcome::Applied)),
            Ok(ReconcileResult::Cleared)
        ));
        assert_eq!(context.context().uncertain(), None);
    }

    #[test]
    fn two_guards_cannot_hold_the_same_path_lock_at_once() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let first = ContextFile::create(&path, 1, 2, deadline()).unwrap();
        let blocked = ContextFile::open(&path, 1, 2, Instant::now() + Duration::from_millis(20));
        assert!(matches!(blocked, Err(ContextError::LockTimeout)));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--ignored")
            .arg("--exact")
            .arg("client_context::tests::separate_process_lock_probe")
            .env("PANELINK_CONTEXT_LOCK_TEST_PATH", &path)
            .output()
            .expect("spawn isolated lock probe");
        assert!(
            child.status.success(),
            "separate process lock probe failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        drop(first);
        assert!(ContextFile::open(&path, 1, 2, deadline()).is_ok());
    }

    #[test]
    #[ignore = "spawned only by the separate-process lock regression"]
    fn separate_process_lock_probe() {
        let path = std::env::var_os("PANELINK_CONTEXT_LOCK_TEST_PATH")
            .expect("parent supplies the locked context path");
        let result = ContextFile::open(
            Path::new(&path),
            1,
            2,
            Instant::now() + Duration::from_millis(40),
        );
        assert!(matches!(result, Err(ContextError::LockTimeout)));
    }

    #[test]
    fn missing_invalid_truncated_and_stale_files_fail_closed() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        assert!(matches!(
            ContextFile::open(&path, 1, 2, deadline()),
            Err(ContextError::Io(_))
        ));
        ContextFile::create(&path, 1, 2, deadline()).unwrap();
        assert!(matches!(
            ContextFile::create(&path, 1, 2, deadline()),
            Err(ContextError::Io(_))
        ));
        assert!(matches!(
            ContextFile::open(&path, 9, 2, deadline()),
            Err(ContextError::IdentityMismatch)
        ));
        drop(ContextFile::open(&path, 1, 2, deadline()).unwrap());
        std_fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(20)
            .unwrap();
        assert!(matches!(
            ContextFile::open(&path, 1, 2, deadline()),
            Err(ContextError::InvalidRecord)
        ));
    }

    #[test]
    fn rejects_symlinks_and_unsafe_parent_or_file_modes() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        ContextFile::create(&path, 1, 2, deadline()).unwrap();
        let symlink_path = fixture.0.join("link.bin");
        symlink(&path, &symlink_path).unwrap();
        assert!(matches!(
            ContextFile::open(&symlink_path, 1, 2, deadline()),
            Err(ContextError::Io(_))
        ));
        drop(ContextFile::open(&path, 1, 2, deadline()).unwrap());
        std_fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            ContextFile::open(&path, 1, 2, deadline()),
            Err(ContextError::UnsafeFile)
        ));
        std_fs::set_permissions(&fixture.0, Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            ContextFile::open(&path, 1, 2, deadline()),
            Err(ContextError::UnsafeParent)
        ));
    }

    #[test]
    fn full_flush_failure_returns_no_key_and_poisons_the_guard() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let mut context = ContextFile::create(&path, 1, 2, deadline()).unwrap();
        context.fail_next_flush = true;
        assert!(matches!(context.reserve(), Err(ContextError::Io(_))));
        assert!(matches!(context.reserve(), Err(ContextError::Poisoned)));
        drop(context);

        let context = ContextFile::open(&path, 1, 2, deadline()).unwrap();
        if context.context().uncertain().is_some() {
            assert_eq!(context.context().next_sequence(), 2);
        } else {
            assert_eq!(context.context().next_sequence(), 1);
        }
    }

    #[test]
    fn expired_lock_deadline_returns_before_acquiring() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        assert!(matches!(
            ContextFile::create(&path, 1, 2, Instant::now()),
            Err(ContextError::LockTimeout)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn sequence_and_record_revision_overflow_fail_before_mutation() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let mut context = ContextFile::create(&path, 1, 2, deadline()).unwrap();
        context.context.next_sequence = u64::MAX;
        assert!(matches!(
            context.reserve(),
            Err(ContextError::SequenceExhausted)
        ));
        assert_eq!(context.context.next_sequence, u64::MAX);
        assert_eq!(context.context.uncertain, None);

        context.context.next_sequence = 1;
        context.context.revision = u64::MAX;
        assert!(matches!(
            context.reserve(),
            Err(ContextError::RevisionExhausted)
        ));
        assert_eq!(context.context.next_sequence, 1);
        assert_eq!(context.context.uncertain, None);
    }

    #[test]
    fn codec_detects_truncated_or_changed_slots_and_rejects_bad_invariants() {
        let mut bytes = encode_slot(ClientContext::new(1, 2).unwrap());
        assert!(decode_slot(&bytes).is_some());
        bytes[33] = 1;
        assert!(decode_slot(&bytes).is_none());
        bytes = encode_slot(ClientContext::new(1, 2).unwrap());
        bytes[40..48].copy_from_slice(&1_u64.to_be_bytes());
        assert!(decode_slot(&bytes).is_none());
        assert!(decode_slot(&bytes[..20]).is_none());
    }

    #[test]
    fn damaged_newest_slot_does_not_fall_back_to_a_reusable_old_sequence() {
        let fixture = Fixture::new();
        let path = fixture.context_path();
        let mut context = ContextFile::create(&path, 1, 2, deadline()).unwrap();
        let key = context.reserve().unwrap();
        context.ack(key).unwrap();
        let newest_slot = context.active_slot;
        drop(context);

        let mut file = std_fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start((newest_slot * SLOT_BYTES + 12) as u64))
            .unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        assert!(matches!(
            ContextFile::open(&path, 1, 2, deadline()),
            Err(ContextError::InvalidRecord)
        ));
    }
}
