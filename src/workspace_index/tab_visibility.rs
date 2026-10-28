//! Strict fixed-size persistence for the reserved workspace tab's visibility.
//!
//! Every store operation borrows an open `WorkspaceIndexStore`, so it can only
//! run while that store owns the workspace-index lifetime lock.

use super::store::{
    cleanup_owned_named_temp, open_named, private_file, SaveOutcome, StoreError,
    WorkspaceIndexStore,
};
use rustix::fs::{openat, Mode, OFlags};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::{FileExt, MetadataExt};

const MAGIC: &[u8; 8] = b"PLWKHT01";
const VERSION: u16 = 1;
const HEADER_LEN: u16 = 32;
const RECORD_LEN: u32 = 32;
const FILE_NAME: &str = "workspace-tabs.v1";
const TEMP_NAME: &str = "workspace-tabs.v1.tmp";

/// Errors from the canonical `workspace-tabs.v1` record codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabVisibilityCodecError {
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion,
    InvalidHeaderLength,
    InvalidTotalLength,
    InvalidRevision,
    InvalidHiddenFlag,
    ReservedField,
}

/// The only state persisted by the workspace tab sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabVisibilityRecord {
    revision: u64,
    hidden: bool,
}

impl TabVisibilityRecord {
    pub fn new(revision: u64, hidden: bool) -> Result<Self, TabVisibilityCodecError> {
        if revision == 0 {
            return Err(TabVisibilityCodecError::InvalidRevision);
        }
        Ok(Self { revision, hidden })
    }

    pub const fn revision(self) -> u64 {
        self.revision
    }

    pub const fn hidden(self) -> bool {
        self.hidden
    }

    /// Encode the exact canonical 32-byte big-endian representation.
    pub fn encode(self) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..10].copy_from_slice(&VERSION.to_be_bytes());
        bytes[10..12].copy_from_slice(&HEADER_LEN.to_be_bytes());
        bytes[12..16].copy_from_slice(&RECORD_LEN.to_be_bytes());
        bytes[16..24].copy_from_slice(&self.revision.to_be_bytes());
        bytes[24] = u8::from(self.hidden);
        bytes
    }

    /// Decode one complete canonical record, rejecting all noncanonical data.
    pub fn decode(bytes: &[u8]) -> Result<Self, TabVisibilityCodecError> {
        if bytes.len() != 32 {
            return Err(TabVisibilityCodecError::InvalidLength);
        }
        if &bytes[..8] != MAGIC {
            return Err(TabVisibilityCodecError::InvalidMagic);
        }
        if u16::from_be_bytes(bytes[8..10].try_into().unwrap()) != VERSION {
            return Err(TabVisibilityCodecError::UnsupportedVersion);
        }
        if u16::from_be_bytes(bytes[10..12].try_into().unwrap()) != HEADER_LEN {
            return Err(TabVisibilityCodecError::InvalidHeaderLength);
        }
        if u32::from_be_bytes(bytes[12..16].try_into().unwrap()) != RECORD_LEN {
            return Err(TabVisibilityCodecError::InvalidTotalLength);
        }
        let revision = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
        if revision == 0 {
            return Err(TabVisibilityCodecError::InvalidRevision);
        }
        let hidden = match bytes[24] {
            0 => false,
            1 => true,
            _ => return Err(TabVisibilityCodecError::InvalidHiddenFlag),
        };
        if bytes[25..32].iter().any(|byte| *byte != 0) {
            return Err(TabVisibilityCodecError::ReservedField);
        }
        Ok(Self { revision, hidden })
    }
}

struct ExistingSidecar {
    identity: (u64, u64),
    bytes: [u8; 32],
    record: TabVisibilityRecord,
}

/// Sidecar access scoped to the exclusive workspace-index store lock.
pub struct TabVisibilityStore<'a> {
    workspace: &'a mut WorkspaceIndexStore,
}

impl fmt::Debug for TabVisibilityStore<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TabVisibilityStore { <redacted> }")
    }
}

impl<'a> TabVisibilityStore<'a> {
    /// Bind the sidecar API to the already locked workspace-index store.
    pub fn new(workspace: &'a mut WorkspaceIndexStore) -> Self {
        Self { workspace }
    }

    /// Load sidecar state. Absence means the legacy all-visible presentation.
    pub fn load(&mut self) -> Result<Option<TabVisibilityRecord>, StoreError> {
        self.current()
            .map(|sidecar| sidecar.map(|item| item.record))
    }

    /// Initialize revision 1 with the reserved route visible, without changing
    /// the v1 catalog. Existing valid sidecar state is returned unchanged.
    pub fn initialize(&mut self) -> Result<TabVisibilityRecord, StoreError> {
        if self.workspace.disabled {
            return Err(StoreError::NeedsReopen);
        }
        if let Some(existing) = self.current()? {
            return Ok(existing.record);
        }
        let record =
            TabVisibilityRecord::new(1, false).map_err(|_| StoreError::InvalidTransition)?;
        self.publish(None, record.encode())?;
        Ok(record)
    }

    /// Compare-and-swap the hidden bit, advancing the sidecar revision by one.
    pub fn save_hidden(
        &mut self,
        expected_revision: u64,
        hidden: bool,
    ) -> Result<SaveOutcome, StoreError> {
        if self.workspace.disabled {
            return Err(StoreError::NeedsReopen);
        }
        let current = self.current()?.ok_or(StoreError::Uninitialized)?;
        if expected_revision.checked_add(1) == Some(current.record.revision())
            && current.record.hidden() == hidden
        {
            if self.workspace.directory.sync_all().is_err() {
                self.workspace.disabled = true;
                return Err(StoreError::Io);
            }
            return Ok(SaveOutcome::AlreadyDurable);
        }
        if current.record.revision() != expected_revision {
            return Err(StoreError::RevisionConflict);
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::InvalidTransition)?;
        let next = TabVisibilityRecord::new(next_revision, hidden)
            .map_err(|_| StoreError::InvalidTransition)?;
        self.publish(Some(current), next.encode())
    }

    fn current(&self) -> Result<Option<ExistingSidecar>, StoreError> {
        self.workspace.verify_identity()?;
        let Some(file) = open_named(&self.workspace.directory, FILE_NAME)? else {
            return Ok(None);
        };
        private_file(&file, false)?;
        let metadata = file.metadata().map_err(|_| StoreError::Io)?;
        if metadata.len() != RECORD_LEN as u64 {
            return Err(StoreError::CorruptRecord);
        }
        let bytes = read_record(&file)?;
        let record = TabVisibilityRecord::decode(&bytes).map_err(|_| StoreError::CorruptRecord)?;

        self.workspace.verify_identity()?;
        let named =
            open_named(&self.workspace.directory, FILE_NAME)?.ok_or(StoreError::UnsafeFile)?;
        private_file(&named, false)?;
        let named_metadata = named.metadata().map_err(|_| StoreError::Io)?;
        if (metadata.dev(), metadata.ino(), metadata.len())
            != (
                named_metadata.dev(),
                named_metadata.ino(),
                named_metadata.len(),
            )
            || read_record(&named)? != bytes
        {
            return Err(StoreError::UnsafeFile);
        }
        Ok(Some(ExistingSidecar {
            identity: (metadata.dev(), metadata.ino()),
            bytes,
            record,
        }))
    }

    fn publish(
        &mut self,
        previous: Option<ExistingSidecar>,
        bytes: [u8; 32],
    ) -> Result<SaveOutcome, StoreError> {
        self.workspace.verify_identity()?;
        let existing = open_named(&self.workspace.directory, FILE_NAME)?;
        match (previous.as_ref(), existing.as_ref()) {
            (None, None) => {}
            (Some(previous), Some(file)) => {
                private_file(file, false)?;
                let metadata = file.metadata().map_err(|_| StoreError::Io)?;
                if (metadata.dev(), metadata.ino()) != previous.identity
                    || read_record(file)? != previous.bytes
                {
                    return Err(StoreError::UnsafeFile);
                }
            }
            _ => return Err(StoreError::UnsafeFile),
        }

        let mut temp_identity = None;
        let result = (|| {
            let temp = File::from(
                openat(
                    &self.workspace.directory,
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
            writer.write_all(&bytes).map_err(|_| StoreError::Io)?;
            temp.sync_all().map_err(|_| StoreError::Io)?;

            self.workspace.verify_identity()?;
            let existing = open_named(&self.workspace.directory, FILE_NAME)?;
            match (previous.as_ref(), existing.as_ref()) {
                (None, None) => {}
                (Some(previous), Some(file)) => {
                    private_file(file, false)?;
                    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
                    if (metadata.dev(), metadata.ino()) != previous.identity
                        || read_record(file)? != previous.bytes
                    {
                        return Err(StoreError::UnsafeFile);
                    }
                }
                _ => return Err(StoreError::UnsafeFile),
            }
            verify_temp(
                &self.workspace.directory,
                temp_identity.ok_or(StoreError::Io)?,
                &bytes,
            )?;
            rustix::fs::renameat(
                &self.workspace.directory,
                TEMP_NAME,
                &self.workspace.directory,
                FILE_NAME,
            )
            .map_err(|_| StoreError::Io)?;
            if self.workspace.directory.sync_all().is_err() {
                self.workspace.disabled = true;
                return Err(StoreError::Io);
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(identity) = temp_identity {
                cleanup_owned_named_temp(&self.workspace.directory, TEMP_NAME, identity);
            }
            return Err(error);
        }
        Ok(SaveOutcome::Written)
    }
}

fn read_record(file: &File) -> Result<[u8; 32], StoreError> {
    let mut bytes = [0; 32];
    let mut offset = 0;
    while offset < bytes.len() {
        let read = file
            .read_at(&mut bytes[offset..], offset as u64)
            .map_err(|_| StoreError::Io)?;
        if read == 0 {
            return Err(StoreError::CorruptRecord);
        }
        offset += read;
    }
    let mut extra = [0; 1];
    if file.read_at(&mut extra, 32).map_err(|_| StoreError::Io)? != 0 {
        return Err(StoreError::CorruptRecord);
    }
    Ok(bytes)
}

fn verify_temp(
    directory: &File,
    identity: (u64, u64),
    expected: &[u8; 32],
) -> Result<(), StoreError> {
    let named = open_named(directory, TEMP_NAME)?.ok_or(StoreError::UnsafeFile)?;
    private_file(&named, false)?;
    let metadata = named.metadata().map_err(|_| StoreError::Io)?;
    if (metadata.dev(), metadata.ino()) != identity || read_record(&named)? != *expected {
        return Err(StoreError::UnsafeFile);
    }
    Ok(())
}
