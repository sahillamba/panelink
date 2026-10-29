//! Locked, bounded persistence for the project/workspace metadata registry.
//!
//! The caller supplies an already existing private application root. This
//! module never discovers roots or touches server, context, journal, history,
//! socket, or process state. The record contains metadata only.

use super::codec::MetadataRecord;
use super::{ProjectId, WorkspaceIndex, WorkspaceTabId};
use rustix::fs::{flock, open, openat, renameat, unlinkat, AtFlags, FlockOperation, Mode, OFlags};
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const REGISTRY_NAME: &str = "workspace-index.v1";
const LOCK_NAME: &str = "workspace-index.lock";
const TEMP_NAME: &str = "workspace-index.v1.tmp";
// Kept local so the pure codec API and its frozen surface remain unchanged.
const MAX_RECORD_BYTES: usize = 23_352;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    UnsafeRoot,
    UnsafeFile,
    CorruptRecord,
    Locked,
    Io,
    Uninitialized,
    RevisionConflict,
    InvalidTransition,
    NeedsReopen,
    AlreadyInitialized,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsafeRoot => "unsafe workspace registry root",
            Self::UnsafeFile => "unsafe workspace registry file",
            Self::CorruptRecord => "invalid workspace registry record",
            Self::Locked => "workspace registry is locked",
            Self::Io => "workspace registry I/O failed",
            Self::Uninitialized => "workspace registry is not initialized",
            Self::RevisionConflict => "workspace registry revision conflict",
            Self::InvalidTransition => "invalid workspace registry transition",
            Self::NeedsReopen => "workspace registry requires reopen",
            Self::AlreadyInitialized => "workspace registry already exists",
        })
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    Written,
    AlreadyDurable,
}

/// An exclusive lifetime lock and one exact registry slot under `root`.
pub struct WorkspaceIndexStore {
    pub(super) root: PathBuf,
    pub(super) directory: File,
    lock: File,
    pub(super) disabled: bool,
    #[cfg(test)]
    fault: Option<FaultPoint>,
}

impl fmt::Debug for WorkspaceIndexStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkspaceIndexStore { <redacted> }")
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultPoint {
    BeforeRename,
    AfterRename,
    ReplaceTempWithSymlink,
    HardlinkTemp,
}

#[derive(Clone)]
struct ExistingRecord {
    identity: (u64, u64),
    len: u64,
    bytes: Vec<u8>,
}

type CurrentRecord = (
    Option<MetadataRecord>,
    Option<ExistingRecord>,
    Option<Vec<u8>>,
);

fn uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

pub(super) fn private_file(file: &File, allow_empty: bool) -> Result<(), StoreError> {
    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
    if !metadata.is_file()
        || metadata.uid() != uid()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || (!allow_empty && metadata.len() == 0)
    {
        return Err(StoreError::UnsafeFile);
    }
    Ok(())
}

fn root_path_safe(path: &Path) -> Result<(), StoreError> {
    if !path.is_absolute() || fs::canonicalize(path).map_err(|_| StoreError::UnsafeRoot)? != path {
        return Err(StoreError::UnsafeRoot);
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix).map_err(|_| StoreError::UnsafeRoot)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || (metadata.uid() != uid() && metadata.uid() != 0)
            || (metadata.permissions().mode() & 0o022 != 0
                && !(metadata.uid() == 0 && metadata.permissions().mode() & 0o1000 != 0))
        {
            return Err(StoreError::UnsafeRoot);
        }
    }
    Ok(())
}

fn verify_root(path: &Path, directory: &File) -> Result<(), StoreError> {
    root_path_safe(path)?;
    let named = fs::symlink_metadata(path).map_err(|_| StoreError::UnsafeRoot)?;
    let held = directory.metadata().map_err(|_| StoreError::Io)?;
    if held.uid() != uid()
        || held.permissions().mode() & 0o7777 != 0o700
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
    {
        return Err(StoreError::UnsafeRoot);
    }
    Ok(())
}

pub(super) fn open_named(directory: &File, name: &str) -> Result<Option<File>, StoreError> {
    match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(None),
        Err(_) => Err(StoreError::UnsafeFile),
    }
}

fn record_bytes(file: &File) -> Result<Vec<u8>, StoreError> {
    private_file(file, false)?;
    let size = file.metadata().map_err(|_| StoreError::Io)?.len();
    if size as usize > MAX_RECORD_BYTES {
        return Err(StoreError::CorruptRecord);
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let remaining = (size as usize).saturating_sub(bytes.len());
        let mut chunk = [0_u8; 4096];
        let count = remaining.min(chunk.len());
        let read = file
            .read_at(&mut chunk[..count], bytes.len() as u64)
            .map_err(|_| StoreError::Io)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let mut extra = [0_u8; 1];
    let has_extra = file.read_at(&mut extra, size).map_err(|_| StoreError::Io)? != 0;
    if bytes.len() as u64 != size || bytes.len() > MAX_RECORD_BYTES || has_extra {
        return Err(StoreError::CorruptRecord);
    }
    Ok(bytes)
}

impl WorkspaceIndexStore {
    /// Opens an existing private application root and takes its lifetime lock.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref();
        root_path_safe(root)?;
        let directory = File::from(
            open(
                root,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| StoreError::UnsafeRoot)?,
        );
        verify_root(root, &directory)?;
        let lock = match openat(
            &directory,
            LOCK_NAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => File::from(fd),
            Err(_) => return Err(StoreError::UnsafeFile),
        };
        private_file(&lock, true)?;
        if lock.metadata().map_err(|_| StoreError::Io)?.len() != 0 {
            return Err(StoreError::UnsafeFile);
        }
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|_| StoreError::Locked)?;
        private_file(&lock, true)?;
        let named_lock = open_named(&directory, LOCK_NAME)?.ok_or(StoreError::UnsafeFile)?;
        private_file(&named_lock, true)?;
        if lock.metadata().map_err(|_| StoreError::Io)?.dev()
            != named_lock.metadata().map_err(|_| StoreError::Io)?.dev()
            || lock.metadata().map_err(|_| StoreError::Io)?.ino()
                != named_lock.metadata().map_err(|_| StoreError::Io)?.ino()
        {
            return Err(StoreError::UnsafeFile);
        }
        verify_root(root, &directory)?;
        Ok(Self {
            root: root.to_owned(),
            directory,
            lock,
            disabled: false,
            #[cfg(test)]
            fault: None,
        })
    }

    pub(super) fn verify_identity(&self) -> Result<(), StoreError> {
        verify_root(&self.root, &self.directory)?;
        private_file(&self.lock, true)?;
        if self.lock.metadata().map_err(|_| StoreError::Io)?.len() != 0 {
            return Err(StoreError::UnsafeFile);
        }
        let named = open_named(&self.directory, LOCK_NAME)?.ok_or(StoreError::UnsafeFile)?;
        private_file(&named, true)?;
        let held = self.lock.metadata().map_err(|_| StoreError::Io)?;
        let current = named.metadata().map_err(|_| StoreError::Io)?;
        if (held.dev(), held.ino()) != (current.dev(), current.ino()) {
            return Err(StoreError::UnsafeFile);
        }
        Ok(())
    }

    fn current(&self) -> Result<CurrentRecord, StoreError> {
        self.verify_identity()?;
        let Some(file) = open_named(&self.directory, REGISTRY_NAME)? else {
            return Ok((None, None, None));
        };
        let metadata = file.metadata().map_err(|_| StoreError::Io)?;
        let bytes = record_bytes(&file)?;
        let record = MetadataRecord::decode(&bytes).map_err(|_| StoreError::CorruptRecord)?;
        self.verify_identity()?;
        let named = open_named(&self.directory, REGISTRY_NAME)?.ok_or(StoreError::UnsafeFile)?;
        private_file(&named, false)?;
        let named_metadata = named.metadata().map_err(|_| StoreError::Io)?;
        if (metadata.dev(), metadata.ino(), metadata.len())
            != (
                named_metadata.dev(),
                named_metadata.ino(),
                named_metadata.len(),
            )
            || record_bytes(&named)? != bytes
        {
            return Err(StoreError::UnsafeFile);
        }
        Ok((
            Some(record),
            Some(ExistingRecord {
                identity: (metadata.dev(), metadata.ino()),
                len: metadata.len(),
                bytes: bytes.clone(),
            }),
            Some(bytes),
        ))
    }

    /// Returns `None` only when no registry record has ever been initialized.
    pub fn load(&self) -> Result<Option<MetadataRecord>, StoreError> {
        self.current().map(|(record, _, _)| record)
    }

    /// Additively registers the legacy runtime as Project 1 / tab 1.
    /// This writes only the metadata registry and leaves every legacy subtree
    /// and running process untouched. It refuses to replace any existing slot.
    pub fn initialize_legacy(&mut self) -> Result<SaveOutcome, StoreError> {
        if self.disabled {
            return Err(StoreError::NeedsReopen);
        }
        let (existing, _, _) = self.current()?;
        if existing.is_some() {
            return Err(StoreError::AlreadyInitialized);
        }
        let mut index = WorkspaceIndex::new();
        index
            .add_project(
                ProjectId::new(1).map_err(|_| StoreError::InvalidTransition)?,
                "Default",
                WorkspaceTabId::new(1).map_err(|_| StoreError::InvalidTransition)?,
                "Main",
            )
            .map_err(|_| StoreError::InvalidTransition)?;
        let record = MetadataRecord::new(index, 1, 2).map_err(|_| StoreError::InvalidTransition)?;
        self.publish(
            None,
            &record.encode().map_err(|_| StoreError::InvalidTransition)?,
        )
    }

    /// Publishes revision `expected_revision + 1`, retaining IDs and tab
    /// project ownership. An exact retry of the committed bytes is accepted
    /// as `AlreadyDurable` to resolve a lost acknowledgment.
    pub fn save(
        &mut self,
        expected_revision: u64,
        new_record: &MetadataRecord,
    ) -> Result<SaveOutcome, StoreError> {
        if self.disabled {
            return Err(StoreError::NeedsReopen);
        }
        let (current, identity, current_bytes) = self.current()?;
        let bytes = new_record
            .encode()
            .map_err(|_| StoreError::InvalidTransition)?;
        if let Some(current) = current.as_ref() {
            if new_record.revision() == current.revision()
                && expected_revision.checked_add(1) == Some(new_record.revision())
                && current_bytes.as_deref() == Some(bytes.as_slice())
            {
                return self.publish(identity, &bytes);
            }
            if current.revision() != expected_revision
                || expected_revision.checked_add(1) != Some(new_record.revision())
            {
                return Err(StoreError::RevisionConflict);
            }
            validate_transition(current, new_record)?;
        } else {
            return Err(StoreError::Uninitialized);
        }
        self.publish(identity, &bytes)
    }

    fn publish(
        &mut self,
        previous: Option<ExistingRecord>,
        bytes: &[u8],
    ) -> Result<SaveOutcome, StoreError> {
        self.verify_identity()?;
        let named = open_named(&self.directory, REGISTRY_NAME)?;
        match (previous.as_ref(), named.as_ref()) {
            (None, None) => {}
            (Some(previous), Some(file)) => {
                private_file(file, false)?;
                let metadata = file.metadata().map_err(|_| StoreError::Io)?;
                if (metadata.dev(), metadata.ino(), metadata.len())
                    != (previous.identity.0, previous.identity.1, previous.len)
                    || record_bytes(file)? != previous.bytes
                {
                    return Err(StoreError::UnsafeFile);
                }
            }
            _ => return Err(StoreError::UnsafeFile),
        }
        if let Some(file) = named {
            let existing_bytes = record_bytes(&file)?;
            if existing_bytes == bytes {
                self.directory.sync_all().map_err(|_| {
                    self.disabled = true;
                    StoreError::Io
                })?;
                return Ok(SaveOutcome::AlreadyDurable);
            }
        }
        let mut temp_identity = None;
        let result = (|| {
            let temp = File::from(
                openat(
                    &self.directory,
                    TEMP_NAME,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC
                        | OFlags::NONBLOCK,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(|_| StoreError::UnsafeFile)?,
            );
            private_file(&temp, true)?;
            let metadata = temp.metadata().map_err(|_| StoreError::Io)?;
            temp_identity = Some((metadata.dev(), metadata.ino()));
            let mut writer = &temp;
            writer.write_all(bytes).map_err(|_| StoreError::Io)?;
            temp.sync_all().map_err(|_| StoreError::Io)?;
            #[cfg(test)]
            match self.fault.take() {
                Some(FaultPoint::ReplaceTempWithSymlink) => {
                    fs::remove_file(self.root.join(TEMP_NAME)).map_err(|_| StoreError::Io)?;
                    std::os::unix::fs::symlink(REGISTRY_NAME, self.root.join(TEMP_NAME))
                        .map_err(|_| StoreError::Io)?;
                }
                Some(FaultPoint::HardlinkTemp) => {
                    fs::hard_link(
                        self.root.join(TEMP_NAME),
                        self.root.join("workspace-index.v1.tmp-alias"),
                    )
                    .map_err(|_| StoreError::Io)?;
                }
                other => self.fault = other,
            }
            self.verify_identity()?;
            let named_now = open_named(&self.directory, REGISTRY_NAME)?;
            match (previous.as_ref(), named_now.as_ref()) {
                (None, None) => {}
                (Some(previous), Some(file)) => {
                    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
                    if (metadata.dev(), metadata.ino(), metadata.len())
                        != (previous.identity.0, previous.identity.1, previous.len)
                        || record_bytes(file)? != previous.bytes
                    {
                        return Err(StoreError::UnsafeFile);
                    }
                }
                _ => return Err(StoreError::UnsafeFile),
            }
            verify_named_temp(&self.directory, temp_identity.ok_or(StoreError::Io)?, bytes)?;
            #[cfg(test)]
            if self.fault == Some(FaultPoint::BeforeRename) {
                self.fault = None;
                return Err(StoreError::Io);
            }
            renameat(&self.directory, TEMP_NAME, &self.directory, REGISTRY_NAME)
                .map_err(|_| StoreError::Io)?;
            #[cfg(test)]
            if self.fault == Some(FaultPoint::AfterRename) {
                self.fault = None;
                self.disabled = true;
                return Err(StoreError::Io);
            }
            self.directory.sync_all().map_err(|_| {
                self.disabled = true;
                StoreError::Io
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(identity) = temp_identity {
                cleanup_owned_temp(&self.directory, identity);
            }
            return Err(error);
        }
        Ok(SaveOutcome::Written)
    }
}

fn cleanup_owned_temp(directory: &File, identity: (u64, u64)) {
    cleanup_owned_named_temp(directory, TEMP_NAME, identity);
}

/// Remove `name` only when it is still the exact private temp created by the
/// caller (`identity`); never removes a file another writer replaced it with.
pub(super) fn cleanup_owned_named_temp(directory: &File, name: &str, identity: (u64, u64)) {
    let Ok(Some(file)) = open_named(directory, name) else {
        return;
    };
    if private_file(&file, true).is_ok() {
        if let Ok(metadata) = file.metadata() {
            if (metadata.dev(), metadata.ino()) == identity {
                let _ = unlinkat(directory, name, AtFlags::empty());
            }
        }
    }
}

fn verify_named_temp(
    directory: &File,
    identity: (u64, u64),
    expected_bytes: &[u8],
) -> Result<(), StoreError> {
    let named = open_named(directory, TEMP_NAME)?.ok_or(StoreError::UnsafeFile)?;
    private_file(&named, false)?;
    let metadata = named.metadata().map_err(|_| StoreError::Io)?;
    if (metadata.dev(), metadata.ino()) != identity || record_bytes(&named)? != expected_bytes {
        return Err(StoreError::UnsafeFile);
    }
    Ok(())
}

fn validate_transition(old: &MetadataRecord, new: &MetadataRecord) -> Result<(), StoreError> {
    if new.next_id() < old.next_id() {
        return Err(StoreError::InvalidTransition);
    }
    let mut old_projects = Vec::new();
    let mut old_tabs = Vec::new();
    for project in old.index().projects() {
        old_projects.push(project.id().get());
        for tab in project.tabs() {
            old_tabs.push((tab.id.get(), project.id().get()));
        }
    }
    for project in new.index().projects() {
        let project_id = project.id().get();
        if !old_projects.contains(&project_id) && project_id < old.next_id() {
            return Err(StoreError::InvalidTransition);
        }
        for tab in project.tabs() {
            let tab_id = tab.id.get();
            match old_tabs.iter().find(|(old_id, _)| *old_id == tab_id) {
                Some((_, old_project_id)) if *old_project_id != project_id => {
                    return Err(StoreError::InvalidTransition)
                }
                Some(_) => {}
                None if tab_id < old.next_id() => return Err(StoreError::InvalidTransition),
                None => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "panelink-workspace-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn atomic_before_and_after_rename_faults_have_distinct_recovery() {
        let root = Root::new();
        let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
        let initial = store.initialize_legacy().unwrap();
        assert_eq!(initial, SaveOutcome::Written);
        let old = store.load().unwrap().unwrap();
        let mut index = old.index().clone();
        index
            .rename_project(ProjectId::new(1).unwrap(), "Renamed")
            .unwrap();
        let next = MetadataRecord::new(index, 2, 2).unwrap();

        store.fault = Some(FaultPoint::BeforeRename);
        assert_eq!(store.save(1, &next), Err(StoreError::Io));
        assert_eq!(store.load().unwrap().unwrap(), old);
        assert!(!root.0.join(TEMP_NAME).exists());

        store.fault = Some(FaultPoint::AfterRename);
        assert_eq!(store.save(1, &next), Err(StoreError::Io));
        assert_eq!(store.load().unwrap().unwrap(), next);
        assert_eq!(store.save(1, &next), Err(StoreError::NeedsReopen));
    }

    #[test]
    fn replaced_temporary_symlink_and_hardlink_are_not_published() {
        for fault in [FaultPoint::ReplaceTempWithSymlink, FaultPoint::HardlinkTemp] {
            let root = Root::new();
            let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
            store.initialize_legacy().unwrap();
            let old = store.load().unwrap().unwrap();
            let mut index = old.index().clone();
            index
                .rename_project(ProjectId::new(1).unwrap(), "Renamed")
                .unwrap();
            let next = MetadataRecord::new(index, 2, 2).unwrap();

            store.fault = Some(fault);
            assert_eq!(store.save(1, &next), Err(StoreError::UnsafeFile));
            assert_eq!(store.load().unwrap().unwrap(), old);
            assert!(root.0.join(REGISTRY_NAME).is_file());

            // A substituted object is not ours to clean up. Remove test debris
            // explicitly after confirming it was never renamed over the record.
            let temp = root.0.join(TEMP_NAME);
            if temp.exists() || fs::symlink_metadata(&temp).is_ok() {
                fs::remove_file(temp).unwrap();
            }
            let alias = root.0.join("workspace-index.v1.tmp-alias");
            if alias.exists() {
                fs::remove_file(alias).unwrap();
            }
        }
    }
}
