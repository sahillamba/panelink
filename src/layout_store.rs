//! Two-slot saved-layout store for historical layout snapshots.
//!
//! Holds at most `current.layout` and `previous.layout` in a private directory.
//! A rotation archives the exact current bytes into previous before writing a
//! new current. The caller supplies singleton exclusion for save/cleanup.
//!
//! See `docs/progress/P5-SAVED-LAYOUT.md` for the durability contract.

use crate::saved_layout::{decode, encode, SavedLayout, SAVED_LAYOUT_BYTES};
use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Mutex;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

const CURRENT_NAME: &str = "current.layout";
const PREVIOUS_NAME: &str = "previous.layout";
const MAX_DIR_ENTRIES: usize = 128;
const MAX_RECOGNIZED_TEMPS: usize = 16;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestFaultPoint {
    BeforeArchiveRename,
    AfterArchiveBeforeCurrentWrite,
    BeforeCurrentRename,
    DirectorySync,
}

#[cfg(test)]
struct TestFault {
    point: TestFaultPoint,
    failures_remaining: usize,
}

/// Identifies which slot to read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutSlot {
    /// The most recent saved layout for the current generation.
    Current,
    /// The immediately preceding generation archived from current.
    Previous,
}

/// Result of a successful save.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    /// New bytes were written and committed.
    Written,
    /// The requested bytes were already durable (or identical to current).
    AlreadyDurable,
}

/// Private directory owned by the current user, with two atomic slots.
pub struct LayoutStore {
    directory: File,
    path: PathBuf,
    #[cfg(test)]
    test_fault: Mutex<Option<TestFault>>,
}

impl std::fmt::Debug for LayoutStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LayoutStore { <redacted> }")
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid saved layout")
}

fn unsafe_storage() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "unsafe layout storage")
}

fn layout_save_refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "layout save refused")
}

fn block_decode(data: &[u8]) -> io::Result<SavedLayout> {
    decode(data).map_err(|_| invalid())
}

fn slot_file_name(slot: LayoutSlot) -> &'static str {
    match slot {
        LayoutSlot::Current => CURRENT_NAME,
        LayoutSlot::Previous => PREVIOUS_NAME,
    }
}

fn validate_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(unsafe_storage());
    }
    Ok(())
}

fn canonical_nonzero_u32(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parsed = value.parse::<u32>().ok()?;
    (parsed != 0 && parsed.to_string() == value).then_some(parsed)
}

fn canonical_nonzero_u64(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parsed = value.parse::<u64>().ok()?;
    (parsed != 0 && parsed.to_string() == value).then_some(parsed)
}

/// Return true if this is a recognized layout temporary name.
fn is_temporary_slot_name(value: &str) -> bool {
    let value = match value.strip_suffix(".tmp") {
        Some(v) => v,
        None => return false,
    };
    let (value, counter) = match value.rsplit_once('-') {
        Some(v) => v,
        None => return false,
    };
    if canonical_nonzero_u64(counter).is_none() {
        return false;
    }
    let (dot_final_name, pid) = match value.rsplit_once('-') {
        Some(v) => v,
        None => return false,
    };
    if canonical_nonzero_u32(pid).is_none() {
        return false;
    }
    let final_name = match dot_final_name.strip_prefix('.') {
        Some(v) => v,
        None => return false,
    };
    matches!(final_name, CURRENT_NAME | PREVIOUS_NAME)
}

impl LayoutStore {
    /// Open an existing private directory, refusing symlinks and unsafe parents.
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() || fs::canonicalize(path)? != path {
            return Err(unsafe_storage());
        }
        let uid = rustix::process::geteuid().as_raw();
        let mut parent = PathBuf::new();
        for component in path.components() {
            parent.push(component.as_os_str());
            let metadata = fs::symlink_metadata(&parent)?;
            if !metadata.is_dir()
                || (metadata.uid() != uid && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0
                    && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0))
            {
                return Err(unsafe_storage());
            }
        }
        let directory = File::from(rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?);
        let metadata = directory.metadata()?;
        let named = fs::symlink_metadata(path)?;
        if metadata.uid() != uid
            || metadata.mode() & 0o7777 != 0o700
            || (metadata.dev(), metadata.ino()) != (named.dev(), named.ino())
        {
            return Err(unsafe_storage());
        }
        Ok(Self {
            directory,
            path: path.to_owned(),
            #[cfg(test)]
            test_fault: Mutex::new(None),
        })
    }

    /// Create the layout directory inside `server_directory` if absent, then open it.
    pub fn prepare(server_directory: &Path) -> io::Result<Self> {
        let layout = server_directory.join("layout");
        if !layout.exists() {
            fs::DirBuilder::new().mode(0o700).create(&layout)?;
        }
        Self::open(&layout)
    }

    /// The canonical path of the held directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check that the held directory still matches the named path.
    pub fn verify_identity(&self) -> io::Result<()> {
        let named = Self::open(&self.path)?;
        let old = self.directory.metadata()?;
        let new = named.directory.metadata()?;
        if (old.dev(), old.ino()) != (new.dev(), new.ino()) {
            return Err(unsafe_storage());
        }
        Ok(())
    }

    /// Check that this store belongs to the expected server directory.
    pub(crate) fn verify_server_directory(&self, server_directory: &Path) -> io::Result<()> {
        self.verify_identity()?;
        if self.path != server_directory.join("layout") {
            return Err(unsafe_storage());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_test_fault(&self, point: TestFaultPoint, failures: usize) {
        *self.test_fault.lock().expect("test fault mutex") = (failures != 0).then_some(TestFault {
            point,
            failures_remaining: failures,
        });
    }

    #[cfg(test)]
    fn maybe_fail_for_test(&self, point: TestFaultPoint) -> io::Result<()> {
        let mut configured = self.test_fault.lock().expect("test fault mutex");
        let Some(fault) = configured
            .as_mut()
            .filter(|fault| fault.point == point && fault.failures_remaining != 0)
        else {
            return Ok(());
        };
        fault.failures_remaining -= 1;
        if fault.failures_remaining == 0 {
            *configured = None;
        }
        Err(io::Error::other("injected layout durability failure"))
    }

    fn before_archive_rename(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::BeforeArchiveRename)?;
        Ok(())
    }

    fn after_archive_before_current_write(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::AfterArchiveBeforeCurrentWrite)?;
        Ok(())
    }

    fn before_current_rename(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::BeforeCurrentRename)?;
        Ok(())
    }

    fn sync_directory(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::DirectorySync)?;
        self.directory.sync_all()
    }

    /// Read verified bytes for a slot, if present.
    fn read_slot(&self, slot: LayoutSlot) -> io::Result<Option<[u8; SAVED_LAYOUT_BYTES]>> {
        // A concurrent atomic rename can leave a just-opened descriptor on the
        // replaced, unlinked inode (nlink 0); reopen boundedly rather than
        // misreport tampering.
        let mut opened = None;
        for _ in 0..4 {
            let file = match openat(
                &self.directory,
                slot_file_name(slot),
                // NONBLOCK: a FIFO planted at a slot name must fail
                // validation, not hang a read-only inspection.
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) {
                Ok(file) => File::from(file),
                Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                Err(error) => return Err(io::Error::from(error)),
            };
            if file.metadata()?.nlink() != 0 {
                opened = Some(file);
                break;
            }
        }
        let file = opened.ok_or_else(unsafe_storage)?;
        validate_file(&file)?;
        if file.metadata()?.len() != SAVED_LAYOUT_BYTES as u64 {
            return Err(invalid());
        }
        let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
        file.take(SAVED_LAYOUT_BYTES as u64)
            .read_exact(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Load and validate a saved layout slot. Invalid slots yield a sanitized error,
    /// but an invalid current does not block loading a valid previous.
    pub fn load(&self, slot: LayoutSlot) -> io::Result<Option<SavedLayout>> {
        let Some(bytes) = self.read_slot(slot)? else {
            return Ok(None);
        };
        let layout = block_decode(&bytes)?;
        Ok(Some(layout))
    }

    /// Load the raw bytes for a slot without decoding.
    pub fn load_bytes(&self, slot: LayoutSlot) -> io::Result<Option<[u8; SAVED_LAYOUT_BYTES]>> {
        self.read_slot(slot)
    }

    /// Save a validated candidate under singleton caller exclusion.
    ///
    /// Preflight validation checks both slots before any mutation. Rotation preserves
    /// exact current bytes in previous. Same-key/identical bytes is a no-op sync.
    pub fn save(
        &self,
        bound_generation: u64,
        bound_workspace: u64,
        candidate: &SavedLayout,
    ) -> io::Result<SaveOutcome> {
        self.verify_identity()?;
        if bound_generation == 0 || bound_workspace == 0 {
            return Err(layout_save_refused());
        }
        let candidate_meta = candidate.metadata();
        if candidate_meta.generation() != bound_generation
            || candidate_meta.workspace() != bound_workspace
        {
            return Err(layout_save_refused());
        }

        let current_bytes = self.read_slot(LayoutSlot::Current)?;
        let previous_bytes = self.read_slot(LayoutSlot::Previous)?;

        let current_layout = current_bytes
            .as_ref()
            .map(|bytes| block_decode(bytes))
            .transpose()?;
        let previous_layout = previous_bytes
            .as_ref()
            .map(|bytes| block_decode(bytes))
            .transpose()?;

        // Validate existing slots.
        if let Some(ref layout) = current_layout {
            if layout.metadata().workspace() != bound_workspace
                || layout.metadata().generation() > bound_generation
            {
                return Err(layout_save_refused());
            }
        }
        if let Some(ref layout) = previous_layout {
            if layout.metadata().workspace() != bound_workspace
                || layout.metadata().generation() > bound_generation
            {
                return Err(layout_save_refused());
            }
        }
        if current_bytes.is_none() && previous_bytes.is_some() {
            return Err(layout_save_refused());
        }
        if let (Some(ref current), Some(ref previous)) = (&current_layout, &previous_layout) {
            let current_gen = current.metadata().generation();
            let previous_gen = previous.metadata().generation();
            let current_bytes_val = current_bytes.as_ref().unwrap();
            let previous_bytes_val = previous_bytes.as_ref().unwrap();
            if previous_gen >= current_gen && previous_bytes_val != current_bytes_val {
                return Err(layout_save_refused());
            }
        }

        let candidate_bytes = encode(candidate);

        // Both absent: write current.
        if current_bytes.is_none() && previous_bytes.is_none() {
            return self.atomic_write(LayoutSlot::Current, &candidate_bytes, &None);
        }

        let current_layout = current_layout.unwrap();
        let current_gen = current_layout.metadata().generation();
        let current_seq = current_layout.metadata().snapshot_sequence();
        let current_rev = current_layout.metadata().layout_revision();
        let current_pane_count = current_layout.pane_count();
        let candidate_gen = candidate_meta.generation();
        let candidate_seq = candidate_meta.snapshot_sequence();
        let candidate_rev = candidate_meta.layout_revision();

        if candidate_gen > current_gen {
            // Rotation: archive current to previous.
            let current_bytes_ref = current_bytes.as_ref().unwrap();
            if previous_bytes.as_ref() != Some(current_bytes_ref) {
                self.atomic_write(LayoutSlot::Previous, current_bytes_ref, &previous_bytes)?;
            } else {
                self.verify_identity()?;
                self.sync_directory()?;
            }
            self.after_archive_before_current_write()?;
            self.atomic_write(LayoutSlot::Current, &candidate_bytes, &current_bytes)
        } else if candidate_gen == current_gen {
            if candidate_seq == current_seq {
                if &candidate_bytes == current_bytes.as_ref().unwrap() {
                    self.verify_identity()?;
                    self.sync_directory()?;
                    Ok(SaveOutcome::AlreadyDurable)
                } else {
                    Err(layout_save_refused())
                }
            } else if candidate_seq < current_seq {
                Err(layout_save_refused())
            } else {
                // candidate_seq > current_seq
                if current_pane_count == 0 {
                    return Err(layout_save_refused());
                }
                if candidate.pane_count() > 0 && candidate_rev <= current_rev {
                    return Err(layout_save_refused());
                }
                if candidate.pane_count() == 0 && candidate_rev < current_rev {
                    return Err(layout_save_refused());
                }
                self.atomic_write(LayoutSlot::Current, &candidate_bytes, &current_bytes)
            }
        } else {
            Err(layout_save_refused())
        }
    }

    fn atomic_write(
        &self,
        slot: LayoutSlot,
        bytes: &[u8; SAVED_LAYOUT_BYTES],
        preflight_old: &Option<[u8; SAVED_LAYOUT_BYTES]>,
    ) -> io::Result<SaveOutcome> {
        self.verify_identity()?;
        let filename = slot_file_name(slot);
        let temporary = format!(
            ".{filename}-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let result = (|| {
            let mut file = File::from(openat(
                &self.directory,
                temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            validate_file(&file)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            rustix::fs::fcntl_fullfsync(&file)?;

            // Re-read destination and require it still equals preflight bytes (or still absent).
            let current_on_disk = self.read_slot(slot)?;
            match (preflight_old, current_on_disk) {
                (None, None) => {}
                (Some(expected), Some(actual)) if expected == &actual => {}
                (Some(_expected), None) => {
                    // Destination disappeared between preflight and rename; refuse.
                    return Err(layout_save_refused());
                }
                (None, Some(_)) => {
                    // Destination appeared where none was expected; refuse.
                    return Err(layout_save_refused());
                }
                (Some(expected), Some(actual)) if expected != &actual => {
                    // Destination changed; byte-identical retry should match.
                    if actual == *bytes {
                        self.sync_directory()?;
                        return Ok(SaveOutcome::AlreadyDurable);
                    }
                    return Err(layout_save_refused());
                }
                _ => unreachable!(),
            }

            match slot {
                LayoutSlot::Previous => self.before_archive_rename()?,
                LayoutSlot::Current => self.before_current_rename()?,
            }
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                filename,
            )?;
            self.sync_directory()?;
            Ok(SaveOutcome::Written)
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }

    /// Remove only recognized safe temporary files. The caller must exclude active writers.
    pub fn cleanup_temporary_files(&self) -> io::Result<usize> {
        self.verify_identity()?;
        let mut total_entries = 0_usize;
        let mut named_candidates = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            total_entries += 1;
            if total_entries > MAX_DIR_ENTRIES {
                return Err(invalid());
            }
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if is_temporary_slot_name(file_name) {
                if named_candidates.len() == MAX_RECOGNIZED_TEMPS {
                    return Err(invalid());
                }
                named_candidates.push(file_name.to_owned());
            }
        }

        self.verify_identity()?;
        let mut candidates = Vec::new();
        for name in named_candidates {
            let file = File::from(openat(
                &self.directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o7777 != 0o600
                || metadata.nlink() != 1
                || metadata.len() > SAVED_LAYOUT_BYTES as u64
            {
                return Err(unsafe_storage());
            }
            candidates.push((name, metadata.dev(), metadata.ino(), metadata.len()));
        }

        // Preflight revalidation.
        for (name, ..) in &candidates {
            self.verify_identity()?;
            let file = File::from(openat(
                &self.directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o7777 != 0o600
                || metadata.nlink() != 1
                || metadata.len() > SAVED_LAYOUT_BYTES as u64
            {
                return Err(unsafe_storage());
            }
        }

        let mut removed = 0;
        for (name, dev, inode, len) in &candidates {
            self.verify_identity()?;
            let file = File::from(openat(
                &self.directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            let metadata = file.metadata()?;
            if metadata.dev() != *dev
                || metadata.ino() != *inode
                || metadata.len() != *len
                || !metadata.is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o7777 != 0o600
                || metadata.nlink() != 1
            {
                return Err(unsafe_storage());
            }
            drop(file);
            unlinkat(&self.directory, name.as_str(), AtFlags::empty())?;
            removed += 1;
        }

        self.verify_identity()?;
        self.sync_directory()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::TargetRef;
    use crate::pane_layout::{LayoutSnapshot, PaneGeometry, Rect, MIN_PANE_COLUMNS, MIN_PANE_ROWS};
    use crate::saved_layout::LayoutMetadata;
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after Unix epoch")
                .as_nanos();
            for suffix in 0..32 {
                let path = std::env::temp_dir().join(format!(
                    "panelink-layout-unit-{}-{}-{suffix}",
                    std::process::id(),
                    stamp
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(fs::canonicalize(path).unwrap()),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create private layout fixture: {error}"),
                }
            }
            panic!("could not allocate a unique private layout fixture")
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn target_for(generation: u64, workspace: u64, pane: u64, session: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace,
            pane,
            session,
            occupant_revision: 1,
        }
    }

    fn make_metadata(
        generation: u64,
        workspace: u64,
        sequence: u64,
        revision: u64,
    ) -> LayoutMetadata {
        LayoutMetadata::new(generation, workspace, sequence, revision, 1_000_001)
            .expect("valid test metadata")
    }

    fn snapshot_one(rows: u16, columns: u16, revision: u64, gen: u64, ws: u64) -> LayoutSnapshot {
        LayoutSnapshot {
            revision,
            rows,
            columns,
            panes: [
                Some(PaneGeometry {
                    target: target_for(gen, ws, 2, 3),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns,
                    },
                }),
                None,
            ],
            divider: None,
        }
    }

    fn snapshot_two(rows: u16, columns: u16, revision: u64, gen: u64, ws: u64) -> LayoutSnapshot {
        let usable_cols = columns - 1;
        let first_cols = usable_cols / 2;
        let second_cols = usable_cols - first_cols;
        LayoutSnapshot {
            revision,
            rows,
            columns,
            panes: [
                Some(PaneGeometry {
                    target: target_for(gen, ws, 2, 3),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns: first_cols,
                    },
                }),
                Some(PaneGeometry {
                    target: target_for(gen, ws, 4, 5),
                    rect: Rect {
                        x: first_cols + 1,
                        y: 0,
                        rows,
                        columns: second_cols,
                    },
                }),
            ],
            divider: Some(Rect {
                x: first_cols,
                y: 0,
                rows,
                columns: 1,
            }),
        }
    }

    fn saved_one(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
        let meta = make_metadata(generation, workspace, sequence, revision);
        SavedLayout::from_snapshot(
            meta,
            &snapshot_one(
                MIN_PANE_ROWS + 1,
                MIN_PANE_COLUMNS + 5,
                revision,
                generation,
                workspace,
            ),
        )
        .expect("valid one-pane saved layout")
    }

    fn saved_empty(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
        let meta = make_metadata(generation, workspace, sequence, revision);
        SavedLayout::empty(meta)
    }

    #[test]
    fn first_save_writes_current_without_previous() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let candidate = saved_one(7, 1, 1, 1);
        assert_eq!(store.save(7, 1, &candidate).unwrap(), SaveOutcome::Written);
        let loaded = store.load(LayoutSlot::Current).unwrap().unwrap();
        assert_eq!(loaded.metadata().snapshot_sequence(), 1);
        assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
    }

    #[test]
    fn same_generation_higher_sequence_writes_current_only() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        let second = saved_one(7, 1, 2, 2);
        assert_eq!(store.save(7, 1, &second).unwrap(), SaveOutcome::Written);
        assert_eq!(
            store
                .load(LayoutSlot::Current)
                .unwrap()
                .unwrap()
                .metadata()
                .snapshot_sequence(),
            2
        );
        assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
    }

    #[test]
    fn same_generation_equal_sequence_identical_is_already_durable() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        assert_eq!(
            store.save(7, 1, &first).unwrap(),
            SaveOutcome::AlreadyDurable
        );
    }

    #[test]
    fn same_generation_equal_sequence_different_bytes_refused() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        // Save a one-pane layout with specific dimensions.
        let meta = make_metadata(7, 1, 1, 1);
        let first = SavedLayout::from_snapshot(
            meta,
            &snapshot_one(MIN_PANE_ROWS + 1, MIN_PANE_COLUMNS + 5, 1, 7, 1),
        )
        .unwrap();
        store.save(7, 1, &first).unwrap();
        // Same metadata but different dimensions -> different valid bytes, same seq.
        let second = SavedLayout::from_snapshot(
            meta,
            &snapshot_one(MIN_PANE_ROWS + 2, MIN_PANE_COLUMNS + 5, 1, 7, 1),
        )
        .unwrap();
        assert!(store.save(7, 1, &second).is_err());
    }

    #[test]
    fn lower_sequence_refused() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 2, 2);
        store.save(7, 1, &first).unwrap();
        let second = saved_one(7, 1, 1, 1);
        assert!(store.save(7, 1, &second).is_err());
    }

    #[test]
    fn rotation_archives_exact_current_then_writes_new() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        let second = saved_one(8, 1, 1, 1);
        assert_eq!(store.save(8, 1, &second).unwrap(), SaveOutcome::Written);
        let previous = store.load(LayoutSlot::Previous).unwrap().unwrap();
        assert_eq!(previous.metadata().generation(), 7);
        let current = store.load(LayoutSlot::Current).unwrap().unwrap();
        assert_eq!(current.metadata().generation(), 8);
    }

    #[test]
    fn second_rotation_replaces_previous() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
        store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
        store.save(9, 1, &saved_one(9, 1, 1, 1)).unwrap();
        assert_eq!(
            store
                .load(LayoutSlot::Previous)
                .unwrap()
                .unwrap()
                .metadata()
                .generation(),
            8
        );
        assert_eq!(
            store
                .load(LayoutSlot::Current)
                .unwrap()
                .unwrap()
                .metadata()
                .generation(),
            9
        );
    }

    #[test]
    fn byte_identical_rotation_retry_skips_double_archive() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let gen7 = saved_one(7, 1, 1, 1);
        store.save(7, 1, &gen7).unwrap();
        store.inject_test_fault(TestFaultPoint::BeforeCurrentRename, 1);
        let gen8 = saved_one(8, 1, 1, 1);
        assert!(store.save(8, 1, &gen8).is_err());
        // Retry succeeds without re-archiving
        assert_eq!(store.save(8, 1, &gen8).unwrap(), SaveOutcome::Written);
        let previous = store.load(LayoutSlot::Previous).unwrap().unwrap();
        assert_eq!(previous.metadata().generation(), 7);
        let current = store.load(LayoutSlot::Current).unwrap().unwrap();
        assert_eq!(current.metadata().generation(), 8);
    }

    #[test]
    fn crash_after_archive_leaves_valid_previous_and_retry_completes() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let gen7 = saved_one(7, 1, 1, 1);
        store.save(7, 1, &gen7).unwrap();
        store.inject_test_fault(TestFaultPoint::AfterArchiveBeforeCurrentWrite, 1);
        let gen8 = saved_one(8, 1, 1, 1);
        assert!(store.save(8, 1, &gen8).is_err());
        // Previous already written; current still gen7
        let previous = store.load(LayoutSlot::Previous).unwrap().unwrap();
        assert_eq!(previous.metadata().generation(), 7);
        let current = store.load(LayoutSlot::Current).unwrap().unwrap();
        assert_eq!(current.metadata().generation(), 7);
        // Retry completes without re-archiving
        assert_eq!(store.save(8, 1, &gen8).unwrap(), SaveOutcome::Written);
        assert_eq!(
            store
                .load(LayoutSlot::Previous)
                .unwrap()
                .unwrap()
                .metadata()
                .generation(),
            7
        );
        assert_eq!(
            store
                .load(LayoutSlot::Current)
                .unwrap()
                .unwrap()
                .metadata()
                .generation(),
            8
        );
    }

    #[test]
    fn current_unchanged_after_pre_rename_failure() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        store.inject_test_fault(TestFaultPoint::BeforeCurrentRename, 1);
        let second = saved_one(7, 1, 2, 2);
        assert!(store.save(7, 1, &second).is_err());
        let current = store.load(LayoutSlot::Current).unwrap().unwrap();
        assert_eq!(current.metadata().snapshot_sequence(), 1);
    }

    #[test]
    fn uncertain_current_rename_retry_syncs_without_touching_previous() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        let second = saved_one(7, 1, 2, 2);
        assert!(store.save(7, 1, &second).is_err());
        // After uncertain rename, observe new bytes on disk.
        let observed = LayoutStore::open(fixture.path()).unwrap();
        assert_eq!(
            observed
                .load(LayoutSlot::Current)
                .unwrap()
                .unwrap()
                .metadata()
                .snapshot_sequence(),
            2
        );
        assert!(store.save(7, 1, &second).is_err());
        assert_eq!(
            store.save(7, 1, &second).unwrap(),
            SaveOutcome::AlreadyDurable
        );
        assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
    }

    #[test]
    fn empty_current_blocks_further_nonempty() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let nonempty = saved_one(7, 1, 1, 1);
        store.save(7, 1, &nonempty).unwrap();
        let empty = saved_empty(7, 1, 2, 2);
        store.save(7, 1, &empty).unwrap();
        let later_nonempty = saved_one(7, 1, 3, 3);
        assert!(store.save(7, 1, &later_nonempty).is_err());
    }

    #[test]
    fn empty_current_blocks_further_empty() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let nonempty = saved_one(7, 1, 1, 1);
        store.save(7, 1, &nonempty).unwrap();
        let empty1 = saved_empty(7, 1, 2, 2);
        store.save(7, 1, &empty1).unwrap();
        let empty2 = saved_empty(7, 1, 3, 3);
        assert!(store.save(7, 1, &empty2).is_err());
    }

    #[test]
    fn nonempty_equal_revision_refused() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        // same generation, same revision, different sequence -> candidate must be empty
        // or have strictly greater revision (if nonempty)
        let second = saved_one(7, 1, 2, 1);
        // Actually, for nonempty, revision must be strictly greater.
        // This one has equal revision and is nonempty -> refused
        assert!(store.save(7, 1, &second).is_err());
    }

    #[test]
    fn nonempty_to_empty_equal_revision_accepted() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        // Create nonempty with rev 1
        let snap = snapshot_two(MIN_PANE_ROWS, MIN_PANE_COLUMNS + 21, 1, 7, 1);
        let meta = make_metadata(7, 1, 1, 1);
        let nonempty = SavedLayout::from_snapshot(meta, &snap).unwrap();
        store.save(7, 1, &nonempty).unwrap();
        // Empty with equal revision accepted
        let empty = saved_empty(7, 1, 2, 1);
        assert_eq!(store.save(7, 1, &empty).unwrap(), SaveOutcome::Written);
    }

    #[test]
    fn wrong_generation_or_workspace_candidate_refused() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let candidate = saved_one(7, 1, 1, 1);
        assert!(store.save(8, 1, &candidate).is_err());
        assert!(store.save(7, 2, &candidate).is_err());
    }

    #[test]
    fn missing_current_with_present_previous_refused_for_save() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let gen7 = saved_one(7, 1, 1, 1);
        store.save(7, 1, &gen7).unwrap();
        let gen8 = saved_one(8, 1, 1, 1);
        store.save(8, 1, &gen8).unwrap();
        // Delete current manually
        fs::remove_file(fixture.path().join(CURRENT_NAME)).unwrap();
        let observed = LayoutStore::open(fixture.path()).unwrap();
        assert!(observed.load(LayoutSlot::Previous).unwrap().is_some());
        assert!(observed.load(LayoutSlot::Current).unwrap().is_none());
        assert!(observed.save(9, 1, &saved_one(9, 1, 1, 1)).is_err());
    }

    #[test]
    fn newer_previous_same_generation_different_bytes_refused() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
        let gen8_first = saved_one(8, 1, 1, 1);
        store.save(8, 1, &gen8_first).unwrap();
        // Corrupt previous to have same generation as current but different bytes
        let bytes = encode(&saved_one(8, 1, 2, 2));
        fs::write(fixture.path().join(PREVIOUS_NAME), bytes).unwrap();
        fs::set_permissions(
            fixture.path().join(PREVIOUS_NAME),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let observed = LayoutStore::open(fixture.path()).unwrap();
        assert!(observed.save(9, 1, &saved_one(9, 1, 1, 1)).is_err());
    }

    #[test]
    fn corrupt_or_short_current_refused_and_preserved() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let first = saved_one(7, 1, 1, 1);
        store.save(7, 1, &first).unwrap();
        let path = fixture.path().join(CURRENT_NAME);
        let original = fs::read(&path).unwrap();
        // Short
        fs::write(&path, &original[..100]).unwrap();
        assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
        assert_eq!(fs::read(&path).unwrap().len(), 100);
        // Restore and corrupt
        fs::write(&path, &original).unwrap();
        fs::write(&path, b"not-valid-layout-record-here").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not-valid-layout-record-here");
    }

    #[test]
    fn invalid_current_does_not_block_load_previous() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
        let gen8 = saved_one(8, 1, 1, 1);
        store.save(8, 1, &gen8).unwrap();
        // Corrupt current
        fs::write(fixture.path().join(CURRENT_NAME), b"bad").unwrap();
        fs::set_permissions(
            fixture.path().join(CURRENT_NAME),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let observed = LayoutStore::open(fixture.path()).unwrap();
        assert!(observed.load(LayoutSlot::Current).is_err());
        assert_eq!(
            observed
                .load(LayoutSlot::Previous)
                .unwrap()
                .unwrap()
                .metadata()
                .generation(),
            7
        );
    }

    #[test]
    fn load_never_creates_files() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let before_count = fs::read_dir(fixture.path()).unwrap().count();
        assert!(store.load(LayoutSlot::Current).unwrap().is_none());
        assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), before_count);
    }

    #[test]
    fn cleanup_removes_recognized_temps_and_preserves_committed() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let candidate = saved_one(7, 1, 1, 1);
        store.save(7, 1, &candidate).unwrap();

        // Create a fake temp
        fs::write(
            fixture.path().join(".current.layout-12345-1.tmp"),
            encode(&candidate),
        )
        .unwrap();
        fs::set_permissions(
            fixture.path().join(".current.layout-12345-1.tmp"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        // Create an unknown name
        fs::write(fixture.path().join("unknown-file"), b"hello").unwrap();
        fs::set_permissions(
            fixture.path().join("unknown-file"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        assert_eq!(store.cleanup_temporary_files().unwrap(), 1);
        assert!(store.load(LayoutSlot::Current).unwrap().is_some());
        assert!(fixture.path().join("unknown-file").exists());
        assert!(!fixture.path().join(".current.layout-12345-1.tmp").exists());
    }

    #[test]
    fn cleanup_refuses_unsafe_temp_and_deletes_nothing() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let candidate = saved_one(7, 1, 1, 1);
        store.save(7, 1, &candidate).unwrap();

        // Safe temp
        fs::write(
            fixture.path().join(".current.layout-12345-1.tmp"),
            encode(&candidate),
        )
        .unwrap();
        fs::set_permissions(
            fixture.path().join(".current.layout-12345-1.tmp"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        // Unsafe temp (wrong mode)
        fs::write(
            fixture.path().join(".previous.layout-12345-2.tmp"),
            encode(&candidate),
        )
        .unwrap();
        fs::set_permissions(
            fixture.path().join(".previous.layout-12345-2.tmp"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        assert!(store.cleanup_temporary_files().is_err());
        assert!(fixture.path().join(".current.layout-12345-1.tmp").exists());
        assert!(fixture.path().join(".previous.layout-12345-2.tmp").exists());
    }

    #[test]
    fn cleanup_refuses_too_many_recognized_temps() {
        let fixture = Fixture::new();
        let store = LayoutStore::open(fixture.path()).unwrap();
        let candidate = saved_one(7, 1, 1, 1);
        for index in 1..=MAX_RECOGNIZED_TEMPS + 1 {
            let name = format!(".current.layout-12345-{index}.tmp");
            fs::write(fixture.path().join(&name), encode(&candidate)).unwrap();
            fs::set_permissions(
                fixture.path().join(&name),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        assert!(store.cleanup_temporary_files().is_err());
    }

    #[test]
    fn prepare_creates_layout_directory() {
        let fixture = Fixture::new();
        let store = LayoutStore::prepare(fixture.path()).unwrap();
        assert_eq!(store.path(), fixture.path().join("layout"));
        assert!(fixture.path().join("layout").is_dir());
    }
}
