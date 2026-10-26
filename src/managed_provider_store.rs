//! One atomic historical managed-session and provider-binding record.
//!
//! The record couples the managed launch operation and pinned executable
//! identity with the last-confirmed provider binding in the same byte slot.
//! A loaded `Running` value is explicitly historical; it never means a child
//! is live now and never authorizes automatic respawn or provider resume.

use crate::control_protocol::TargetRef;
use crate::private_record_store::{AtomicPrivateRecordStore, PublishOutcome};
use crate::protocol::OperationKey;
use crate::provider_binding::{Binding, ProjectIdentity, SupportedVersion};
use crate::provider_binding_codec::{self, MAX_BINDING_RECORD_BYTES};
use std::io;
use std::path::Path;

const CURRENT_NAME: &str = "managed-provider.record";
const TEMP_NAME: &str = ".managed-provider.record.tmp";
const LOCK_NAME: &str = ".managed-provider.record.lock";
const MAGIC: &[u8; 8] = b"PLMPRV01";
const HEADER_BYTES: usize = 72;
const MAX_EXECUTABLE_PATH_BYTES: usize = 4_096;
/// Maximum combined metadata and nested provider binding record.
pub const MAX_MANAGED_PROVIDER_RECORD_BYTES: usize =
    HEADER_BYTES + MAX_EXECUTABLE_PATH_BYTES + MAX_BINDING_RECORD_BYTES;

/// Exact SHA-256 digest of the provider executable selected by the launcher.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ProviderExecutableFingerprint([u8; 32]);

impl ProviderExecutableFingerprint {
    /// Construct a nonzero pinned digest. The digest must come from the trusted
    /// executable-resolution path; this type does not hash or attest a file.
    pub fn new(bytes: [u8; 32]) -> io::Result<Self> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(refused());
        }
        Ok(Self(bytes))
    }

    /// Borrow the exact digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ProviderExecutableFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderExecutableFingerprint { <redacted> }")
    }
}

/// One configured managed launch and the identity used to bind its provider.
#[derive(Clone, Eq, PartialEq)]
pub struct ManagedProviderScope {
    target: TargetRef,
    project: ProjectIdentity,
    provider_version: SupportedVersion,
    launch_operation: OperationKey,
    executable: ProviderExecutableFingerprint,
    canonical_executable_path: Option<Vec<u8>>,
}

impl ManagedProviderScope {
    /// Validate the exact launch, project, and provider executable scope.
    ///
    /// `canonical_executable_path`, when present, must be an already-resolved
    /// absolute path. It is bounded metadata only and never includes arguments
    /// or environment values.
    pub fn new(
        target: TargetRef,
        project: &[u8],
        provider_version: &str,
        launch_operation: OperationKey,
        executable_sha256: [u8; 32],
        canonical_executable_path: Option<&[u8]>,
    ) -> io::Result<Self> {
        if !valid_target(target)
            || launch_operation.generation != target.generation
            || launch_operation.lease == 0
            || launch_operation.sequence == 0
        {
            return Err(refused());
        }
        let project = ProjectIdentity::new(project).map_err(|_| refused())?;
        let provider_version = SupportedVersion::new(provider_version).map_err(|_| refused())?;
        let executable = ProviderExecutableFingerprint::new(executable_sha256)?;
        let canonical_executable_path = canonical_executable_path
            .map(validate_canonical_path)
            .transpose()?;
        Ok(Self {
            target,
            project,
            provider_version,
            launch_operation,
            executable,
            canonical_executable_path,
        })
    }

    /// Complete immutable managed target.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Exact canonical project identity bytes.
    pub fn project(&self) -> &[u8] {
        self.project.as_bytes()
    }

    /// Exact configured provider version.
    pub fn provider_version(&self) -> &str {
        self.provider_version.as_str()
    }

    /// Full immutable launch operation key.
    pub const fn launch_operation(&self) -> OperationKey {
        self.launch_operation
    }

    /// Pinned provider executable digest.
    pub const fn executable_fingerprint(&self) -> ProviderExecutableFingerprint {
        self.executable
    }

    /// Optional canonical executable path bytes.
    pub fn canonical_executable_path(&self) -> Option<&[u8]> {
        self.canonical_executable_path.as_deref()
    }
}

impl std::fmt::Debug for ManagedProviderScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagedProviderScope { <redacted> }")
    }
}

/// Lifecycle marker captured by a managed launch record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedLifecycleSnapshot {
    /// The launch was observed running when this historical record was formed.
    /// On load this is historical metadata, never a live-process claim.
    Running,
}

/// Known outcome retained for the managed launch operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedLaunchOutcome {
    /// The launch operation completed successfully.
    Applied,
    /// Completion was uncertain; the child must be retained as historical and
    /// must never be respawned automatically from this record.
    Unknown,
}

/// Combined managed launch metadata and validated provider binding.
pub struct ManagedProviderRecord {
    scope: ManagedProviderScope,
    lifecycle: ManagedLifecycleSnapshot,
    outcome: ManagedLaunchOutcome,
    binding: Binding,
}

impl ManagedProviderRecord {
    /// Combine one scope and binding after validating every shared field.
    pub fn new(
        scope: ManagedProviderScope,
        lifecycle: ManagedLifecycleSnapshot,
        outcome: ManagedLaunchOutcome,
        binding: Binding,
    ) -> io::Result<Self> {
        check_binding_scope(&binding, &scope)?;
        Ok(Self {
            scope,
            lifecycle,
            outcome,
            binding,
        })
    }

    /// Managed metadata scope included in the same atomic publication.
    pub fn scope(&self) -> &ManagedProviderScope {
        &self.scope
    }

    /// Historical lifecycle snapshot. This is not current process status.
    pub const fn lifecycle(&self) -> ManagedLifecycleSnapshot {
        self.lifecycle
    }

    /// Retained launch operation outcome.
    pub const fn outcome(&self) -> ManagedLaunchOutcome {
        self.outcome
    }

    /// Exact validated provider binding inside the combined record.
    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    /// Consume the record and return its validated binding.
    pub fn into_binding(self) -> Binding {
        self.binding
    }
}

impl std::fmt::Debug for ManagedProviderRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagedProviderRecord { <redacted> }")
    }
}

/// Outcome of one atomic managed record publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedPublishOutcome {
    /// New combined bytes were published and the directory synced.
    Written,
    /// The exact combined bytes were already present and the directory synced.
    AlreadyDurable,
}

/// Locked single-writer store for one exact managed target and provider.
pub struct ManagedProviderStore {
    slot: AtomicPrivateRecordStore,
    scope: ManagedProviderScope,
}

impl std::fmt::Debug for ManagedProviderStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagedProviderStore { <redacted> }")
    }
}

impl ManagedProviderStore {
    /// Open the private directory and take its lifetime exclusive writer lock.
    pub fn open(path: &Path, scope: ManagedProviderScope) -> io::Result<Self> {
        let slot = AtomicPrivateRecordStore::open(
            path,
            CURRENT_NAME,
            TEMP_NAME,
            LOCK_NAME,
            MAX_MANAGED_PROVIDER_RECORD_BYTES,
        )?;
        Ok(Self { slot, scope })
    }

    /// Read one exact historical record while holding the store lock.
    ///
    /// This inspection-only API is for a caller that knows the complete old
    /// target, project, provider version, and pinned executable digest but no
    /// longer has the launch lease/sequence. It never scans or picks a latest
    /// record, and it returns no writer handle.
    pub fn inspect_historical(
        path: &Path,
        target: TargetRef,
        project: &[u8],
        provider_version: &str,
        expected_executable_sha256: [u8; 32],
    ) -> io::Result<Option<ManagedProviderRecord>> {
        if !valid_target(target) {
            return Err(refused());
        }
        let project = ProjectIdentity::new(project).map_err(|_| refused())?;
        let provider_version = SupportedVersion::new(provider_version).map_err(|_| refused())?;
        let expected_executable = ProviderExecutableFingerprint::new(expected_executable_sha256)?;
        let slot = AtomicPrivateRecordStore::open(
            path,
            CURRENT_NAME,
            TEMP_NAME,
            LOCK_NAME,
            MAX_MANAGED_PROVIDER_RECORD_BYTES,
        )?;
        let Some(bytes) = slot.read()? else {
            return Ok(None);
        };
        let record = decode(&bytes)?;
        if record.scope.target != target
            || record.scope.project != project
            || record.scope.provider_version != provider_version
            || record.scope.executable != expected_executable
        {
            return Err(invalid_record());
        }
        Ok(Some(record))
    }

    /// Load the exact historical combined record, if one exists.
    pub fn load(&self) -> io::Result<Option<ManagedProviderRecord>> {
        let Some(bytes) = self.slot.read()? else {
            return Ok(None);
        };
        let record = decode(&bytes)?;
        if record.scope != self.scope {
            return Err(invalid_record());
        }
        Ok(Some(record))
    }

    /// Atomically publish lifecycle metadata and binding as one record.
    ///
    /// Binding revisions cannot move backward. Reusing a revision with any
    /// changed bytes is refused. Directory-sync ambiguity disables this store
    /// until reopen, and callers must not acknowledge on an error.
    pub fn publish(&mut self, record: &ManagedProviderRecord) -> io::Result<ManagedPublishOutcome> {
        if record.scope != self.scope {
            return Err(refused());
        }
        check_binding_scope(&record.binding, &self.scope)?;
        let bytes = encode(record)?;
        if let Some(old_bytes) = self.slot.read()? {
            let old = decode(&old_bytes)?;
            if old.scope != self.scope {
                return Err(invalid_record());
            }
            if record.binding.revision() < old.binding.revision()
                || (record.binding.revision() == old.binding.revision() && bytes != old_bytes)
            {
                return Err(refused());
            }
        }
        self.slot.publish(&bytes).map(|outcome| match outcome {
            PublishOutcome::Written => ManagedPublishOutcome::Written,
            PublishOutcome::AlreadyDurable => ManagedPublishOutcome::AlreadyDurable,
        })
    }

    /// Explicitly discard a recognized interrupted temp; it is never promoted.
    pub fn discard_pending(&self) -> io::Result<bool> {
        self.slot.discard_pending()
    }
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

fn check_binding_scope(binding: &Binding, scope: &ManagedProviderScope) -> io::Result<()> {
    if binding.target() != scope.target
        || binding.project_identity() != scope.project.as_bytes()
        || binding.supported_version() != scope.provider_version.as_str()
    {
        return Err(refused());
    }
    if binding.bridge_instance() == 0 || binding.revision() == 0 {
        return Err(refused());
    }
    Ok(())
}

fn validate_canonical_path(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > MAX_EXECUTABLE_PATH_BYTES || bytes.contains(&0) {
        return Err(refused());
    }
    if !bytes.starts_with(b"/")
        || (bytes.len() > 1 && bytes.ends_with(b"/"))
        || bytes.windows(2).any(|part| part == b"//")
        || bytes
            .split(|byte| *byte == b'/')
            .any(|component| component == b"." || component == b"..")
    {
        return Err(refused());
    }
    Ok(bytes.to_vec())
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "managed provider record refused",
    )
}
fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid managed provider record",
    )
}

fn encode(record: &ManagedProviderRecord) -> io::Result<Vec<u8>> {
    check_binding_scope(&record.binding, &record.scope)?;
    let binding = provider_binding_codec::encode(&record.binding).map_err(|_| refused())?;
    let path = record
        .scope
        .canonical_executable_path
        .as_deref()
        .unwrap_or_default();
    let path_len = u16::try_from(path.len()).map_err(|_| refused())?;
    let binding_len = u32::try_from(binding.len()).map_err(|_| refused())?;
    let total = HEADER_BYTES
        .checked_add(path.len())
        .and_then(|n| n.checked_add(binding.len()))
        .filter(|n| *n <= MAX_MANAGED_PROVIDER_RECORD_BYTES)
        .ok_or_else(refused)?;
    let key = record.scope.launch_operation;
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&key.generation.to_be_bytes());
    bytes.extend_from_slice(&key.lease.to_be_bytes());
    bytes.extend_from_slice(&key.sequence.to_be_bytes());
    bytes.extend_from_slice(record.scope.executable.as_bytes());
    bytes.push(match record.lifecycle {
        ManagedLifecycleSnapshot::Running => 1,
    });
    bytes.push(match record.outcome {
        ManagedLaunchOutcome::Applied => 1,
        ManagedLaunchOutcome::Unknown => 2,
    });
    bytes.extend_from_slice(&path_len.to_be_bytes());
    bytes.extend_from_slice(&binding_len.to_be_bytes());
    debug_assert_eq!(bytes.len(), HEADER_BYTES);
    bytes.extend_from_slice(path);
    bytes.extend_from_slice(&binding);
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> io::Result<ManagedProviderRecord> {
    if bytes.len() < HEADER_BYTES
        || bytes.len() > MAX_MANAGED_PROVIDER_RECORD_BYTES
        || &bytes[..8] != MAGIC
    {
        return Err(invalid_record());
    }
    let key = OperationKey {
        generation: read_u64(bytes, 8)?,
        lease: read_u64(bytes, 16)?,
        sequence: read_u64(bytes, 24)?,
    };
    let mut fingerprint = [0; 32];
    fingerprint.copy_from_slice(&bytes[32..64]);
    let lifecycle = match bytes[64] {
        1 => ManagedLifecycleSnapshot::Running,
        _ => return Err(invalid_record()),
    };
    let outcome = match bytes[65] {
        1 => ManagedLaunchOutcome::Applied,
        2 => ManagedLaunchOutcome::Unknown,
        _ => return Err(invalid_record()),
    };
    let path_len = u16::from_be_bytes([bytes[66], bytes[67]]) as usize;
    let binding_len = u32::from_be_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]) as usize;
    let expected = HEADER_BYTES
        .checked_add(path_len)
        .and_then(|n| n.checked_add(binding_len))
        .ok_or_else(invalid_record)?;
    if expected != bytes.len()
        || binding_len > MAX_BINDING_RECORD_BYTES
        || path_len > MAX_EXECUTABLE_PATH_BYTES
    {
        return Err(invalid_record());
    }
    let path_bytes = &bytes[HEADER_BYTES..HEADER_BYTES + path_len];
    let binding = provider_binding_codec::decode(&bytes[HEADER_BYTES + path_len..])
        .map_err(|_| invalid_record())?;
    let scope = ManagedProviderScope::new(
        binding.target(),
        binding.project_identity(),
        binding.supported_version(),
        key,
        fingerprint,
        (!path_bytes.is_empty()).then_some(path_bytes),
    )
    .map_err(|_| invalid_record())?;
    ManagedProviderRecord::new(scope, lifecycle, outcome, binding).map_err(|_| invalid_record())
}

fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let value = bytes.get(offset..offset + 8).ok_or_else(invalid_record)?;
    Ok(u64::from_be_bytes(
        value.try_into().map_err(|_| invalid_record())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::VerificationSource;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: std::path::PathBuf,
        scope: ManagedProviderScope,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = Path::new("/private/tmp").join(format!("panelink-managed-provider-{nonce}"));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let target = TargetRef {
                generation: 1,
                workspace: 2,
                pane: 3,
                session: 4,
                occupant_revision: 5,
            };
            let scope = ManagedProviderScope::new(
                target,
                b"/private/project",
                "2.0.32",
                OperationKey {
                    generation: 1,
                    lease: 6,
                    sequence: 7,
                },
                [1; 32],
                Some(b"/usr/local/bin/opencode"),
            )
            .unwrap();
            Self { root, scope }
        }
        fn binding(&self, revision: u64, id: &str) -> Binding {
            Binding::new(
                self.scope.target,
                &self.scope.project,
                &self.scope.provider_version,
                8,
                revision,
                id,
                10_000 + revision,
                VerificationSource::TuiRouteSnapshot,
            )
            .unwrap()
        }
        fn record(
            &self,
            revision: u64,
            id: &str,
            outcome: ManagedLaunchOutcome,
        ) -> ManagedProviderRecord {
            ManagedProviderRecord::new(
                self.scope.clone(),
                ManagedLifecycleSnapshot::Running,
                outcome,
                self.binding(revision, id),
            )
            .unwrap()
        }
        fn store(&self) -> ManagedProviderStore {
            ManagedProviderStore::open(&self.root, self.scope.clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn managed_fields_and_binding_load_from_one_historical_slot() {
        let fixture = Fixture::new();
        let record = fixture.record(1, "session-a", ManagedLaunchOutcome::Applied);
        let mut store = fixture.store();
        assert!(store.load().unwrap().is_none());
        assert_eq!(
            store.publish(&record).unwrap(),
            ManagedPublishOutcome::Written
        );
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.binding(), record.binding());
        assert_eq!(
            loaded.scope().launch_operation(),
            fixture.scope.launch_operation()
        );
        assert_eq!(loaded.outcome(), ManagedLaunchOutcome::Applied);
        assert_eq!(loaded.lifecycle(), ManagedLifecycleSnapshot::Running);
    }

    #[test]
    fn unknown_outcome_is_preserved_as_historical_and_never_inferred_live() {
        let fixture = Fixture::new();
        let record = fixture.record(1, "session-a", ManagedLaunchOutcome::Unknown);
        let mut store = fixture.store();
        store.publish(&record).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.outcome(), ManagedLaunchOutcome::Unknown);
        assert_eq!(loaded.lifecycle(), ManagedLifecycleSnapshot::Running);
    }

    #[test]
    fn historical_inspection_needs_exact_target_project_version_and_pin() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .publish(&fixture.record(1, "session-a", ManagedLaunchOutcome::Applied))
            .unwrap();
        drop(store);
        let loaded = ManagedProviderStore::inspect_historical(
            &fixture.root,
            fixture.scope.target,
            fixture.scope.project.as_bytes(),
            fixture.scope.provider_version.as_str(),
            *fixture.scope.executable.as_bytes(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.binding().session_id(), "session-a");
        assert!(ManagedProviderStore::inspect_historical(
            &fixture.root,
            TargetRef {
                pane: 99,
                ..fixture.scope.target
            },
            fixture.scope.project.as_bytes(),
            fixture.scope.provider_version.as_str(),
            *fixture.scope.executable.as_bytes(),
        )
        .is_err());
        assert!(ManagedProviderStore::inspect_historical(
            &fixture.root,
            fixture.scope.target,
            b"/foreign/project",
            fixture.scope.provider_version.as_str(),
            *fixture.scope.executable.as_bytes(),
        )
        .is_err());
        assert!(ManagedProviderStore::inspect_historical(
            &fixture.root,
            fixture.scope.target,
            fixture.scope.project.as_bytes(),
            fixture.scope.provider_version.as_str(),
            [2; 32],
        )
        .is_err());
    }

    #[test]
    fn revision_is_monotonic_and_reused_revision_must_be_byte_identical() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let one = fixture.record(2, "session-a", ManagedLaunchOutcome::Applied);
        store.publish(&one).unwrap();
        assert_eq!(
            store.publish(&one).unwrap(),
            ManagedPublishOutcome::AlreadyDurable
        );
        assert!(store
            .publish(&fixture.record(1, "session-b", ManagedLaunchOutcome::Applied))
            .is_err());
        assert!(store
            .publish(&fixture.record(2, "session-b", ManagedLaunchOutcome::Applied))
            .is_err());
        assert_eq!(store.load().unwrap().unwrap().binding(), one.binding());
    }

    #[test]
    fn scope_and_launch_key_are_validated_before_publication() {
        let fixture = Fixture::new();
        let mut wrong_scope = fixture.scope.clone();
        wrong_scope.launch_operation.sequence += 1;
        let record = ManagedProviderRecord::new(
            wrong_scope,
            ManagedLifecycleSnapshot::Running,
            ManagedLaunchOutcome::Applied,
            fixture.binding(1, "session-a"),
        )
        .unwrap();
        assert!(fixture.store().publish(&record).is_err());
        let wrong_generation = ManagedProviderScope::new(
            fixture.scope.target,
            b"/private/project",
            "2.0.32",
            OperationKey {
                generation: 9,
                ..fixture.scope.launch_operation
            },
            [2; 32],
            None,
        );
        assert!(wrong_generation.is_err());
    }

    #[test]
    fn corruption_and_wrong_executable_scope_are_refused_without_rewrite() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .publish(&fixture.record(1, "session-a", ManagedLaunchOutcome::Applied))
            .unwrap();
        drop(store);
        let path = fixture.root.join(CURRENT_NAME);
        let original = fs::read(&path).unwrap();
        let mut wrong = fixture.scope.clone();
        wrong.executable = ProviderExecutableFingerprint::new([2; 32]).unwrap();
        assert!(ManagedProviderStore::open(&fixture.root, wrong)
            .unwrap()
            .load()
            .is_err());
        fs::write(&path, b"corrupt").unwrap();
        assert!(fixture.store().load().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt");
        drop(fs::write(&path, original));
    }

    #[test]
    fn invalid_paths_and_zero_fingerprint_are_rejected() {
        let fixture = Fixture::new();
        assert!(ManagedProviderScope::new(
            fixture.scope.target,
            b"/private/project",
            "2.0.32",
            fixture.scope.launch_operation,
            [0; 32],
            None
        )
        .is_err());
        assert!(ManagedProviderScope::new(
            fixture.scope.target,
            b"/private/project",
            "2.0.32",
            fixture.scope.launch_operation,
            [1; 32],
            Some(b"relative/opencode")
        )
        .is_err());
        assert!(ManagedProviderScope::new(
            fixture.scope.target,
            b"/private/project",
            "2.0.32",
            fixture.scope.launch_operation,
            [1; 32],
            Some(b"/bin/../opencode")
        )
        .is_err());
    }

    #[test]
    fn sync_ambiguity_disables_writer_and_keeps_new_slot_unacknowledged() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .publish(&fixture.record(1, "session-a", ManagedLaunchOutcome::Applied))
            .unwrap();
        store
            .slot
            .inject_test_fault(crate::private_record_store::FaultPoint::DirectorySync);
        assert!(store
            .publish(&fixture.record(2, "session-b", ManagedLaunchOutcome::Applied))
            .is_err());
        assert!(store.load().is_err());
        drop(store);
        assert_eq!(
            fixture
                .store()
                .load()
                .unwrap()
                .unwrap()
                .binding()
                .revision(),
            2
        );
    }

    #[test]
    fn committed_slot_is_private_single_link_and_temp_is_never_promoted() {
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
}
