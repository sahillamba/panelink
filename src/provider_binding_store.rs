//! Durable last-confirmed provider-binding storage for one configured identity.
//!
//! The store persists only the bounded metadata record from
//! `provider_binding_codec`. It does not prove provider authenticity, current
//! identity at crash time, or power-loss durability beyond the filesystem sync
//! operations used here.

use crate::control_protocol::TargetRef;
#[cfg(test)]
use crate::private_record_store::FaultPoint as TestFaultPoint;
use crate::private_record_store::{AtomicPrivateRecordStore, PublishOutcome};
use crate::provider_binding::{Binding, LastConfirmedRecord, ProjectIdentity, SupportedVersion};
use crate::provider_binding_codec::{self, MAX_BINDING_RECORD_BYTES};
use std::io;
use std::path::Path;

const CURRENT_NAME: &str = "last-confirmed.binding";
const TEMP_NAME: &str = ".last-confirmed.binding.tmp";
const LOCK_NAME: &str = ".last-confirmed.binding.lock";

/// A single-writer store for the last confirmed binding of one exact scope.
///
/// `path` must be a caller-dedicated, canonical, private directory for this
/// complete target/project/version tuple. The store deliberately does not
/// define a product-wide path hierarchy.
pub struct ProviderBindingStore {
    slot: AtomicPrivateRecordStore,
    target: TargetRef,
    project: ProjectIdentity,
    version: SupportedVersion,
}

/// Result of a successful store save.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    /// New binding bytes were atomically published and the directory synced.
    Written,
    /// The exact binding bytes were already present and the directory synced.
    AlreadyDurable,
}

impl std::fmt::Debug for ProviderBindingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderBindingStore { <redacted> }")
    }
}

fn fixed_error(kind: io::ErrorKind, message: &'static str) -> io::Error {
    io::Error::new(kind, message)
}

fn invalid_record() -> io::Error {
    fixed_error(
        io::ErrorKind::InvalidData,
        "invalid provider binding record",
    )
}

fn save_refused() -> io::Error {
    fixed_error(
        io::ErrorKind::PermissionDenied,
        "provider binding save refused",
    )
}

fn valid_target(target: TargetRef) -> bool {
    [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .iter()
    .all(|value| *value != 0)
}

fn check_binding_scope(
    binding: &Binding,
    target: TargetRef,
    project: &ProjectIdentity,
    version: &SupportedVersion,
) -> io::Result<()> {
    if binding.target() != target
        || binding.project_identity() != project.as_bytes()
        || binding.supported_version() != version.as_str()
    {
        return Err(save_refused());
    }
    Ok(())
}

impl ProviderBindingStore {
    /// Open a private directory and acquire its lifetime exclusive writer lock.
    ///
    /// A fixed, validated temporary file left by an interrupted write is
    /// preserved. Call `discard_pending` explicitly while holding the lock if
    /// that exact recognized temp is safe to remove.
    pub fn open(path: &Path, target: TargetRef, project: &[u8], version: &str) -> io::Result<Self> {
        if !valid_target(target) {
            return Err(save_refused());
        }
        let project = ProjectIdentity::new(project).map_err(|_| save_refused())?;
        let version = SupportedVersion::new(version).map_err(|_| save_refused())?;
        let slot = AtomicPrivateRecordStore::open(
            path,
            CURRENT_NAME,
            TEMP_NAME,
            LOCK_NAME,
            MAX_BINDING_RECORD_BYTES,
        )?;

        Ok(Self {
            slot,
            target,
            project,
            version,
        })
    }

    /// Verify that the held directory and lock still name the same inodes.
    pub fn verify_identity(&self) -> io::Result<()> {
        self.slot.verify_identity()
    }

    #[cfg(test)]
    fn inject_test_fault(&self, point: TestFaultPoint) {
        self.slot.inject_test_fault(point);
    }

    /// Load the committed last-confirmed record, if present, for this exact scope.
    pub fn load(&self) -> io::Result<Option<LastConfirmedRecord>> {
        let Some(bytes) = self.slot.read()? else {
            return Ok(None);
        };
        let binding = provider_binding_codec::decode(&bytes).map_err(|_| invalid_record())?;
        check_binding_scope(&binding, self.target, &self.project, &self.version)?;
        Ok(Some(LastConfirmedRecord::from_validated_binding(binding)))
    }

    /// Persist a binding before the caller acknowledges its confirmation.
    ///
    /// Revisions must increase across bridge replacements. An exact byte-for-
    /// byte duplicate is idempotent and syncs the directory before success.
    pub fn save(&mut self, binding: &Binding) -> io::Result<SaveOutcome> {
        check_binding_scope(binding, self.target, &self.project, &self.version)?;
        let bytes = provider_binding_codec::encode(binding).map_err(|_| save_refused())?;
        let current = self.slot.read()?;
        let old_binding = current
            .as_deref()
            .map(|data| provider_binding_codec::decode(data).map_err(|_| invalid_record()))
            .transpose()?;
        if let Some(old) = old_binding.as_ref() {
            check_binding_scope(old, self.target, &self.project, &self.version)?;
            if binding.revision() < old.revision()
                || (binding.revision() == old.revision()
                    && bytes.as_slice() != current.as_deref().unwrap_or_default())
            {
                return Err(save_refused());
            }
            if bytes.as_slice() == current.as_deref().unwrap_or_default() {
                return self.slot.publish(&bytes).map(map_publish_outcome);
            }
        }
        self.slot.publish(&bytes).map(map_publish_outcome)
    }

    /// Remove the one recognized interrupted temp after validating its inode and mode.
    pub fn discard_pending(&self) -> io::Result<bool> {
        self.slot.discard_pending()
    }
}

fn map_publish_outcome(outcome: PublishOutcome) -> SaveOutcome {
    match outcome {
        PublishOutcome::Written => SaveOutcome::Written,
        PublishOutcome::AlreadyDurable => SaveOutcome::AlreadyDurable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::{Binding, VerificationSource};
    use rustix::fs::{openat, unlinkat, AtFlags, Mode, OFlags};
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
        target: TargetRef,
        project: ProjectIdentity,
        version: SupportedVersion,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = Path::new("/private/tmp").join(format!("panelink-binding-store-{nonce}"));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("fixture dir");
            Self {
                root,
                target: TargetRef {
                    generation: 1,
                    workspace: 2,
                    pane: 3,
                    session: 4,
                    occupant_revision: 5,
                },
                project: ProjectIdentity::new(b"/private/project").expect("project"),
                version: SupportedVersion::new("2.0.32").expect("version"),
            }
        }

        fn store(&self) -> ProviderBindingStore {
            ProviderBindingStore::open(
                &self.root,
                self.target,
                self.project.as_bytes(),
                self.version.as_str(),
            )
            .expect("open store")
        }

        fn binding(&self, revision: u64, session_id: &str) -> Binding {
            Binding::new(
                self.target,
                &self.project,
                &self.version,
                7,
                revision,
                session_id,
                10_000 + revision,
                VerificationSource::TuiRouteSnapshot,
            )
            .expect("binding")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn saves_loads_and_reopens_last_confirmed_record() {
        let fixture = Fixture::new();
        let binding = fixture.binding(1, "session-a");
        {
            let mut store = fixture.store();
            assert!(store.load().unwrap().is_none());
            store.save(&binding).unwrap();
            assert_eq!(store.load().unwrap().unwrap().binding(), &binding);
        }
        assert_eq!(fixture.store().load().unwrap().unwrap().binding(), &binding);
    }

    #[test]
    fn exact_duplicate_is_idempotent_and_revision_conflicts_refuse() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let first = fixture.binding(2, "session-a");
        store.save(&first).unwrap();
        store.save(&first).unwrap();
        assert!(store.save(&fixture.binding(1, "session-b")).is_err());
        assert!(store.save(&fixture.binding(2, "session-b")).is_err());
        assert_eq!(store.load().unwrap().unwrap().binding(), &first);
    }

    #[test]
    fn duplicate_resync_failure_disables_acknowledgment_until_reopen() {
        let fixture = Fixture::new();
        let first = fixture.binding(2, "session-a");
        let mut store = fixture.store();
        store.save(&first).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync);
        assert!(store.save(&first).is_err());
        assert!(store.load().is_err());
        drop(store);
        assert_eq!(fixture.store().load().unwrap().unwrap().binding(), &first);
    }

    #[test]
    fn wrong_scope_and_unsafe_or_corrupt_committed_slots_are_preserved() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let binding = fixture.binding(1, "session-a");
        store.save(&binding).unwrap();
        drop(store);
        let path = fixture.root.join(CURRENT_NAME);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"corrupt").unwrap();
        let store = fixture.store();
        assert!(store.load().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt");
        drop(store);
        fs::write(&path, original).unwrap();
        let store = fixture.store();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.load().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let alias = fixture.root.join("alias");
        fs::hard_link(&path, &alias).unwrap();
        assert!(store.load().is_err());
        fs::remove_file(alias).unwrap();
        drop(store);
        let wrong_project = ProjectIdentity::new(b"/other/project").unwrap();
        let store = ProviderBindingStore::open(
            &fixture.root,
            fixture.target,
            wrong_project.as_bytes(),
            fixture.version.as_str(),
        )
        .unwrap();
        assert!(store.load().is_err());
        drop(store);
        let wrong_version = ProviderBindingStore::open(
            &fixture.root,
            fixture.target,
            fixture.project.as_bytes(),
            "2.0.33",
        )
        .unwrap();
        assert!(wrong_version.load().is_err());
        drop(wrong_version);
        let wrong_target = TargetRef {
            pane: fixture.target.pane + 1,
            ..fixture.target
        };
        let wrong_target = ProviderBindingStore::open(
            &fixture.root,
            wrong_target,
            fixture.project.as_bytes(),
            fixture.version.as_str(),
        )
        .unwrap();
        assert!(wrong_target.load().is_err());
    }

    #[test]
    fn refuses_lock_contention_directory_replacement_and_wrong_file_type() {
        let fixture = Fixture::new();
        let first = fixture.store();
        assert!(ProviderBindingStore::open(
            &fixture.root,
            fixture.target,
            fixture.project.as_bytes(),
            fixture.version.as_str()
        )
        .is_err());
        drop(first);
        let moved = fixture.root.with_extension("moved");
        let held = fixture.store();
        fs::rename(&fixture.root, &moved).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.root)
            .unwrap();
        assert!(held.verify_identity().is_err());
        drop(held);
        let link = fixture.root.join(CURRENT_NAME);
        std::os::unix::fs::symlink(moved.join(LOCK_NAME), &link).unwrap();
        let store = fixture.store();
        assert!(store.load().is_err());
        drop(store);
        fs::remove_dir_all(moved).unwrap();
        let _ = fs::remove_file(link);
        let _ = fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755));
        assert!(ProviderBindingStore::open(
            &fixture.root,
            fixture.target,
            fixture.project.as_bytes(),
            fixture.version.as_str()
        )
        .is_err());
    }

    #[test]
    fn fifo_slot_is_opened_nonblocking_then_refused() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let fifo_path = CString::new(fixture.root.join(CURRENT_NAME).as_os_str().as_bytes())
            .expect("fixture path has no NUL");
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        assert!(store.load().is_err());
    }

    #[test]
    fn interrupted_temp_is_never_promoted_and_requires_explicit_safe_discard() {
        let fixture = Fixture::new();
        let store = fixture.store();
        fs::write(fixture.root.join(TEMP_NAME), b"partial").unwrap();
        fs::set_permissions(
            fixture.root.join(TEMP_NAME),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(store.discard_pending().unwrap());
        assert!(!store.discard_pending().unwrap());
    }

    #[test]
    fn cleanup_does_not_unlink_a_replacement_temp_inode() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let old = File::from(
            openat(
                &store.slot.directory,
                TEMP_NAME,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .unwrap(),
        );
        let metadata = old.metadata().unwrap();
        let old_identity = (metadata.dev(), metadata.ino());
        unlinkat(&store.slot.directory, TEMP_NAME, AtFlags::empty()).unwrap();
        let replacement = File::from(
            openat(
                &store.slot.directory,
                TEMP_NAME,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .unwrap(),
        );
        let replacement_metadata = replacement.metadata().unwrap();
        assert_ne!(
            old_identity,
            (replacement_metadata.dev(), replacement_metadata.ino())
        );
        store.slot.cleanup_temp_if_identity(old_identity);
        assert!(fixture.root.join(TEMP_NAME).exists());
    }

    #[test]
    fn pre_rename_faults_keep_old_slot_and_directory_sync_fault_disables_writer() {
        for point in [
            TestFaultPoint::BeforeWrite,
            TestFaultPoint::AfterFileSync,
            TestFaultPoint::BeforeRename,
        ] {
            let fixture = Fixture::new();
            let old = fixture.binding(1, "session-old");
            let new = fixture.binding(2, "session-new");
            let mut store = fixture.store();
            store.save(&old).unwrap();
            store.inject_test_fault(point);
            assert!(store.save(&new).is_err());
            assert_eq!(store.load().unwrap().unwrap().binding(), &old);
            drop(store);
            assert_eq!(fixture.store().load().unwrap().unwrap().binding(), &old);
        }
        let fixture = Fixture::new();
        let old = fixture.binding(1, "session-old");
        let new = fixture.binding(2, "session-new");
        let mut store = fixture.store();
        store.save(&old).unwrap();
        store.inject_test_fault(TestFaultPoint::DirectorySync);
        assert!(store.save(&new).is_err());
        assert!(store.load().is_err());
        assert!(store.save(&fixture.binding(3, "session-third")).is_err());
        drop(store);
        assert_eq!(fixture.store().load().unwrap().unwrap().binding(), &new);
    }
}
