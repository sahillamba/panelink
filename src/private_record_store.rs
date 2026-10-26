//! Hardened single-slot publication for typed private records.
//!
//! This module owns only the byte-slot mechanics. Each caller supplies its
//! own codec, scope validation, filenames, and record-size bound.

use rustix::fs::{flock, open, openat, renameat, unlinkat, AtFlags, FlockOperation, Mode, OFlags};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaultPoint {
    BeforeWrite,
    AfterFileSync,
    BeforeRename,
    DirectorySync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublishOutcome {
    Written,
    AlreadyDurable,
}

#[cfg(test)]
struct TestFault {
    point: FaultPoint,
    remaining: usize,
}

/// One locked, exact-path private record slot.
pub(crate) struct AtomicPrivateRecordStore {
    pub(super) directory: File,
    lock: File,
    path: PathBuf,
    current_name: &'static str,
    temp_name: &'static str,
    lock_name: &'static str,
    max_bytes: usize,
    disabled: bool,
    #[cfg(test)]
    test_fault: Mutex<Option<TestFault>>,
}

impl std::fmt::Debug for AtomicPrivateRecordStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AtomicPrivateRecordStore { <redacted> }")
    }
}

fn failure(kind: io::ErrorKind, message: &'static str) -> io::Error {
    io::Error::new(kind, message)
}

fn unsafe_storage() -> io::Error {
    failure(
        io::ErrorKind::PermissionDenied,
        "unsafe private record storage",
    )
}

fn validate_private_file(file: &File) -> io::Result<()> {
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

fn validate_lock_file(file: &File) -> io::Result<()> {
    validate_private_file(file)?;
    if file.metadata()?.len() != 0 {
        return Err(unsafe_storage());
    }
    Ok(())
}

fn validate_directory_path(path: &Path) -> io::Result<()> {
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
    Ok(())
}

fn validate_directory(path: &Path, directory: &File) -> io::Result<()> {
    validate_directory_path(path)?;
    let named = fs::symlink_metadata(path)?;
    let held = directory.metadata()?;
    if held.uid() != rustix::process::geteuid().as_raw()
        || held.permissions().mode() & 0o7777 != 0o700
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
    {
        return Err(unsafe_storage());
    }
    Ok(())
}

fn cleanup_owned_temp(directory: &File, name: &str, identity: (u64, u64)) {
    let Ok(named) = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) else {
        return;
    };
    let named = File::from(named);
    if validate_private_file(&named).is_ok() {
        if let Ok(metadata) = named.metadata() {
            if (metadata.dev(), metadata.ino()) == identity {
                let _ = unlinkat(directory, name, AtFlags::empty());
            }
        }
    }
}

impl AtomicPrivateRecordStore {
    pub(crate) fn open(
        path: &Path,
        current_name: &'static str,
        temp_name: &'static str,
        lock_name: &'static str,
        max_bytes: usize,
    ) -> io::Result<Self> {
        if max_bytes == 0
            || !valid_name(current_name)
            || !valid_name(temp_name)
            || !valid_name(lock_name)
        {
            return Err(failure(
                io::ErrorKind::InvalidInput,
                "invalid private record slot",
            ));
        }
        validate_directory_path(path)?;
        let directory = File::from(open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        validate_directory(path, &directory)?;
        let lock = File::from(openat(
            &directory,
            lock_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )?);
        validate_lock_file(&lock)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| failure(io::ErrorKind::WouldBlock, "private record store is locked"))?;
        validate_lock_file(&lock)?;
        let named_lock = File::from(openat(
            &directory,
            lock_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?);
        validate_lock_file(&named_lock)?;
        if (lock.metadata()?.dev(), lock.metadata()?.ino())
            != (named_lock.metadata()?.dev(), named_lock.metadata()?.ino())
        {
            return Err(unsafe_storage());
        }
        validate_directory(path, &directory)?;
        Ok(Self {
            directory,
            lock,
            path: path.to_owned(),
            current_name,
            temp_name,
            lock_name,
            max_bytes,
            disabled: false,
            #[cfg(test)]
            test_fault: Mutex::new(None),
        })
    }

    pub(crate) fn verify_identity(&self) -> io::Result<()> {
        validate_directory(&self.path, &self.directory)?;
        validate_lock_file(&self.lock)?;
        let named_lock = File::from(openat(
            &self.directory,
            self.lock_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?);
        validate_lock_file(&named_lock)?;
        let held = self.lock.metadata()?;
        let named = named_lock.metadata()?;
        if (held.dev(), held.ino()) != (named.dev(), named.ino()) {
            return Err(unsafe_storage());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_test_fault(&self, point: FaultPoint) {
        *self.test_fault.lock().expect("fault mutex") = Some(TestFault {
            point,
            remaining: 1,
        });
    }

    #[cfg(test)]
    pub(crate) fn cleanup_temp_if_identity(&self, identity: (u64, u64)) {
        cleanup_owned_temp(&self.directory, self.temp_name, identity);
    }

    fn fault(&self, point: FaultPoint) -> io::Result<()> {
        #[cfg(test)]
        {
            let mut configured = self.test_fault.lock().expect("fault mutex");
            if let Some(fault) = configured
                .as_mut()
                .filter(|fault| fault.point == point && fault.remaining > 0)
            {
                fault.remaining -= 1;
                if fault.remaining == 0 {
                    *configured = None;
                }
                return Err(failure(
                    io::ErrorKind::Other,
                    "injected private record storage failure",
                ));
            }
        }
        #[cfg(not(test))]
        let _ = point;
        Ok(())
    }

    fn open_slot(&self, name: &str) -> io::Result<Option<File>> {
        let file = match openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(io::Error::from(error)),
        };
        validate_private_file(&file)?;
        Ok(Some(file))
    }

    pub(crate) fn read(&self) -> io::Result<Option<Vec<u8>>> {
        if self.disabled {
            return Err(failure(
                io::ErrorKind::Other,
                "private record store requires reopen",
            ));
        }
        self.verify_identity()?;
        let Some(file) = self.open_slot(self.current_name)? else {
            return Ok(None);
        };
        let size = file.metadata()?.len();
        if size > self.max_bytes as u64 {
            return Err(failure(
                io::ErrorKind::InvalidData,
                "invalid private record",
            ));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(self.max_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > self.max_bytes || bytes.len() as u64 != size {
            return Err(failure(
                io::ErrorKind::InvalidData,
                "invalid private record",
            ));
        }
        Ok(Some(bytes))
    }

    pub(crate) fn publish(&mut self, bytes: &[u8]) -> io::Result<PublishOutcome> {
        if self.disabled {
            return Err(failure(
                io::ErrorKind::Other,
                "private record store requires reopen",
            ));
        }
        if bytes.is_empty() || bytes.len() > self.max_bytes {
            return Err(failure(
                io::ErrorKind::InvalidInput,
                "private record refused",
            ));
        }
        self.verify_identity()?;
        if self.read()?.as_deref() == Some(bytes) {
            if let Err(error) = self
                .fault(FaultPoint::DirectorySync)
                .and_then(|_| self.directory.sync_all())
            {
                self.disabled = true;
                return Err(error);
            }
            return Ok(PublishOutcome::AlreadyDurable);
        }
        self.verify_identity()?;
        let mut temp_identity = None;
        let created = (|| {
            self.fault(FaultPoint::BeforeWrite)?;
            let mut file = File::from(openat(
                &self.directory,
                self.temp_name,
                OFlags::WRONLY
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK,
                Mode::RUSR | Mode::WUSR,
            )?);
            validate_private_file(&file)?;
            let metadata = file.metadata()?;
            temp_identity = Some((metadata.dev(), metadata.ino()));
            file.write_all(bytes)?;
            file.sync_all()?;
            self.fault(FaultPoint::AfterFileSync)?;
            self.fault(FaultPoint::BeforeRename)?;
            self.verify_identity()?;
            renameat(
                &self.directory,
                self.temp_name,
                &self.directory,
                self.current_name,
            )?;
            Ok(())
        })();
        if let Err(error) = created {
            if let Some(identity) = temp_identity {
                cleanup_owned_temp(&self.directory, self.temp_name, identity);
            }
            return Err(error);
        }
        if let Err(error) = self
            .fault(FaultPoint::DirectorySync)
            .and_then(|_| self.directory.sync_all())
        {
            self.disabled = true;
            return Err(error);
        }
        Ok(PublishOutcome::Written)
    }

    pub(crate) fn discard_pending(&self) -> io::Result<bool> {
        if self.disabled {
            return Err(failure(
                io::ErrorKind::Other,
                "private record store requires reopen",
            ));
        }
        self.verify_identity()?;
        let Some(file) = self.open_slot(self.temp_name)? else {
            return Ok(false);
        };
        if file.metadata()?.len() > self.max_bytes as u64 {
            return Err(failure(
                io::ErrorKind::InvalidData,
                "invalid private record",
            ));
        }
        let before = file.metadata()?;
        self.verify_identity()?;
        let named = self
            .open_slot(self.temp_name)?
            .ok_or_else(|| failure(io::ErrorKind::NotFound, "private record temp disappeared"))?;
        let after = named.metadata()?;
        if (before.dev(), before.ino(), before.len()) != (after.dev(), after.ino(), after.len()) {
            return Err(unsafe_storage());
        }
        unlinkat(&self.directory, self.temp_name, AtFlags::empty())?;
        self.directory.sync_all()?;
        Ok(true)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".." && !name.contains('\0')
}
