//! Bounded, historical links from saved targets to explicitly relaunched ones.
//!
//! A link is an association for inspection only. It never makes its old target
//! live or authorizes dispatch to either target.

use crate::control_protocol::TargetRef;
use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of retained relaunch links. Oldest links are removed first.
pub const MAX_LINKS: usize = 64;
/// Link retention, matching saved history.
pub const LINK_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

const MAGIC: &[u8; 8] = b"PLLNK001";
const HEADER_BYTES: usize = 12;
const TARGET_BYTES: usize = 5 * 8;
const RECORD_BYTES: usize = 2 * 8 + 2 * TARGET_BYTES + 1 + 8;
const MAX_FILE_BYTES: usize = HEADER_BYTES + MAX_LINKS * RECORD_BYTES;
const FILE_NAME: &str = "links.bin";
const MAX_READ_RETRIES: usize = 4;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// Result of an explicitly attempted relaunch for one target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutcome {
    /// The new target was verified as applied.
    Applied,
    /// The attempt's result could not be verified.
    Unknown,
}

/// Historical association between a saved target and a new-generation target.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RelaunchLink {
    /// Generation recorded by the saved layout.
    pub source_generation: u64,
    /// Snapshot sequence recorded by the saved layout.
    pub source_sequence: u64,
    /// Historical target; never dispatched by this store.
    pub old_target: TargetRef,
    /// Owner-issued target in the newly launched generation.
    pub new_target: TargetRef,
    /// Outcome recorded for the attempted relaunch.
    pub outcome: LinkOutcome,
    /// Link creation time in Unix milliseconds.
    pub linked_at_unix_ms: u64,
}

impl fmt::Debug for RelaunchLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelaunchLink")
            .field("source_generation", &"<redacted>")
            .field("source_sequence", &"<redacted>")
            .field("old_target", &"<redacted>")
            .field("new_target", &"<redacted>")
            .field("outcome", &self.outcome)
            .field("linked_at_unix_ms", &"<redacted>")
            .finish()
    }
}

/// Held private directory containing one bounded atomic link file.
pub struct LinkStore {
    directory: File,
    path: PathBuf,
    #[cfg(test)]
    fail_before_rename: std::sync::Mutex<bool>,
}

impl fmt::Debug for LinkStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LinkStore { <redacted> }")
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid relaunch links")
}

fn unsafe_storage() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unsafe relaunch link storage",
    )
}

fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn validate_directory_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || fs::canonicalize(path)? != path {
        return Err(unsafe_storage());
    }
    let uid = current_uid();
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
    Ok(())
}

fn open_directory(path: &Path) -> io::Result<File> {
    validate_directory_path(path)?;
    let directory = File::from(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = directory.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if metadata.uid() != current_uid()
        || metadata.mode() & 0o7777 != 0o700
        || (metadata.dev(), metadata.ino()) != (named.dev(), named.ino())
    {
        return Err(unsafe_storage());
    }
    Ok(directory)
}

fn validate_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(unsafe_storage());
    }
    Ok(())
}

fn validate_target(target: TargetRef) -> bool {
    target.generation != 0
        && target.workspace != 0
        && target.pane != 0
        && target.session != 0
        && target.occupant_revision != 0
}

fn validate_link(link: RelaunchLink) -> io::Result<()> {
    if link.source_generation == 0
        || link.source_sequence == 0
        || link.linked_at_unix_ms == 0
        || !validate_target(link.old_target)
        || !validate_target(link.new_target)
        || link.source_generation != link.old_target.generation
        || link.old_target.generation == link.new_target.generation
        || link.old_target.workspace != link.new_target.workspace
    {
        return Err(invalid());
    }
    Ok(())
}

fn encode(links: &[RelaunchLink]) -> io::Result<Vec<u8>> {
    if links.len() > MAX_LINKS {
        return Err(invalid());
    }
    let capacity = HEADER_BYTES + links.len() * RECORD_BYTES;
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(links.len() as u32).to_be_bytes());
    for link in links {
        validate_link(*link)?;
        bytes.extend_from_slice(&link.source_generation.to_be_bytes());
        bytes.extend_from_slice(&link.source_sequence.to_be_bytes());
        encode_target(&mut bytes, link.old_target);
        encode_target(&mut bytes, link.new_target);
        bytes.push(match link.outcome {
            LinkOutcome::Applied => 1,
            LinkOutcome::Unknown => 2,
        });
        bytes.extend_from_slice(&link.linked_at_unix_ms.to_be_bytes());
    }
    Ok(bytes)
}

fn encode_target(bytes: &mut Vec<u8>, target: TargetRef) {
    bytes.extend_from_slice(&target.generation.to_be_bytes());
    bytes.extend_from_slice(&target.workspace.to_be_bytes());
    bytes.extend_from_slice(&target.pane.to_be_bytes());
    bytes.extend_from_slice(&target.session.to_be_bytes());
    bytes.extend_from_slice(&target.occupant_revision.to_be_bytes());
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> io::Result<u64> {
    let end = offset.checked_add(8).ok_or_else(invalid)?;
    let value = u64::from_be_bytes(
        bytes
            .get(*offset..end)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    );
    *offset = end;
    Ok(value)
}

fn decode_target(bytes: &[u8], offset: &mut usize) -> io::Result<TargetRef> {
    Ok(TargetRef {
        generation: take_u64(bytes, offset)?,
        workspace: take_u64(bytes, offset)?,
        pane: take_u64(bytes, offset)?,
        session: take_u64(bytes, offset)?,
        occupant_revision: take_u64(bytes, offset)?,
    })
}

fn decode(bytes: &[u8]) -> io::Result<Vec<RelaunchLink>> {
    if bytes.len() < HEADER_BYTES || bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err(invalid());
    }
    let count = u32::from_be_bytes(bytes[8..12].try_into().map_err(|_| invalid())?) as usize;
    if count > MAX_LINKS || bytes.len() != HEADER_BYTES + count * RECORD_BYTES {
        return Err(invalid());
    }
    let mut links = Vec::with_capacity(count);
    let mut offset = HEADER_BYTES;
    for _ in 0..count {
        let source_generation = take_u64(bytes, &mut offset)?;
        let source_sequence = take_u64(bytes, &mut offset)?;
        let old_target = decode_target(bytes, &mut offset)?;
        let new_target = decode_target(bytes, &mut offset)?;
        let outcome = match *bytes.get(offset).ok_or_else(invalid)? {
            1 => LinkOutcome::Applied,
            2 => LinkOutcome::Unknown,
            _ => return Err(invalid()),
        };
        offset += 1;
        let linked_at_unix_ms = take_u64(bytes, &mut offset)?;
        let link = RelaunchLink {
            source_generation,
            source_sequence,
            old_target,
            new_target,
            outcome,
            linked_at_unix_ms,
        };
        validate_link(link)?;
        if links.iter().any(|existing: &RelaunchLink| {
            existing.old_target == link.old_target
                && existing.new_target.generation == link.new_target.generation
        }) {
            return Err(invalid());
        }
        links.push(link);
    }
    Ok(links)
}

fn is_expired(link: RelaunchLink, now_unix_ms: u64) -> bool {
    now_unix_ms
        .checked_sub(link.linked_at_unix_ms)
        .is_some_and(|age| age >= LINK_RETENTION_MS)
}

fn prune(links: &mut Vec<RelaunchLink>, now_unix_ms: u64) {
    links.retain(|link| !is_expired(*link, now_unix_ms));
    while links.len() > MAX_LINKS {
        let oldest = links
            .iter()
            .enumerate()
            .min_by_key(|(index, link)| (link.linked_at_unix_ms, *index))
            .map(|(index, _)| index)
            .expect("over-capacity link vector is not empty");
        links.remove(oldest);
    }
}

impl LinkStore {
    /// Open an existing `server/links/` directory without creating it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = open_directory(path)?;
        Ok(Self {
            directory,
            path: path.to_owned(),
            #[cfg(test)]
            fail_before_rename: std::sync::Mutex::new(false),
        })
    }

    /// Create `server/links/` as a private directory when it is absent, then open it.
    pub fn prepare(server_directory: &Path) -> io::Result<Self> {
        let server = open_directory(server_directory)?;
        let path = server_directory.join("links");
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        drop(server);
        Self::open(&path)
    }

    /// Append a link atomically, prune expired/over-capacity records, and make
    /// an identical retry idempotent. Caller holds the server singleton.
    pub fn record(&self, link: RelaunchLink, now_unix_ms: u64) -> io::Result<()> {
        validate_link(link)?;
        self.verify_identity()?;
        let existing_bytes = self.read_committed_bytes()?;
        let mut links = existing_bytes
            .as_deref()
            .map(decode)
            .transpose()?
            .unwrap_or_default();
        prune(&mut links, now_unix_ms);
        for existing in &links {
            if existing.old_target == link.old_target
                && existing.new_target.generation == link.new_target.generation
            {
                if *existing == link {
                    self.directory.sync_all()?;
                    return Ok(());
                }
                return Err(invalid());
            }
        }
        if !is_expired(link, now_unix_ms) {
            links.push(link);
        }
        prune(&mut links, now_unix_ms);
        self.atomic_write(&links, &existing_bytes)
    }

    /// Read all validated retained links. Corruption refuses the complete file.
    pub fn load(&self) -> io::Result<Vec<RelaunchLink>> {
        self.verify_identity()?;
        let Some(file) = self.open_committed()? else {
            return Ok(Vec::new());
        };
        let length = file.metadata()?.len();
        if length > MAX_FILE_BYTES as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        decode(&bytes)
    }

    /// Read-only filter for links from one exact historical target.
    pub fn links_for_old(&self, old: TargetRef) -> io::Result<Vec<RelaunchLink>> {
        if !validate_target(old) {
            return Err(invalid());
        }
        Ok(self
            .load()?
            .into_iter()
            .filter(|link| link.old_target == old)
            .collect())
    }

    fn verify_identity(&self) -> io::Result<()> {
        let named = open_directory(&self.path)?;
        let held = self.directory.metadata()?;
        let current = named.metadata()?;
        if (held.dev(), held.ino()) != (current.dev(), current.ino()) {
            return Err(unsafe_storage());
        }
        Ok(())
    }

    fn open_committed(&self) -> io::Result<Option<File>> {
        for _ in 0..MAX_READ_RETRIES {
            let file = match openat(
                &self.directory,
                FILE_NAME,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) {
                Ok(file) => File::from(file),
                Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                Err(error) => return Err(io::Error::from(error)),
            };
            if file.metadata()?.nlink() == 0 {
                continue;
            }
            validate_file(&file)?;
            return Ok(Some(file));
        }
        Err(unsafe_storage())
    }

    fn atomic_write(
        &self,
        links: &[RelaunchLink],
        expected_bytes: &Option<Vec<u8>>,
    ) -> io::Result<()> {
        self.verify_identity()?;
        let bytes = encode(links)?;
        let temporary = format!(
            ".links-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let mut temporary_created = false;
        let result = (|| {
            let mut file = File::from(openat(
                &self.directory,
                temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            temporary_created = true;
            validate_file(&file)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            rustix::fs::fcntl_fullfsync(&file)?;

            self.verify_identity()?;
            let current = self.read_committed_bytes()?;
            if &current != expected_bytes {
                return Err(unsafe_storage());
            }
            #[cfg(test)]
            {
                let mut fail = self.fail_before_rename.lock().expect("test fault mutex");
                if *fail {
                    *fail = false;
                    return Err(io::Error::other("injected relaunch-link failure"));
                }
            }
            renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                FILE_NAME,
            )?;
            temporary_created = false;
            self.directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() && temporary_created {
            let _ = unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }

    fn read_committed_bytes(&self) -> io::Result<Option<Vec<u8>>> {
        let Some(file) = self.open_committed()? else {
            return Ok(None);
        };
        let length = file.metadata()?.len();
        if length > MAX_FILE_BYTES as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    #[cfg(test)]
    fn inject_before_rename(&self) {
        *self.fail_before_rename.lock().expect("test fault mutex") = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_before_rename_preserves_committed_file() {
        let base = std::env::temp_dir().join(format!(
            "panelink-link-unit-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let store = LinkStore::prepare(&base).unwrap();
        let link = RelaunchLink {
            source_generation: 1,
            source_sequence: 2,
            old_target: TargetRef {
                generation: 1,
                workspace: 3,
                pane: 4,
                session: 5,
                occupant_revision: 6,
            },
            new_target: TargetRef {
                generation: 2,
                workspace: 3,
                pane: 7,
                session: 8,
                occupant_revision: 9,
            },
            outcome: LinkOutcome::Applied,
            linked_at_unix_ms: 10,
        };
        store.record(link, 10).unwrap();
        let path = base.join("links").join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        let changed = RelaunchLink {
            new_target: TargetRef {
                generation: 3,
                ..link.new_target
            },
            ..link
        };
        store.inject_before_rename();
        assert!(store.record(changed, 11).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        drop(store);
        fs::remove_dir_all(base).unwrap();
    }
}
