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

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const MAX_ROWS: usize = 1024;
const MAX_SEGMENT_BYTES: usize = 8 * 1024 * 1024;
const SEGMENT_HEADER_BYTES: usize = 72;
const SEGMENT_MAGIC: &[u8; 8] = b"PLHSEG02";
const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

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

/// A private directory owned by the current user, with one atomic slot for
/// each target/buffer. The caller supplies a created 0700 history directory.
pub struct HistoryStore {
    directory: File,
    path: PathBuf,
}

impl std::fmt::Debug for HistoryStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HistoryStore { <redacted> }")
    }
}

impl HistoryStore {
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
        })
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
            return if existing.epoch == epoch && existing.ended_at_unix_ms == 0 {
                Ok(())
            } else {
                Err(invalid())
            };
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

    /// Commit a bounded batch of rows in one atomic replacement. No cursor
    /// advances until the directory sync succeeds.
    pub fn append_rows(&self, target: TargetRef, records: &[Vec<u8>], gap: bool) -> io::Result<()> {
        self.verify_identity()?;
        let Some(first) = records.first() else {
            return Ok(());
        };
        let parsed = history_rows::decode(first)?;
        if parsed.target != target || records.len() > MAX_ROWS {
            return Err(invalid());
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
        saved.gap |= gap;
        // The complete existing segment was validated by load_rows. Its
        // records are in ordinal order; compare by offset instead of decoding
        // every retained row again for every incoming ordinal.
        let mut start = 0;
        let first_ordinal = saved
            .records
            .first()
            .map(|first| history_rows::decode(first).map(|row| row.ordinal))
            .transpose()?;
        while start < records.len() {
            let row = history_rows::decode(&records[start])?;
            let Some(existing) = first_ordinal.and_then(|first_ordinal| {
                let offset = row.ordinal.checked_sub(first_ordinal)?;
                saved.records.get(usize::try_from(offset).ok()?)
            }) else {
                break;
            };
            if existing != &records[start]
                && !history_rows::same_content(existing, &records[start])?
            {
                return Err(invalid());
            }
            start += 1;
        }
        if start == records.len() {
            self.directory.sync_all()?;
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
            return Ok(());
        }
        saved.ended_at_unix_ms = at_unix_ms;
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
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                filename,
            )?;
            self.directory.sync_all()
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
                    self.directory.sync_all()?;
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
                            self.directory.sync_all()?;
                            unlinkat(&self.directory, temporary.as_str(), AtFlags::empty())?;
                            return Ok(());
                        }
                        return Err(invalid());
                    }
                }
                Err(error) if error == rustix::io::Errno::NOENT => {}
                Err(error) => return Err(io::Error::from(error)),
            }
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                final_name.as_str(),
            )?;
            self.directory.sync_all()?;
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
