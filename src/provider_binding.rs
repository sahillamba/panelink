//! Pure state machine for authoritative top-level provider-session bindings.
//!
//! This module validates bounded identity values and orders observations. It
//! does not discover providers, canonicalize project paths, persist records,
//! launch processes, or resume conversations. The caller supplies a trusted
//! selected top-level observation and owns the durable-writer contract.

use crate::control_protocol::TargetRef;
use std::error::Error;
use std::fmt;

/// Maximum opaque byte length for a caller-canonicalized project identity.
pub const MAX_PROJECT_IDENTITY_BYTES: usize = 4_096;
/// Maximum byte length for a provider session identifier.
pub const MAX_PROVIDER_SESSION_ID_BYTES: usize = 128;
/// Maximum ASCII byte length for a supported provider version string.
pub const MAX_PROVIDER_VERSION_BYTES: usize = 64;

/// Resulting lifecycle state for one configured provider bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingState {
    /// No authoritative selected-session observation has been accepted.
    Unbound,
    /// A newer selected-session observation awaits durable acknowledgment.
    Pending,
    /// The current bridge's selected binding was durably acknowledged.
    Verified,
    /// Current identity cannot be claimed; the last confirmed record remains.
    Stale,
    /// Observations disagree or are otherwise ambiguous.
    Ambiguous,
    /// The configured bridge is unavailable; the last confirmed record remains.
    Unavailable,
}

/// Source used to verify the selected top-level session identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationSource {
    /// Snapshot of the provider TUI's current route.
    TuiRouteSnapshot,
    /// Provider observation ordered with an explicit selection operation.
    OrderedSelection,
}

/// Whether an observation describes the displayed top-level session or child activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationScope {
    /// The selected conversation shown by the top-level provider TUI.
    SelectedTopLevel,
    /// A child/subagent activity that must never replace the displayed binding.
    ChildActivity,
}

/// Outcome of submitting a valid observation to the state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserveOutcome {
    /// A newer binding is now pending durable acknowledgment.
    Pending,
    /// The same revision and every binding field were already observed.
    Duplicate,
}

/// Fixed error categories. No variant carries untrusted text or identity bytes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum BindingError {
    /// A target identity contains a zero field.
    InvalidTarget,
    /// A project identity is empty, oversized, or contains NUL.
    InvalidProjectIdentity,
    /// A provider version has invalid characters, or is empty/oversized.
    InvalidProviderVersion,
    /// A provider session ID is outside its bounded ASCII alphabet.
    InvalidSessionId,
    /// A bridge instance identifier must be nonzero.
    InvalidBridgeInstance,
    /// An observation revision must be nonzero.
    InvalidRevision,
    /// An observation verification timestamp must be nonzero.
    InvalidVerificationTime,
    /// An observation's complete target differs from the configured target.
    WrongTarget,
    /// An observation's project identity differs from the configured identity.
    WrongProject,
    /// An observation's provider version differs from the configured version.
    WrongProviderVersion,
    /// An observation belongs to a different bridge instance.
    WrongBridge,
    /// Child activity cannot update the selected top-level binding.
    ChildObservation,
    /// An observation revision is lower than the latest accepted revision.
    StaleObservation,
    /// The same revision was reused with different fields.
    RevisionConflict,
    /// There is no currently pending binding to confirm.
    NoPendingBinding,
    /// The acknowledgment names a revision other than the current pending one.
    StaleAcknowledgment,
    /// The external durable-writer callback failed.
    StorageFailure,
}

impl fmt::Debug for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingError { <redacted> }")
    }
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTarget
            | Self::InvalidProjectIdentity
            | Self::InvalidProviderVersion
            | Self::InvalidSessionId
            | Self::InvalidBridgeInstance
            | Self::InvalidRevision
            | Self::InvalidVerificationTime => "invalid provider binding",
            Self::WrongTarget
            | Self::WrongProject
            | Self::WrongProviderVersion
            | Self::WrongBridge => "provider observation does not match configured identity",
            Self::ChildObservation => "provider observation is not top-level",
            Self::StaleObservation => "stale provider observation",
            Self::RevisionConflict => "provider revision conflicts with prior observation",
            Self::NoPendingBinding => "no pending provider binding",
            Self::StaleAcknowledgment => "stale provider acknowledgment",
            Self::StorageFailure => "provider binding storage failed",
        };
        f.write_str(message)
    }
}

impl Error for BindingError {}

/// Validated opaque identity for the expected canonical project.
#[derive(Clone, Eq, PartialEq)]
pub struct ProjectIdentity(Vec<u8>);

impl ProjectIdentity {
    /// Validate and copy a nonempty, NUL-free project identity of at most 4 KiB.
    pub fn new(bytes: &[u8]) -> Result<Self, BindingError> {
        if bytes.is_empty() || bytes.len() > MAX_PROJECT_IDENTITY_BYTES || bytes.contains(&0) {
            return Err(BindingError::InvalidProjectIdentity);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Borrow the opaque identity bytes without interpreting or normalizing them.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ProjectIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProjectIdentity { <redacted> }")
    }
}

/// Validated provider version string selected by the outer adapter contract.
#[derive(Clone, Eq, PartialEq)]
pub struct SupportedVersion(String);

impl SupportedVersion {
    /// Validate and copy a bounded version token using ASCII alphanumerics, `.`, `+`, or `-`.
    pub fn new(version: &str) -> Result<Self, BindingError> {
        if version.is_empty()
            || version.len() > MAX_PROVIDER_VERSION_BYTES
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
        {
            return Err(BindingError::InvalidProviderVersion);
        }
        Ok(Self(version.to_owned()))
    }

    /// Borrow the exact configured version string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SupportedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SupportedVersion { <redacted> }")
    }
}

/// One validated, owned selected top-level session binding.
///
/// Values are bounded and immutable through the public API. Construction does
/// not prove that a provider supplied the values or that a writer persisted them.
#[derive(Eq, PartialEq)]
pub struct Binding {
    target: TargetRef,
    project_identity: ProjectIdentity,
    supported_version: SupportedVersion,
    bridge_instance: u64,
    revision: u64,
    session_id: String,
    verified_at_unix_ms: u64,
    source: VerificationSource,
}

impl Binding {
    /// Validate and own all fields of one selected-session binding.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target: TargetRef,
        project_identity: &ProjectIdentity,
        supported_version: &SupportedVersion,
        bridge_instance: u64,
        revision: u64,
        session_id: &str,
        verified_at_unix_ms: u64,
        source: VerificationSource,
    ) -> Result<Self, BindingError> {
        validate_target(target)?;
        if bridge_instance == 0 {
            return Err(BindingError::InvalidBridgeInstance);
        }
        if revision == 0 {
            return Err(BindingError::InvalidRevision);
        }
        if verified_at_unix_ms == 0 {
            return Err(BindingError::InvalidVerificationTime);
        }
        validate_session_id(session_id)?;
        Ok(Self {
            target,
            project_identity: project_identity.clone(),
            supported_version: supported_version.clone(),
            bridge_instance,
            revision,
            session_id: session_id.to_owned(),
            verified_at_unix_ms,
            source,
        })
    }

    /// Complete immutable target reference, including occupant revision.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Opaque project identity bytes supplied by the caller.
    pub fn project_identity(&self) -> &[u8] {
        self.project_identity.as_bytes()
    }

    /// Exact provider version string configured for this binding.
    pub fn supported_version(&self) -> &str {
        self.supported_version.as_str()
    }

    /// Nonzero bridge instance that reported this binding.
    pub const fn bridge_instance(&self) -> u64 {
        self.bridge_instance
    }

    /// Nonzero monotonically increasing revision within the bridge instance.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Exact validated top-level provider session identifier.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Unix timestamp in milliseconds supplied with the verification.
    pub const fn verified_at_unix_ms(&self) -> u64 {
        self.verified_at_unix_ms
    }

    /// Fixed adapter verification source.
    pub const fn source(&self) -> VerificationSource {
        self.source
    }
}

impl fmt::Debug for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Binding { <redacted> }")
    }
}

/// Validated record supplied to restoration as the prior last-confirmed value.
///
/// The storage adapter must create this only from its durable last-confirmed
/// field. This wrapper validates shape only; it cannot prove storage provenance.
pub struct LastConfirmedRecord(Binding);

impl LastConfirmedRecord {
    /// Wrap a previously validated binding decoded from the last-confirmed slot.
    pub fn from_validated_binding(binding: Binding) -> Self {
        Self(binding)
    }

    /// Borrow the record for a storage adapter or audit view.
    pub fn binding(&self) -> &Binding {
        &self.0
    }
}

impl fmt::Debug for LastConfirmedRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LastConfirmedRecord { <redacted> }")
    }
}

/// Validated observation presented to the state machine.
pub struct BindingObservation {
    scope: ObservationScope,
    binding: Binding,
}

impl BindingObservation {
    /// Validate a top-level or child observation without inferring either scope.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: ObservationScope,
        target: TargetRef,
        project_identity: &ProjectIdentity,
        supported_version: &SupportedVersion,
        bridge_instance: u64,
        revision: u64,
        session_id: &str,
        verified_at_unix_ms: u64,
        source: VerificationSource,
    ) -> Result<Self, BindingError> {
        Ok(Self {
            scope,
            binding: Binding::new(
                target,
                project_identity,
                supported_version,
                bridge_instance,
                revision,
                session_id,
                verified_at_unix_ms,
                source,
            )?,
        })
    }
}

impl fmt::Debug for BindingObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingObservation { <redacted> }")
    }
}

/// Pure provider identity state machine for one target, project, version, and bridge.
pub struct ProviderBinding {
    target: TargetRef,
    expected_project: ProjectIdentity,
    supported_version: SupportedVersion,
    active_bridge_instance: u64,
    state: BindingState,
    last_confirmed: Option<Binding>,
    /// Latest accepted observation; retained after invalidation only as a bounded
    /// revision/conflict watermark. It is pending only while state is `Pending`.
    latest_observation: Option<Binding>,
}

impl ProviderBinding {
    /// Configure one complete target, canonical project identity, version, and bridge.
    pub fn new(
        target: TargetRef,
        expected_project: &[u8],
        supported_version: &str,
        active_bridge_instance: u64,
    ) -> Result<Self, BindingError> {
        validate_target(target)?;
        if active_bridge_instance == 0 {
            return Err(BindingError::InvalidBridgeInstance);
        }
        Ok(Self {
            target,
            expected_project: ProjectIdentity::new(expected_project)?,
            supported_version: SupportedVersion::new(supported_version)?,
            active_bridge_instance,
            state: BindingState::Unbound,
            last_confirmed: None,
            latest_observation: None,
        })
    }

    /// Restore a validated last-confirmed record as stale, never as current.
    ///
    /// A record from an earlier bridge instance remains available as last
    /// confirmed and remains the revision watermark. The outer adapter must
    /// continue revision order across bridge replacement.
    pub fn restore(
        target: TargetRef,
        expected_project: &[u8],
        supported_version: &str,
        active_bridge_instance: u64,
        record: LastConfirmedRecord,
    ) -> Result<Self, BindingError> {
        let mut binding = Self::new(
            target,
            expected_project,
            supported_version,
            active_bridge_instance,
        )?;
        if record.0.target != binding.target {
            return Err(BindingError::WrongTarget);
        }
        if record.0.project_identity != binding.expected_project {
            return Err(BindingError::WrongProject);
        }
        if record.0.supported_version != binding.supported_version {
            return Err(BindingError::WrongProviderVersion);
        }
        binding.last_confirmed = Some(record.0);
        binding.state = BindingState::Stale;
        Ok(binding)
    }

    /// Current lifecycle state.
    pub const fn state(&self) -> BindingState {
        self.state
    }

    /// Return the pending binding only while it awaits durable acknowledgment.
    pub fn pending(&self) -> Option<&Binding> {
        if self.state == BindingState::Pending {
            self.latest_observation.as_ref()
        } else {
            None
        }
    }

    /// Return the most recent successfully acknowledged binding, if any.
    pub fn last_confirmed(&self) -> Option<&Binding> {
        self.last_confirmed.as_ref()
    }

    /// Return the binding only when the current state is verified.
    pub fn verified_current(&self) -> Option<&Binding> {
        if self.state == BindingState::Verified {
            self.last_confirmed.as_ref()
        } else {
            None
        }
    }

    /// Accept an explicit observation or reject it without changing state.
    pub fn observe(
        &mut self,
        observation: BindingObservation,
    ) -> Result<ObserveOutcome, BindingError> {
        if observation.scope != ObservationScope::SelectedTopLevel {
            return Err(BindingError::ChildObservation);
        }
        let observed = &observation.binding;
        if observed.bridge_instance != self.active_bridge_instance {
            return Err(BindingError::WrongBridge);
        }
        if observed.target != self.target {
            return Err(BindingError::WrongTarget);
        }
        if observed.project_identity != self.expected_project {
            return Err(BindingError::WrongProject);
        }
        if observed.supported_version != self.supported_version {
            return Err(BindingError::WrongProviderVersion);
        }

        let last_seen = self
            .latest_observation
            .as_ref()
            .or(self.last_confirmed.as_ref());
        if let Some(previous) = last_seen {
            if observed.revision < previous.revision {
                return Err(BindingError::StaleObservation);
            }
            if observed.revision == previous.revision {
                if observed == previous {
                    return Ok(ObserveOutcome::Duplicate);
                }
                return Err(BindingError::RevisionConflict);
            }
        }

        self.latest_observation = Some(observation.binding);
        self.state = BindingState::Pending;
        Ok(ObserveOutcome::Pending)
    }

    /// Persist the current pending value before promoting it to verified.
    ///
    /// The callback receives an immutable bounded value. Its successful return
    /// is the caller's durability contract; this state machine cannot inspect
    /// or prove the storage implementation. Callback errors are replaced with
    /// the fixed `StorageFailure` category.
    pub fn confirm_pending<F, E>(
        &mut self,
        revision: u64,
        durable_writer: F,
    ) -> Result<(), BindingError>
    where
        F: FnOnce(&Binding) -> Result<(), E>,
    {
        if revision == 0 {
            return Err(BindingError::InvalidRevision);
        }
        let Some(pending) = self.pending() else {
            return Err(BindingError::NoPendingBinding);
        };
        if pending.revision != revision {
            return Err(BindingError::StaleAcknowledgment);
        }
        if durable_writer(pending).is_err() {
            return Err(BindingError::StorageFailure);
        }
        self.last_confirmed = self.latest_observation.take();
        self.state = BindingState::Verified;
        Ok(())
    }

    /// Mark current identity stale while retaining last confirmation and revision watermark.
    pub fn mark_stale(&mut self) {
        self.state = BindingState::Stale;
    }

    /// Mark identity ambiguous while retaining last confirmation and revision watermark.
    pub fn mark_ambiguous(&mut self) {
        self.state = BindingState::Ambiguous;
    }

    /// Mark the bridge unavailable while retaining last confirmation and revision watermark.
    pub fn mark_unavailable(&mut self) {
        self.state = BindingState::Unavailable;
    }
}

impl fmt::Debug for ProviderBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderBinding { <redacted> }")
    }
}

fn validate_target(target: TargetRef) -> Result<(), BindingError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(BindingError::InvalidTarget);
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<(), BindingError> {
    let bytes = session_id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_PROVIDER_SESSION_ID_BYTES
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err(BindingError::InvalidSessionId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &[u8] = b"canonical-project-v1";
    const VERSION: &str = "1.18.32";
    const BRIDGE: u64 = 41;

    fn target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 12,
            pane: 13,
            session: 14,
            occupant_revision: 15,
        }
    }

    fn configured() -> ProviderBinding {
        ProviderBinding::new(target(), PROJECT, VERSION, BRIDGE).unwrap()
    }

    fn observation(revision: u64) -> BindingObservation {
        observation_with(
            ObservationScope::SelectedTopLevel,
            target(),
            PROJECT,
            VERSION,
            BRIDGE,
            revision,
            "session_abc-1.2",
            1_760_000_000_000 + revision,
            VerificationSource::TuiRouteSnapshot,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn observation_with(
        scope: ObservationScope,
        target: TargetRef,
        project: &[u8],
        version: &str,
        bridge: u64,
        revision: u64,
        session_id: &str,
        verified_at: u64,
        source: VerificationSource,
    ) -> BindingObservation {
        let project = ProjectIdentity::new(project).unwrap();
        let version = SupportedVersion::new(version).unwrap();
        BindingObservation::new(
            scope,
            target,
            &project,
            &version,
            bridge,
            revision,
            session_id,
            verified_at,
            source,
        )
        .unwrap()
    }

    fn binding(revision: u64, bridge: u64) -> Binding {
        let project = ProjectIdentity::new(PROJECT).unwrap();
        let version = SupportedVersion::new(VERSION).unwrap();
        Binding::new(
            target(),
            &project,
            &version,
            bridge,
            revision,
            "session_abc-1.2",
            1_760_000_000_000 + revision,
            VerificationSource::OrderedSelection,
        )
        .unwrap()
    }

    #[test]
    fn new_observation_is_pending_until_durable_writer_succeeds() {
        let mut state = configured();
        assert_eq!(state.state(), BindingState::Unbound);
        assert!(state.pending().is_none());
        assert!(state.last_confirmed().is_none());
        assert!(state.verified_current().is_none());

        assert_eq!(
            state.observe(observation(1)).unwrap(),
            ObserveOutcome::Pending
        );
        assert_eq!(state.state(), BindingState::Pending);
        assert_eq!(state.pending().unwrap().revision(), 1);
        assert!(state.verified_current().is_none());

        let mut called = false;
        state
            .confirm_pending(1, |pending| {
                called = true;
                assert_eq!(pending.session_id(), "session_abc-1.2");
                assert_eq!(pending.source(), VerificationSource::TuiRouteSnapshot);
                Ok::<(), ()>(())
            })
            .unwrap();
        assert!(called);
        assert_eq!(state.state(), BindingState::Verified);
        assert!(state.pending().is_none());
        assert_eq!(state.last_confirmed().unwrap().revision(), 1);
        assert_eq!(state.verified_current().unwrap().revision(), 1);
    }

    #[test]
    fn writer_failure_preserves_old_confirmation_and_new_pending_value() {
        let mut state = configured();
        state.observe(observation(1)).unwrap();
        state.confirm_pending(1, |_| Ok::<(), ()>(())).unwrap();
        state.observe(observation(2)).unwrap();

        let error = state
            .confirm_pending(2, |_| Err::<(), _>("private storage detail"))
            .unwrap_err();
        assert_eq!(error, BindingError::StorageFailure);
        assert_eq!(state.state(), BindingState::Pending);
        assert_eq!(state.pending().unwrap().revision(), 2);
        assert_eq!(state.last_confirmed().unwrap().revision(), 1);
        assert!(state.verified_current().is_none());
        assert!(!error.to_string().contains("private storage detail"));
    }

    #[test]
    fn exact_duplicate_is_idempotent_changed_duplicate_conflicts_and_older_is_stale() {
        let mut state = configured();
        state.observe(observation(3)).unwrap();
        assert_eq!(
            state.observe(observation(3)).unwrap(),
            ObserveOutcome::Duplicate
        );
        assert_eq!(state.state(), BindingState::Pending);

        let changed = observation_with(
            ObservationScope::SelectedTopLevel,
            target(),
            PROJECT,
            VERSION,
            BRIDGE,
            3,
            "other-session",
            1_760_000_000_003,
            VerificationSource::TuiRouteSnapshot,
        );
        assert_eq!(state.observe(changed), Err(BindingError::RevisionConflict));
        assert_eq!(state.state(), BindingState::Pending);
        assert_eq!(state.pending().unwrap().session_id(), "session_abc-1.2");
        assert_eq!(
            state.observe(observation(2)),
            Err(BindingError::StaleObservation)
        );
        assert_eq!(state.pending().unwrap().revision(), 3);
    }

    #[test]
    fn invalid_scope_and_wrong_configuration_leave_state_unchanged() {
        let mut state = configured();
        let child = observation_with(
            ObservationScope::ChildActivity,
            target(),
            PROJECT,
            VERSION,
            BRIDGE,
            9,
            "child-session",
            1_760_000_000_009,
            VerificationSource::OrderedSelection,
        );
        assert_eq!(state.observe(child), Err(BindingError::ChildObservation));

        let wrong_target = TargetRef {
            pane: 99,
            ..target()
        };
        let wrong_project = b"another-canonical-project";
        let mismatches = [
            observation_with(
                ObservationScope::SelectedTopLevel,
                wrong_target,
                PROJECT,
                VERSION,
                BRIDGE,
                9,
                "session_abc-1.2",
                1_760_000_000_009,
                VerificationSource::OrderedSelection,
            ),
            observation_with(
                ObservationScope::SelectedTopLevel,
                target(),
                wrong_project,
                VERSION,
                BRIDGE,
                9,
                "session_abc-1.2",
                1_760_000_000_009,
                VerificationSource::OrderedSelection,
            ),
            observation_with(
                ObservationScope::SelectedTopLevel,
                target(),
                PROJECT,
                "1.18.33",
                BRIDGE,
                9,
                "session_abc-1.2",
                1_760_000_000_009,
                VerificationSource::OrderedSelection,
            ),
            observation_with(
                ObservationScope::SelectedTopLevel,
                target(),
                PROJECT,
                VERSION,
                BRIDGE + 1,
                9,
                "session_abc-1.2",
                1_760_000_000_009,
                VerificationSource::OrderedSelection,
            ),
        ];
        let expected = [
            BindingError::WrongTarget,
            BindingError::WrongProject,
            BindingError::WrongProviderVersion,
            BindingError::WrongBridge,
        ];
        for (observation, error) in mismatches.into_iter().zip(expected) {
            assert_eq!(state.observe(observation), Err(error));
            assert_eq!(state.state(), BindingState::Unbound);
            assert!(state.pending().is_none());
        }
        assert_eq!(
            state.observe(observation(1)).unwrap(),
            ObserveOutcome::Pending
        );
    }

    #[test]
    fn stale_acknowledgment_cannot_confirm_newer_pending_revision() {
        let mut state = configured();
        state.observe(observation(1)).unwrap();
        state.observe(observation(2)).unwrap();
        let mut called = false;
        assert_eq!(
            state.confirm_pending(1, |_| {
                called = true;
                Ok::<(), ()>(())
            }),
            Err(BindingError::StaleAcknowledgment)
        );
        assert!(!called);
        assert_eq!(state.state(), BindingState::Pending);
        assert_eq!(state.pending().unwrap().revision(), 2);
        state.confirm_pending(2, |_| Ok::<(), ()>(())).unwrap();
        assert_eq!(state.verified_current().unwrap().revision(), 2);
    }

    #[test]
    fn invalidation_clears_pending_view_retains_confirmation_and_accepts_newer_reconcile() {
        for invalidation in [
            BindingState::Stale,
            BindingState::Ambiguous,
            BindingState::Unavailable,
        ] {
            let mut state = configured();
            state.observe(observation(1)).unwrap();
            state.confirm_pending(1, |_| Ok::<(), ()>(())).unwrap();
            state.observe(observation(2)).unwrap();
            match invalidation {
                BindingState::Stale => state.mark_stale(),
                BindingState::Ambiguous => state.mark_ambiguous(),
                BindingState::Unavailable => state.mark_unavailable(),
                _ => unreachable!(),
            }
            assert_eq!(state.state(), invalidation);
            assert!(state.pending().is_none());
            assert!(state.verified_current().is_none());
            assert_eq!(state.last_confirmed().unwrap().revision(), 1);
            assert_eq!(
                state.observe(observation(2)).unwrap(),
                ObserveOutcome::Duplicate
            );
            assert_eq!(state.state(), invalidation);
            assert_eq!(
                state.observe(observation(3)).unwrap(),
                ObserveOutcome::Pending
            );
            assert_eq!(state.pending().unwrap().revision(), 3);
        }
    }

    #[test]
    fn restore_is_stale_and_preserves_revision_order_across_bridges() {
        let record = LastConfirmedRecord::from_validated_binding(binding(90, BRIDGE));
        let mut restored =
            ProviderBinding::restore(target(), PROJECT, VERSION, BRIDGE + 1, record).unwrap();
        assert_eq!(restored.state(), BindingState::Stale);
        assert!(restored.pending().is_none());
        assert!(restored.verified_current().is_none());
        assert_eq!(restored.last_confirmed().unwrap().revision(), 90);
        let old_revision = observation_with(
            ObservationScope::SelectedTopLevel,
            target(),
            PROJECT,
            VERSION,
            BRIDGE + 1,
            89,
            "new-bridge-session",
            1_760_000_000_089,
            VerificationSource::OrderedSelection,
        );
        assert_eq!(
            restored.observe(old_revision),
            Err(BindingError::StaleObservation)
        );
        let duplicate_revision = observation_with(
            ObservationScope::SelectedTopLevel,
            target(),
            PROJECT,
            VERSION,
            BRIDGE + 1,
            90,
            "new-bridge-session",
            1_760_000_000_090,
            VerificationSource::OrderedSelection,
        );
        assert_eq!(
            restored.observe(duplicate_revision),
            Err(BindingError::RevisionConflict)
        );
        let next_revision = observation_with(
            ObservationScope::SelectedTopLevel,
            target(),
            PROJECT,
            VERSION,
            BRIDGE + 1,
            91,
            "new-bridge-session",
            1_760_000_000_091,
            VerificationSource::OrderedSelection,
        );
        assert_eq!(
            restored.observe(next_revision).unwrap(),
            ObserveOutcome::Pending
        );
        restored.confirm_pending(91, |_| Ok::<(), ()>(())).unwrap();
        assert_eq!(restored.state(), BindingState::Verified);
        assert_eq!(restored.verified_current().unwrap().revision(), 91);

        let same_bridge = LastConfirmedRecord::from_validated_binding(binding(90, BRIDGE));
        let mut restored =
            ProviderBinding::restore(target(), PROJECT, VERSION, BRIDGE, same_bridge).unwrap();
        assert_eq!(
            restored.observe(observation(89)),
            Err(BindingError::StaleObservation)
        );
        assert_eq!(
            restored
                .observe(observation_with(
                    ObservationScope::SelectedTopLevel,
                    target(),
                    PROJECT,
                    VERSION,
                    BRIDGE,
                    90,
                    "session_abc-1.2",
                    1_760_000_000_090,
                    VerificationSource::OrderedSelection,
                ))
                .unwrap(),
            ObserveOutcome::Duplicate
        );
        assert_eq!(restored.state(), BindingState::Stale);
    }

    #[test]
    fn rejects_invalid_bounds_and_zero_ordering_fields() {
        assert_eq!(
            ProjectIdentity::new(b""),
            Err(BindingError::InvalidProjectIdentity)
        );
        assert_eq!(
            ProjectIdentity::new(&vec![b'x'; MAX_PROJECT_IDENTITY_BYTES + 1]),
            Err(BindingError::InvalidProjectIdentity)
        );
        assert_eq!(
            ProjectIdentity::new(b"bad\0project"),
            Err(BindingError::InvalidProjectIdentity)
        );
        assert!(ProjectIdentity::new(&vec![b'x'; MAX_PROJECT_IDENTITY_BYTES]).is_ok());
        assert_eq!(
            SupportedVersion::new(""),
            Err(BindingError::InvalidProviderVersion)
        );
        assert_eq!(
            SupportedVersion::new(&"x".repeat(MAX_PROVIDER_VERSION_BYTES + 1)),
            Err(BindingError::InvalidProviderVersion)
        );
        assert_eq!(
            SupportedVersion::new("é"),
            Err(BindingError::InvalidProviderVersion)
        );
        for invalid in [
            "\0v1.2.3",
            "v1.\x1b.2",
            "v1.2.3\x7f",
            "v1. 2.3",
            "v1.é.2",
            "1.2.3\n",
        ] {
            assert_eq!(
                SupportedVersion::new(invalid),
                Err(BindingError::InvalidProviderVersion),
                "accepted invalid provider version token {invalid:?}"
            );
        }
        assert!(SupportedVersion::new("1.2.3-beta.1+build.5").is_ok());
        assert!(SupportedVersion::new(&"v".repeat(MAX_PROVIDER_VERSION_BYTES)).is_ok());
        assert_eq!(
            ProviderBinding::new(
                TargetRef {
                    session: 0,
                    ..target()
                },
                PROJECT,
                VERSION,
                BRIDGE
            )
            .unwrap_err(),
            BindingError::InvalidTarget
        );
        assert_eq!(
            ProviderBinding::new(target(), PROJECT, VERSION, 0).unwrap_err(),
            BindingError::InvalidBridgeInstance
        );

        let project = ProjectIdentity::new(PROJECT).unwrap();
        let version = SupportedVersion::new(VERSION).unwrap();
        let make = |bridge, revision, session: &str, verified_at| {
            Binding::new(
                target(),
                &project,
                &version,
                bridge,
                revision,
                session,
                verified_at,
                VerificationSource::OrderedSelection,
            )
        };
        assert_eq!(
            make(0, 1, "valid", 1),
            Err(BindingError::InvalidBridgeInstance)
        );
        assert_eq!(
            make(BRIDGE, 0, "valid", 1),
            Err(BindingError::InvalidRevision)
        );
        assert_eq!(
            make(BRIDGE, 1, "valid", 0),
            Err(BindingError::InvalidVerificationTime)
        );
        assert_eq!(make(BRIDGE, 1, "", 1), Err(BindingError::InvalidSessionId));
        assert!(make(BRIDGE, 1, &"s".repeat(MAX_PROVIDER_SESSION_ID_BYTES), 1).is_ok());
        assert_eq!(
            make(BRIDGE, 1, &"s".repeat(MAX_PROVIDER_SESSION_ID_BYTES + 1), 1),
            Err(BindingError::InvalidSessionId)
        );
        assert_eq!(
            make(BRIDGE, 1, "bad/session", 1),
            Err(BindingError::InvalidSessionId)
        );
    }

    #[test]
    fn debug_and_errors_never_expose_identity_values() {
        let secret = b"private-project/path-token";
        let project = ProjectIdentity::new(secret).unwrap();
        let version = SupportedVersion::new("secret-version").unwrap();
        let binding = Binding::new(
            target(),
            &project,
            &version,
            BRIDGE,
            1,
            "session_secret-1",
            1,
            VerificationSource::TuiRouteSnapshot,
        )
        .unwrap();
        let observation = BindingObservation {
            scope: ObservationScope::SelectedTopLevel,
            binding: Binding::new(
                target(),
                &project,
                &version,
                BRIDGE,
                2,
                "session_secret-2",
                2,
                VerificationSource::OrderedSelection,
            )
            .unwrap(),
        };
        let record = LastConfirmedRecord::from_validated_binding(binding);
        let configured = ProviderBinding::new(target(), secret, "secret-version", BRIDGE).unwrap();
        for output in [
            format!("{:?}", project),
            format!("{:?}", version),
            format!("{:?}", record.binding()),
            format!("{:?}", record),
            format!("{:?}", observation),
            format!("{:?}", configured),
        ] {
            assert!(!output.contains("private-project"));
            assert!(!output.contains("secret-version"));
            assert!(!output.contains("session_secret"));
        }
        assert_eq!(
            format!("{:?}", BindingError::WrongProject),
            "BindingError { <redacted> }"
        );
        assert!(!BindingError::WrongProject
            .to_string()
            .contains("private-project"));
    }
}
