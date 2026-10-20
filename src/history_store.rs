//! Private, read-only-recovery storage for the latest visible screen of each
//! buffer. This stores normalized checkpoints, never terminal control streams.
//! Scrollback extraction, capture scheduling and layout restoration are separate.

use crate::control_protocol::TargetRef;
use crate::screen_checkpoint::{self, ScreenBuffer, ScreenCheckpoint};
use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

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
