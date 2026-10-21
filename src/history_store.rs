//! Private, read-only-recovery storage for the latest visible screen of each
//! buffer. This stores normalized checkpoints, never terminal control streams.
//! Scrollback extraction, capture scheduling and layout restoration are separate.

use crate::control_protocol::TargetRef;
use crate::history_rows;
use crate::screen_checkpoint::{self, ScreenBuffer, ScreenCheckpoint};
use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Mutex;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const MAX_ROWS: usize = 1024;
const MAX_SEGMENT_BYTES: usize = 8 * 1024 * 1024;
const SEGMENT_HEADER_BYTES: usize = 72;
const SEGMENT_MAGIC: &[u8; 8] = b"PLHSEG02";
const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestFaultPoint {
    BeforeRename,
    DirectorySync,
}

#[cfg(test)]
struct TestFault {
    point: TestFaultPoint,
    failures_remaining: usize,
}

/// One validated read-only saved-history view. The caller may display/search
/// its sanitized rows, never replay them to a terminal or child process.
pub struct SavedHistory {
    /// True if capture skipped or evicted rows, or a reset occurred.
    pub gap: bool,
    /// True when the last writer reported storage failure and capture stopped.
    pub capture_disabled: bool,
    /// The current engine epoch represented by the retained rows.
    pub epoch: u64,
    /// Timestamp when this session ended; zero while live or not yet known.
    pub ended_at_unix_ms: u64,
    /// Validated bounded row records in ordinal order.
    pub records: Vec<Vec<u8>>,
}

/// A conservative cap on the directory scan for recorded session identities.
const MAX_HISTORY_TARGETS: usize = 4096;

impl std::fmt::Debug for SavedHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SavedHistory { <redacted> }")
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid screen history")
}

fn unsafe_storage() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "unsafe history storage")
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

fn name(target: TargetRef, buffer: ScreenBuffer) -> io::Result<String> {
    let fields = [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ];
    if fields.contains(&0) {
        return Err(invalid());
    }
    let suffix = match buffer {
        ScreenBuffer::Normal => 'n',
        ScreenBuffer::Alternate => 'a',
    };
    Ok(format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}-{suffix}.screen",
        fields[0], fields[1], fields[2], fields[3], fields[4]
    ))
}

fn history_name(target: TargetRef) -> io::Result<String> {
    let screen = name(target, ScreenBuffer::Normal)?;
    Ok(format!("{}.rows", screen.trim_end_matches("-n.screen")))
}

fn canonical_target_fields(value: &str) -> Option<TargetRef> {
    let mut parts = value.split('-');
    let mut fields = [0_u64; 5];
    for field in &mut fields {
        let part = parts.next()?;
        if part.len() != 16
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        *field = u64::from_str_radix(part, 16).ok()?;
        if *field == 0 {
            return None;
        }
    }
    if parts.next().is_some() {
        return None;
    }
    Some(TargetRef {
        generation: fields[0],
        workspace: fields[1],
        pane: fields[2],
        session: fields[3],
        occupant_revision: fields[4],
    })
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

/// Return the exact committed target name and its bound if this is a
/// writer-generated temporary name. Lookalikes are intentionally ignored.
fn temporary_final_name(value: &str) -> Option<(String, u64)> {
    let value = value.strip_suffix(".tmp")?;
    let (value, counter) = value.rsplit_once('-')?;
    canonical_nonzero_u64(counter)?;
    let (dot_final_name, pid) = value.rsplit_once('-')?;
    canonical_nonzero_u32(pid)?;
    let final_name = dot_final_name.strip_prefix('.')?;

    let (target_text, buffer, is_rows) = final_name
        .strip_suffix(".rows")
        .map(|target| (target, ScreenBuffer::Normal, true))
        .or_else(|| {
            final_name
                .strip_suffix("-n.screen")
                .map(|target| (target, ScreenBuffer::Normal, false))
        })
        .or_else(|| {
            final_name
                .strip_suffix("-a.screen")
                .map(|target| (target, ScreenBuffer::Alternate, false))
        })?;
    let target = canonical_target_fields(target_text)?;
    let reconstructed = if is_rows {
        history_name(target).ok()?
    } else {
        name(target, buffer).ok()?
    };
    if reconstructed != final_name {
        return None;
    }
    let bound = if is_rows {
        MAX_SEGMENT_BYTES
    } else {
        screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES
    };
    Some((reconstructed, bound as u64))
}

#[derive(Clone, Debug)]
struct TemporaryCandidate {
    name: String,
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    links: u64,
    length: u64,
    bound: u64,
}

impl TemporaryCandidate {
    fn from_file(name: String, bound: u64, file: &File) -> io::Result<Self> {
        validate_file(file)?;
        let metadata = file.metadata()?;
        if metadata.len() > bound {
            return Err(invalid());
        }
        Ok(Self {
            name,
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            mode: metadata.mode() & 0o7777,
            links: metadata.nlink(),
            length: metadata.len(),
            bound,
        })
    }

    fn revalidate(&self, file: &File) -> io::Result<()> {
        validate_file(file)?;
        let metadata = file.metadata()?;
        if metadata.len() > self.bound
            || (
                metadata.dev(),
                metadata.ino(),
                metadata.uid(),
                metadata.mode() & 0o7777,
                metadata.nlink(),
                metadata.len(),
            ) != (
                self.device,
                self.inode,
                self.uid,
                self.mode,
                self.links,
                self.length,
            )
        {
            return Err(unsafe_storage());
        }
        Ok(())
    }
}

/// A private directory owned by the current user, with one atomic slot for
/// each target/buffer. The caller supplies a created 0700 history directory.
pub struct HistoryStore {
    directory: File,
    path: PathBuf,
    #[cfg(test)]
    test_fault: Mutex<Option<TestFault>>,
}

impl std::fmt::Debug for HistoryStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HistoryStore { <redacted> }")
    }
}

impl HistoryStore {
    /// Check that the held history directory belongs to the locked server
    /// identity domain before transferring it to a background writer.
    pub(crate) fn verify_server_directory(&self, server_directory: &Path) -> io::Result<()> {
        self.verify_identity()?;
        if self.path != server_directory.join("history") {
            return Err(unsafe_storage());
        }
        Ok(())
    }

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
        Err(io::Error::other("injected history durability failure"))
    }

    fn before_rename(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::BeforeRename)?;
        Ok(())
    }

    fn sync_directory(&self) -> io::Result<()> {
        #[cfg(test)]
        self.maybe_fail_for_test(TestFaultPoint::DirectorySync)?;
        self.directory.sync_all()
    }

    /// Refuse writes when the named directory was replaced, deleted or made
    /// public after opening. The descriptor remains the IO authority.
    pub fn verify_identity(&self) -> io::Result<()> {
        let named = Self::open(&self.path)?;
        let old = self.directory.metadata()?;
        let new = named.directory.metadata()?;
        if (old.dev(), old.ino()) != (new.dev(), new.ino()) {
            return Err(unsafe_storage());
        }
        Ok(())
    }

    fn read_named(&self, filename: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        let file = match openat(
            &self.directory,
            filename,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(io::Error::from(error)),
        };
        validate_file(&file)?;
        if file.metadata()?.len() > max as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(max as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max {
            return Err(invalid());
        }
        Ok(Some(bytes))
    }

    /// Read exactly one target's bounded normal-history segment. It is not a
    /// live terminal replay or provider conversation.
    pub fn load_rows(&self, target: TargetRef) -> io::Result<Option<SavedHistory>> {
        let Some(bytes) = self.read_named(&history_name(target)?, MAX_SEGMENT_BYTES)? else {
            return Ok(None);
        };
        decode_segment(target, &bytes).map(Some)
    }

    /// Inspect retained normal/alternate screen checkpoints without starting
    /// a process. Only validated normalized cells are returned.
    pub fn load_screens(&self, target: TargetRef) -> io::Result<[Option<Vec<u8>>; 2]> {
        Ok([
            self.load(target, ScreenBuffer::Normal)?,
            self.load(target, ScreenBuffer::Alternate)?,
        ])
    }

    /// Establish a metadata-only segment for a new live session. This gives
    /// even a shell that never scrolls an end timestamp and retention identity.
    pub fn begin_history(&self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
        self.verify_identity()?;
        if epoch == 0 {
            return Err(invalid());
        }
        if let Some(existing) = self.load_rows(target)? {
            if existing.epoch == epoch && existing.ended_at_unix_ms == 0 {
                self.sync_directory()?;
                return Ok(());
            }
            return Err(invalid());
        }
        let saved = SavedHistory {
            gap,
            capture_disabled: false,
            epoch,
            ended_at_unix_ms: 0,
            records: Vec::new(),
        };
        self.replace_named(&history_name(target)?, &encode_segment(target, &saved)?)
    }

    /// List only complete immutable targets named by validated saved records.
    /// Refuse an excessive directory or unsafe matching entry rather than
    /// silently skipping it during retention or recovery.
    pub fn list_targets(&self) -> io::Result<Vec<TargetRef>> {
        self.verify_identity()?;
        let mut targets = Vec::new();
        let mut examined = 0;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            examined += 1;
            if examined > MAX_HISTORY_TARGETS * 4 {
                return Err(invalid());
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.ends_with(".rows") {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o7777 != 0o600
                || metadata.nlink() != 1
            {
                return Err(unsafe_storage());
            }
            if targets.len() >= MAX_HISTORY_TARGETS {
                return Err(invalid());
            }
            let parts = name
                .trim_end_matches(".rows")
                .split('-')
                .collect::<Vec<_>>();
            if parts.len() != 5
                || parts.iter().any(|part| {
                    part.len() != 16
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
            {
                continue;
            }
            let numbers = parts
                .iter()
                .map(|part| u64::from_str_radix(part, 16))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid())?;
            let target = TargetRef {
                generation: numbers[0],
                workspace: numbers[1],
                pane: numbers[2],
                session: numbers[3],
                occupant_revision: numbers[4],
            };
            if self.load_rows(target)?.is_some() {
                targets.push(target);
            }
        }
        targets.sort_by_key(|target| (target.generation, target.session));
        Ok(targets)
    }

    /// Prune only sessions explicitly marked ended more than 30 days ago.
    /// The caller supplies wall time; a clock rollback never shortens expiry.
    pub fn prune_expired(&self, now_unix_ms: u64) -> io::Result<usize> {
        self.verify_identity()?;
        let mut removed = 0;
        for target in self.list_targets()? {
            let Some(saved) = self.load_rows(target)? else {
                continue;
            };
            if saved.ended_at_unix_ms != 0
                && now_unix_ms
                    .checked_sub(saved.ended_at_unix_ms)
                    .is_some_and(|age| age >= RETENTION_MS)
            {
                self.clear(target)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Commit a bounded retention result in one atomic replacement. No cursor
    /// advances until the directory sync succeeds. A retry validates retained
    /// overlap; an already evicted prefix cannot be recovered or compared.
    pub fn append_rows(&self, target: TargetRef, records: &[Vec<u8>], gap: bool) -> io::Result<()> {
        self.verify_identity()?;
        let Some(first) = records.first() else {
            return Ok(());
        };
        let parsed = history_rows::decode(first)?;
        if parsed.target != target || records.len() > MAX_ROWS {
            return Err(invalid());
        }
        // Validate even the prefix that a previous attempt may have evicted.
        let mut incoming_ordinal = 0;
        for record in records {
            let row = history_rows::decode(record)?;
            if row.target != target || row.epoch != parsed.epoch || row.ordinal <= incoming_ordinal
            {
                return Err(invalid());
            }
            incoming_ordinal = row.ordinal;
        }
        let mut saved = self.load_rows(target)?.unwrap_or(SavedHistory {
            gap: false,
            capture_disabled: false,
            epoch: parsed.epoch,
            ended_at_unix_ms: 0,
            records: Vec::new(),
        });
        if saved.capture_disabled || saved.ended_at_unix_ms != 0 {
            return Err(invalid());
        }
        if saved.epoch > parsed.epoch {
            return Err(invalid());
        }
        if saved.epoch != parsed.epoch {
            saved = SavedHistory {
                gap: true,
                capture_disabled: false,
                epoch: parsed.epoch,
                ended_at_unix_ms: 0,
                records: Vec::new(),
            };
        }
        let prior_gap = saved.gap;
        saved.gap |= gap;
        // Merge the two ordered sequences. Gaps make ordinal offsets differ
        // from vector indices; byte eviction may remove an incoming prefix.
        // Each retained row is visited at most once, including sparse spans.
        let mut start = 0;
        let mut existing_index = 0;
        let mut skipped_prefix = false;
        let mut matched_overlap = false;
        let first_ordinal = saved
            .records
            .first()
            .map(|first| history_rows::decode(first).map(|row| row.ordinal))
            .transpose()?;
        while start < records.len() {
            let row = history_rows::decode(&records[start])?;
            if first_ordinal.is_some_and(|first| row.ordinal < first) {
                skipped_prefix = true;
                start += 1;
                continue;
            }
            while let Some(existing) = saved.records.get(existing_index) {
                if history_rows::decode(existing)?.ordinal >= row.ordinal {
                    break;
                }
                existing_index += 1;
            }
            let Some(existing) = saved.records.get(existing_index) else {
                break;
            };
            // Never backfill an ordinal missing inside the retained span.
            if history_rows::decode(existing)?.ordinal != row.ordinal {
                return Err(invalid());
            }
            if existing != &records[start]
                && !history_rows::same_content(existing, &records[start])?
            {
                return Err(invalid());
            }
            matched_overlap = true;
            existing_index += 1;
            start += 1;
        }
        saved.gap |= skipped_prefix;
        if start == records.len() {
            // An entirely stale, evicted batch has no verifiable overlap and
            // no new tail. It cannot receive a fresh durable acknowledgment.
            if skipped_prefix && !matched_overlap {
                return Err(invalid());
            }
            if saved.gap != prior_gap {
                return self
                    .replace_named(&history_name(target)?, &encode_segment(target, &saved)?);
            }
            self.sync_directory()?;
            return Ok(());
        }
        let mut previous = saved
            .records
            .last()
            .map(|last| history_rows::decode(last).map(|row| row.ordinal))
            .transpose()?
            .unwrap_or(0);
        for record in &records[start..] {
            let row = history_rows::decode(record)?;
            if row.target != target || row.epoch != saved.epoch || row.ordinal <= previous {
                return Err(invalid());
            }
            if row.ordinal != previous.saturating_add(1) {
                saved.gap = true;
            }
            previous = row.ordinal;
            saved.records.push(record.clone());
        }
        let mut total = SEGMENT_HEADER_BYTES
            + saved
                .records
                .iter()
                .map(|record| record.len() + 4)
                .sum::<usize>();
        while saved.records.len() > MAX_ROWS || total > MAX_SEGMENT_BYTES {
            total -= saved.records[0].len() + 4;
            saved.records.remove(0);
            saved.gap = true;
        }
        self.replace_named(&history_name(target)?, &encode_segment(target, &saved)?)
    }

    /// Commit a gap/reset/disabled marker even if no row follows. Saved rows
    /// from earlier epochs are not presented as continuous history.
    pub fn mark_history(&self, target: TargetRef, epoch: u64, disabled: bool) -> io::Result<()> {
        self.verify_identity()?;
        if epoch == 0 {
            return Err(invalid());
        }
        let mut saved = self.load_rows(target)?.unwrap_or(SavedHistory {
            gap: true,
            capture_disabled: disabled,
            epoch,
            ended_at_unix_ms: 0,
            records: Vec::new(),
        });
        if epoch < saved.epoch {
            return Err(invalid());
        }
        if epoch != saved.epoch {
            saved.records.clear();
            saved.epoch = epoch;
        }
        saved.gap = true;
        saved.capture_disabled |= disabled;
        self.replace_named(&history_name(target)?, &encode_segment(target, &saved)?)
    }

    /// Mark the session ended once. Only the server-owned lifecycle path may
    /// call this; retention never treats detachment as an end.
    pub fn mark_ended(&self, target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
        self.verify_identity()?;
        if at_unix_ms == 0 {
            return Err(invalid());
        }
        let Some(mut saved) = self.load_rows(target)? else {
            return Ok(());
        };
        if saved.ended_at_unix_ms != 0 {
            self.sync_directory()?;
            return Ok(());
        }
        saved.ended_at_unix_ms = at_unix_ms;
        self.replace_named(&history_name(target)?, &encode_segment(target, &saved)?)
    }

    /// Best-effort final metadata for accepted work canceled after a timeout.
    ///
    /// The caller owns writer exclusion and supplies a confirmed end time.
    /// A live segment gets a conservative continuity gap and end in one atomic
    /// replacement. Matching ended metadata remains immutable and is synced
    /// again before acknowledgment. Screen checkpoints are never touched.
    pub(crate) fn finalize_incomplete_end(
        &self,
        target: TargetRef,
        epoch: u64,
        end_at_unix_ms: u64,
    ) -> io::Result<()> {
        self.verify_identity()?;
        if epoch == 0 || end_at_unix_ms == 0 {
            return Err(invalid());
        }
        let mut saved = self.load_rows(target)?.unwrap_or(SavedHistory {
            gap: true,
            capture_disabled: false,
            epoch,
            ended_at_unix_ms: 0,
            records: Vec::new(),
        });
        if saved.capture_disabled
            || saved.epoch > epoch
            || (saved.ended_at_unix_ms != 0 && saved.epoch != epoch)
        {
            return Err(invalid());
        }
        if saved.ended_at_unix_ms != 0 {
            return self.sync_directory();
        }
        if saved.epoch != epoch {
            saved.records.clear();
            saved.epoch = epoch;
        }
        saved.gap = true;
        saved.ended_at_unix_ms = end_at_unix_ms;
        self.replace_named(&history_name(target)?, &encode_segment(target, &saved)?)
    }

    /// Explicitly clear only the named target's saved rows and both screen
    /// checkpoints. This does not touch a running PTY or provider conversation.
    pub fn clear(&self, target: TargetRef) -> io::Result<()> {
        self.verify_identity()?;
        // Preflight every named entry before deleting any: refuse symlinks,
        // corrupt rows/checkpoints or non-owned files without partial clear.
        self.load_rows(target)?;
        self.load(target, ScreenBuffer::Normal)?;
        self.load(target, ScreenBuffer::Alternate)?;
        // A generation that is still live may immediately create new files.
        // The caller must coordinate with that server before relying on clear
        // as a durable deletion boundary.
        for filename in [
            history_name(target)?,
            name(target, ScreenBuffer::Normal)?,
            name(target, ScreenBuffer::Alternate)?,
        ] {
            match unlinkat(&self.directory, filename.as_str(), AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.directory.sync_all()
    }

    /// Remove only exact writer-generated temporary files from this history
    /// directory. The caller must exclude all active history writers, just
    /// as it does for [`clear`](Self::clear). This never reads a temporary
    /// body, promotes a temporary file, or repairs a committed history file.
    pub fn cleanup_temporary_files(&self) -> io::Result<usize> {
        self.verify_identity()?;
        let mut total_entries = 0_usize;
        let mut named_candidates = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            total_entries += 1;
            if total_entries > MAX_HISTORY_TARGETS * 4 {
                return Err(invalid());
            }
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if let Some((_, bound)) = temporary_final_name(file_name) {
                if named_candidates.len() == MAX_HISTORY_TARGETS {
                    return Err(invalid());
                }
                named_candidates.push((file_name.to_owned(), bound));
            }
        }

        // The path-based directory listing is only for bounded name discovery;
        // candidate opens and all mutations use the held directory descriptor.
        self.verify_identity()?;
        let mut candidates = Vec::new();
        for (name, bound) in named_candidates {
            let file = File::from(openat(
                &self.directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            candidates.push(TemporaryCandidate::from_file(name, bound, &file)?);
        }

        // Complete preflight before the first unlink. A malformed or unsafe
        // recognized name therefore cannot cause safe candidates to be lost.
        for candidate in &candidates {
            self.verify_identity()?;
            let file = File::from(openat(
                &self.directory,
                candidate.name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            candidate.revalidate(&file)?;
        }

        let mut removed = 0;
        for candidate in &candidates {
            self.verify_identity()?;
            let file = File::from(openat(
                &self.directory,
                candidate.name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            candidate.revalidate(&file)?;
            drop(file);
            unlinkat(&self.directory, candidate.name.as_str(), AtFlags::empty())?;
            removed += 1;
        }

        // Even an empty pass settles an earlier uncertain directory sync.
        self.verify_identity()?;
        self.sync_directory()?;
        Ok(removed)
    }

    fn replace_named(&self, filename: &str, bytes: &[u8]) -> io::Result<()> {
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
            // Refuse a corrupt or unsafe existing destination before rename.
            if let Some(old) = self.read_named(filename, MAX_SEGMENT_BYTES)? {
                let target = segment_target(bytes)?;
                decode_segment(target, &old)?;
            }
            self.before_rename()?;
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                filename,
            )?;
            self.sync_directory()
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }

    /// Atomically replace one checkpoint after validation and a full file sync.
    /// A failed write does not replace the last committed checkpoint.
    pub fn save(&self, target: TargetRef, buffer: ScreenBuffer, bytes: &[u8]) -> io::Result<()> {
        self.verify_identity()?;
        let checkpoint = screen_checkpoint::decode(bytes).map_err(|_| invalid())?;
        if checkpoint.metadata().target() != target || checkpoint.buffer() != buffer {
            return Err(invalid());
        }
        let final_name = name(target, buffer)?;
        // Check the installed slot before creating a temporary. An identical
        // record may already have been renamed by an interrupted sync; a
        // changed record with the same ordering must never replace it.
        if let Some(previous) = self.load(target, buffer)? {
            let decoded = screen_checkpoint::decode(&previous).map_err(|_| invalid())?;
            let order = (
                decoded.metadata().output_sequence(),
                decoded.metadata().screen_revision(),
            );
            let proposed = (
                checkpoint.metadata().output_sequence(),
                checkpoint.metadata().screen_revision(),
            );
            if order >= proposed {
                if order == proposed && previous == bytes {
                    self.sync_directory()?;
                    return Ok(());
                }
                return Err(invalid());
            }
            // A newer observation from this same target is permitted; bytes
            // from a different target cannot alias the generated slot name.
        }
        let temporary = format!(
            ".{}-{}-{}.tmp",
            final_name,
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
            // Existing destination must be ours; do not replace an unexpected
            // entry even though rename would replace a symlink atomically.
            match openat(
                &self.directory,
                final_name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(existing) => {
                    let existing = File::from(existing);
                    validate_file(&existing)?;
                    if existing.metadata()?.len()
                        > screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES as u64
                    {
                        return Err(invalid());
                    }
                    let mut old = Vec::new();
                    existing
                        .take(screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES as u64 + 1)
                        .read_to_end(&mut old)?;
                    let previous = screen_checkpoint::decode(&old).map_err(|_| invalid())?;
                    if previous.metadata().target() != target || previous.buffer() != buffer {
                        return Err(invalid());
                    }
                    if (
                        previous.metadata().output_sequence(),
                        previous.metadata().screen_revision(),
                    ) >= (
                        checkpoint.metadata().output_sequence(),
                        checkpoint.metadata().screen_revision(),
                    ) {
                        // A crash or failed directory sync after rename may
                        // leave the new checkpoint visible. Only the exact
                        // same bytes are safe to acknowledge on retry.
                        if old == bytes {
                            self.sync_directory()?;
                            unlinkat(&self.directory, temporary.as_str(), AtFlags::empty())?;
                            return Ok(());
                        }
                        return Err(invalid());
                    }
                }
                Err(error) if error == rustix::io::Errno::NOENT => {}
                Err(error) => return Err(io::Error::from(error)),
            }
            self.before_rename()?;
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                final_name.as_str(),
            )?;
            self.sync_directory()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }

    /// Load a validated, bounded checkpoint without executing any saved bytes.
    /// `None` means the buffer has not been captured.
    pub fn load(&self, target: TargetRef, buffer: ScreenBuffer) -> io::Result<Option<Vec<u8>>> {
        let filename = name(target, buffer)?;
        let file = match openat(
            &self.directory,
            filename.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(io::Error::from(error)),
        };
        validate_file(&file)?;
        if file.metadata()?.len() > screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let checkpoint: ScreenCheckpoint<'_> =
            screen_checkpoint::decode(&bytes).map_err(|_| invalid())?;
        if checkpoint.metadata().target() != target || checkpoint.buffer() != buffer {
            return Err(invalid());
        }
        Ok(Some(bytes))
    }
}

fn segment_target(bytes: &[u8]) -> io::Result<TargetRef> {
    if bytes.len() < SEGMENT_HEADER_BYTES || &bytes[..8] != SEGMENT_MAGIC {
        return Err(invalid());
    }
    let number = |offset| u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
    Ok(TargetRef {
        generation: number(8),
        workspace: number(16),
        pane: number(24),
        session: number(32),
        occupant_revision: number(40),
    })
}

fn encode_segment(target: TargetRef, saved: &SavedHistory) -> io::Result<Vec<u8>> {
    if saved.epoch == 0 || saved.records.len() > MAX_ROWS {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(
        SEGMENT_HEADER_BYTES
            + saved
                .records
                .iter()
                .map(|record| record.len() + 4)
                .sum::<usize>(),
    );
    bytes.extend_from_slice(SEGMENT_MAGIC);
    for value in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
        saved.epoch,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&(saved.records.len() as u16).to_be_bytes());
    bytes.push(u8::from(saved.gap));
    bytes.push(u8::from(saved.capture_disabled));
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&saved.ended_at_unix_ms.to_be_bytes());
    for record in &saved.records {
        let row = history_rows::decode(record)?;
        if row.target != target || row.epoch != saved.epoch {
            return Err(invalid());
        }
        bytes.extend_from_slice(&(record.len() as u32).to_be_bytes());
        bytes.extend_from_slice(record);
    }
    if bytes.len() > MAX_SEGMENT_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

fn decode_segment(target: TargetRef, bytes: &[u8]) -> io::Result<SavedHistory> {
    if bytes.len() < SEGMENT_HEADER_BYTES
        || bytes.len() > MAX_SEGMENT_BYTES
        || segment_target(bytes)? != target
        || bytes[60..64] != [0; 4]
    {
        return Err(invalid());
    }
    let epoch = u64::from_be_bytes(bytes[48..56].try_into().unwrap());
    let count = usize::from(u16::from_be_bytes(bytes[56..58].try_into().unwrap()));
    if epoch == 0 || count > MAX_ROWS || bytes[58] > 1 || bytes[59] > 1 {
        return Err(invalid());
    }
    let mut saved = SavedHistory {
        gap: bytes[58] == 1,
        capture_disabled: bytes[59] == 1,
        epoch,
        ended_at_unix_ms: u64::from_be_bytes(bytes[64..72].try_into().unwrap()),
        records: Vec::new(),
    };
    saved
        .records
        .try_reserve_exact(count)
        .map_err(|_| invalid())?;
    let mut cursor = SEGMENT_HEADER_BYTES;
    let mut previous = 0;
    let mut first_ordinal = 0;
    for _ in 0..count {
        let length = bytes.get(cursor..cursor + 4).ok_or_else(invalid)?;
        let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
        if !(history_rows::HEADER_BYTES..=history_rows::MAX_ROW_BYTES).contains(&length) {
            return Err(invalid());
        }
        cursor += 4;
        let record = bytes.get(cursor..cursor + length).ok_or_else(invalid)?;
        let row = history_rows::decode(record)?;
        if row.target != target
            || row.epoch != epoch
            || row.ordinal <= previous
            || (previous != 0 && row.ordinal != previous + 1 && !saved.gap)
        {
            return Err(invalid());
        }
        if first_ordinal == 0 {
            first_ordinal = row.ordinal;
        }
        previous = row.ordinal;
        saved.records.push(record.to_vec());
        cursor += length;
    }
    if cursor != bytes.len() || (!saved.gap && first_ordinal > 1) {
        return Err(invalid());
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_checkpoint::CheckpointMetadata;
    use crate::terminal::{ScreenSnapshot, TerminalSize};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
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
                    "panelink-history-unit-{}-{}-{suffix}",
                    std::process::id(),
                    stamp
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(fs::canonicalize(path).unwrap()),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create private history fixture: {error}"),
                }
            }
            panic!("could not allocate a unique private history fixture")
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

    fn target(session: u64) -> TargetRef {
        TargetRef {
            generation: 7,
            workspace: 1,
            pane: 2,
            session,
            occupant_revision: 1,
        }
    }

    fn row_records(target: TargetRef, count: usize) -> Vec<Vec<u8>> {
        row_records_at_width(target, count, 20)
    }

    fn row_records_at_width(target: TargetRef, count: usize, width: u16) -> Vec<Vec<u8>> {
        let mut parser = vt100::Parser::new(2, width, 1024);
        let initial = parser.screen().history_cursor().unwrap();
        for index in 0..=count {
            parser.process(format!("row-{index}\r\n").as_bytes());
        }
        let records = parser
            .screen()
            .history_after(initial)
            .unwrap()
            .map(|row| history_rows::encode(target, 1, row, 123).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), count);
        records
    }

    #[test]
    fn post_rename_retry_matches_retained_suffix_after_incoming_byte_eviction() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records_at_width(target, 1024, 240);
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.append_rows(target, &rows, false).is_err());
        let saved = store.load_rows(target).unwrap().unwrap();
        assert!(saved.gap);
        assert!(saved.records.len() < rows.len());
        let committed = fs::read(fixture.path().join(history_name(target).unwrap())).unwrap();
        // A duplicate still has to reach the second injected sync failure.
        assert_eq!(
            store.append_rows(target, &rows, false).unwrap_err().kind(),
            io::ErrorKind::Other
        );
        store.append_rows(target, &rows, false).unwrap();
        assert_eq!(
            fs::read(fixture.path().join(history_name(target).unwrap())).unwrap(),
            committed
        );
        let mut conflict = rows.clone();
        conflict.last_mut().unwrap()[history_rows::HEADER_BYTES + 10] = b'X';
        assert!(store.append_rows(target, &conflict, false).is_err());
        // Even discarded ordinals must be fully validated, never blindly skipped.
        let mut malformed = rows.clone();
        malformed[0][history_rows::HEADER_BYTES + 10] = 0x1b;
        assert!(store.append_rows(target, &malformed, false).is_err());
        let mut wrong_target = rows.clone();
        wrong_target[0][32..40].copy_from_slice(&99_u64.to_be_bytes());
        assert!(store.append_rows(target, &wrong_target, false).is_err());
        assert!(store.append_rows(target, &rows[..1], false).is_err());
        let mut out_of_order = rows.clone();
        out_of_order.swap(0, 1);
        assert!(store.append_rows(target, &out_of_order, false).is_err());
    }

    #[test]
    fn post_rename_retry_matches_sparse_ordinals_without_allowing_backfill() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records(target, 6);
        store.append_rows(target, &rows[..2], false).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.append_rows(target, &rows[4..], true).is_err());
        assert_eq!(
            store
                .append_rows(target, &rows[4..], true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
        store.append_rows(target, &rows[4..], true).unwrap();
        let expected = [rows[..2].to_vec(), rows[4..].to_vec()].concat();
        assert_eq!(store.load_rows(target).unwrap().unwrap().records, expected);
        assert!(store.append_rows(target, &rows[2..3], true).is_err());
        let mut conflict = rows[4..].to_vec();
        conflict[0][history_rows::HEADER_BYTES + 10] = b'X';
        assert!(store.append_rows(target, &conflict, true).is_err());
        assert_eq!(store.load_rows(target).unwrap().unwrap().records, expected);
    }

    #[test]
    fn duplicate_row_batch_persists_a_new_gap_before_acknowledgment() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records(target, 2);
        store.append_rows(target, &rows, false).unwrap();
        assert!(!store.load_rows(target).unwrap().unwrap().gap);
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.append_rows(target, &rows, true).is_err());
        assert!(store.load_rows(target).unwrap().unwrap().gap);
        assert!(store.append_rows(target, &rows, true).is_err());
        store.append_rows(target, &rows, true).unwrap();
        let saved = store.load_rows(target).unwrap().unwrap();
        assert!(saved.gap);
        assert_eq!(saved.records, rows);
    }

    fn checkpoint(target: TargetRef, sequence: u64, captured_at: u64) -> Vec<u8> {
        let screen = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
        let metadata =
            CheckpointMetadata::new(target, sequence, captured_at, sequence + 1).unwrap();
        screen_checkpoint::encode(&screen, &metadata).unwrap()
    }

    #[test]
    fn canceled_end_preserves_rows_screens_and_first_time_across_sync_retry() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records(target, 3);
        store.append_rows(target, &rows, false).unwrap();
        let normal = checkpoint(target, 1, 11);
        let mut alternate = checkpoint(target, 2, 22);
        alternate[80] = 1;
        store.save(target, ScreenBuffer::Normal, &normal).unwrap();
        store
            .save(target, ScreenBuffer::Alternate, &alternate)
            .unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.finalize_incomplete_end(target, 1, 333).is_err());
        let saved = store.load_rows(target).unwrap().unwrap();
        assert_eq!(saved.records, rows);
        assert!(saved.gap);
        assert_eq!(saved.ended_at_unix_ms, 333);
        let committed = fs::read(fixture.path().join(history_name(target).unwrap())).unwrap();
        // A visible renamed record must not bypass the remaining sync failure.
        assert!(store.finalize_incomplete_end(target, 1, 999).is_err());
        store.finalize_incomplete_end(target, 1, 999).unwrap();
        assert_eq!(
            fs::read(fixture.path().join(history_name(target).unwrap())).unwrap(),
            committed
        );
        assert_eq!(
            store.load_rows(target).unwrap().unwrap().ended_at_unix_ms,
            333
        );
        assert_eq!(
            store.load(target, ScreenBuffer::Normal).unwrap().unwrap(),
            normal
        );
        assert_eq!(
            store
                .load(target, ScreenBuffer::Alternate)
                .unwrap()
                .unwrap(),
            alternate
        );
    }

    #[test]
    fn canceled_end_initializes_or_resets_live_epochs_but_refuses_invalid_state() {
        let fixture = Fixture::new();
        let store = HistoryStore::open(fixture.path()).unwrap();
        let missing = target(3);
        assert!(store.finalize_incomplete_end(missing, 0, 11).is_err());
        assert!(store.finalize_incomplete_end(missing, 1, 0).is_err());
        assert!(store.load_rows(missing).unwrap().is_none());
        store.finalize_incomplete_end(missing, 2, 11).unwrap();
        let saved = store.load_rows(missing).unwrap().unwrap();
        assert_eq!((saved.epoch, saved.ended_at_unix_ms), (2, 11));
        assert!(saved.gap && saved.records.is_empty());

        let live = target(4);
        store
            .append_rows(live, &row_records(live, 2), false)
            .unwrap();
        store.finalize_incomplete_end(live, 2, 22).unwrap();
        let saved = store.load_rows(live).unwrap().unwrap();
        assert_eq!((saved.epoch, saved.ended_at_unix_ms), (2, 22));
        assert!(saved.gap && saved.records.is_empty());
        let bytes = fs::read(fixture.path().join(history_name(live).unwrap())).unwrap();
        assert!(store.finalize_incomplete_end(live, 1, 33).is_err());
        assert!(store.finalize_incomplete_end(live, 3, 33).is_err());
        assert_eq!(
            fs::read(fixture.path().join(history_name(live).unwrap())).unwrap(),
            bytes
        );

        let disabled = target(5);
        store.mark_history(disabled, 1, true).unwrap();
        let bytes = fs::read(fixture.path().join(history_name(disabled).unwrap())).unwrap();
        assert!(store.finalize_incomplete_end(disabled, 1, 44).is_err());
        assert_eq!(
            fs::read(fixture.path().join(history_name(disabled).unwrap())).unwrap(),
            bytes
        );
        let newer = target(6);
        store.begin_history(newer, 3, false).unwrap();
        assert!(store.finalize_incomplete_end(newer, 2, 44).is_err());
        assert_eq!(store.load_rows(newer).unwrap().unwrap().ended_at_unix_ms, 0);

        let complete = target(7);
        store.begin_history(complete, 1, false).unwrap();
        store.mark_ended(complete, 55).unwrap();
        store.finalize_incomplete_end(complete, 1, 66).unwrap();
        let saved = store.load_rows(complete).unwrap().unwrap();
        assert_eq!(saved.ended_at_unix_ms, 55);
        assert!(!saved.gap, "already-ended continuity must remain unchanged");
    }

    #[test]
    fn canceled_end_pre_rename_failure_and_unsafe_records_never_replace_data() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let store = HistoryStore::open(fixture.path()).unwrap();
        let live = target(3);
        store
            .append_rows(live, &row_records(live, 2), false)
            .unwrap();
        let path = fixture.path().join(history_name(live).unwrap());
        let original = fs::read(&path).unwrap();
        store.inject_test_fault(TestFaultPoint::BeforeRename, 1);
        assert!(store.finalize_incomplete_end(live, 1, 77).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::write(&path, b"corrupt-private-row").unwrap();
        assert!(store.finalize_incomplete_end(live, 1, 77).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt-private-row");
        fs::write(&path, &original).unwrap();
        let mismatch = target(4);
        let mismatch_path = fixture.path().join(history_name(mismatch).unwrap());
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&mismatch_path)
            .unwrap()
            .write_all(&original)
            .unwrap();
        assert!(store.finalize_incomplete_end(mismatch, 1, 77).is_err());
        assert_eq!(fs::read(&mismatch_path).unwrap(), original);
        let linked = target(5);
        let linked_path = fixture.path().join(history_name(linked).unwrap());
        symlink(&path, &linked_path).unwrap();
        assert!(store.finalize_incomplete_end(linked, 1, 77).is_err());
        assert!(fs::symlink_metadata(linked_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn failures_before_rename_preserve_committed_rows_and_checkpoints() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records(target, 2);
        store.append_rows(target, &rows[..1], false).unwrap();
        store.inject_test_fault(TestFaultPoint::BeforeRename, 1);
        assert!(store.append_rows(target, &rows[1..], false).is_err());
        assert_eq!(
            store.load_rows(target).unwrap().unwrap().records,
            vec![rows[0].clone()]
        );

        let first = checkpoint(target, 1, 11);
        let second = checkpoint(target, 2, 22);
        store.save(target, ScreenBuffer::Normal, &first).unwrap();
        store.inject_test_fault(TestFaultPoint::BeforeRename, 1);
        assert!(store.save(target, ScreenBuffer::Normal, &second).is_err());
        assert_eq!(
            store.load(target, ScreenBuffer::Normal).unwrap(),
            Some(first.clone())
        );

        drop(store);
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(
            reopened.load_rows(target).unwrap().unwrap().records,
            vec![rows[0].clone()]
        );
        assert_eq!(
            reopened.load(target, ScreenBuffer::Normal).unwrap(),
            Some(first)
        );
        assert!(fs::read_dir(fixture.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        }));
    }

    #[test]
    fn post_rename_row_and_checkpoint_retries_sync_before_ack_and_reject_conflicts() {
        let fixture = Fixture::new();
        let target = target(3);
        let store = HistoryStore::open(fixture.path()).unwrap();
        let rows = row_records(target, 3);
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.append_rows(target, &rows, false).is_err());

        let observed = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(observed.load_rows(target).unwrap().unwrap().records, rows);
        let mut changed_rows = row_records(target, 3);
        changed_rows[1][history_rows::HEADER_BYTES + 10] = b'X';
        assert!(store.append_rows(target, &changed_rows, false).is_err());
        let mut recaptured_rows = rows.clone();
        for row in &mut recaptured_rows {
            row[64..72].copy_from_slice(&456_u64.to_be_bytes());
        }
        assert!(store.append_rows(target, &recaptured_rows, false).is_err());
        assert!(store.append_rows(target, &recaptured_rows, false).is_ok());
        drop(observed);
        drop(store);
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(reopened.load_rows(target).unwrap().unwrap().records, rows);
        drop(reopened);

        let store = HistoryStore::open(fixture.path()).unwrap();
        let second = checkpoint(target, 2, 22);
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.save(target, ScreenBuffer::Normal, &second).is_err());
        let observed = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(
            observed.load(target, ScreenBuffer::Normal).unwrap(),
            Some(second.clone())
        );

        let changed_same_order = checkpoint(target, 2, 99);
        assert!(store
            .save(target, ScreenBuffer::Normal, &changed_same_order)
            .is_err());
        assert!(store.save(target, ScreenBuffer::Normal, &second).is_err());
        assert!(store.save(target, ScreenBuffer::Normal, &second).is_ok());
        drop(observed);
        drop(store);
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(
            reopened.load(target, ScreenBuffer::Normal).unwrap(),
            Some(second)
        );
    }

    #[test]
    fn repeated_begin_and_end_acknowledgments_sync_and_keep_first_end_time() {
        let fixture = Fixture::new();
        let store = HistoryStore::open(fixture.path()).unwrap();
        let ended = target(3);
        let live = target(4);
        let recently_ended = target(5);
        store.begin_history(ended, 1, false).unwrap();
        store.begin_history(live, 1, false).unwrap();
        store.begin_history(recently_ended, 1, false).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync, 1);
        assert!(store.begin_history(live, 1, false).is_err());
        store.begin_history(live, 1, false).unwrap();

        store.mark_ended(recently_ended, 1001).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.mark_ended(ended, 1000).is_err());
        let observed = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(
            observed.load_rows(ended).unwrap().unwrap().ended_at_unix_ms,
            1000
        );
        assert!(store.mark_ended(ended, 2000).is_err());
        assert_eq!(
            observed.load_rows(ended).unwrap().unwrap().ended_at_unix_ms,
            1000
        );
        store.mark_ended(ended, 9000).unwrap();
        drop(observed);
        drop(store);

        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_eq!(
            reopened.load_rows(ended).unwrap().unwrap().ended_at_unix_ms,
            1000
        );
        assert_eq!(reopened.prune_expired(1000 + RETENTION_MS).unwrap(), 1);
        assert!(reopened.load_rows(ended).unwrap().is_none());
        assert!(reopened.load_rows(live).unwrap().is_some());
        assert!(reopened.load_rows(recently_ended).unwrap().is_some());
    }

    #[test]
    fn temporary_cleanup_retries_directory_sync_even_after_visible_deletion() {
        let fixture = Fixture::new();
        let store = HistoryStore::open(fixture.path()).unwrap();
        let temporary = format!(
            ".{}-{}-1.tmp",
            history_name(target(3)).unwrap(),
            std::process::id()
        );
        let path = fixture.path().join(&temporary);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(b"partial; never read or promoted").unwrap();
        file.sync_all().unwrap();
        drop(file);

        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        assert!(store.cleanup_temporary_files().is_err());
        assert!(!path.exists(), "the unlink is visible before sync failure");
        assert!(store.cleanup_temporary_files().is_err());
        assert_eq!(store.cleanup_temporary_files().unwrap(), 0);
    }
}
