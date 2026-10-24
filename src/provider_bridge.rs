//! Typed, in-process admission contract for the opt-in provider bridge.
//!
//! This module has no wire codec, peer authentication, plugin lifecycle, or
//! server activation. A caller must supply ordered frames and explicit host
//! monotonic times. It makes no claim about provider state at a crash.

use crate::control_protocol::TargetRef;
use crate::provider_binding::{
    Binding, BindingError, BindingObservation, BindingState, LastConfirmedRecord, ObservationScope,
    ObserveOutcome, ProjectIdentity, ProviderBinding, SupportedVersion, VerificationSource,
};
use std::error::Error;
use std::fmt;

/// Version of the typed bridge frame contract.
pub const BRIDGE_PROTOCOL_VERSION: u16 = 1;
/// Exact provider release admitted by this prototype contract.
pub const SUPPORTED_PROVIDER_VERSION: &str = "2.0.32";
/// SHA-256 pinned by the opt-in native executable launcher.
pub const SUPPORTED_PROVIDER_FINGERPRINT: [u8; 32] = [
    0xe4, 0x3e, 0xc2, 0xdf, 0x6f, 0x36, 0x84, 0x2f, 0xd1, 0x7d, 0x1a, 0x71, 0xa8, 0x66, 0xac, 0xb9,
    0xa8, 0x48, 0x84, 0xb4, 0x89, 0x98, 0x83, 0x76, 0xc1, 0x9b, 0xf9, 0xa7, 0x25, 0x0d, 0xab, 0xbf,
];
/// Smallest accepted lease duration in milliseconds.
pub const MIN_LEASE_MS: u64 = 100;
/// Largest accepted lease duration in milliseconds.
pub const MAX_LEASE_MS: u64 = 60_000;

/// Caller-selected bounded lease policy. This is not a production default.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LeasePolicy {
    lease_ms: u64,
}

impl LeasePolicy {
    /// Validate a prototype lease duration in the inclusive range 100–60,000 ms.
    pub fn new(lease_ms: u64) -> Result<Self, BridgeError> {
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
            return Err(BridgeError::InvalidLeasePolicy);
        }
        Ok(Self { lease_ms })
    }

    /// Configured lease length in milliseconds.
    pub const fn lease_ms(self) -> u64 {
        self.lease_ms
    }
}

impl fmt::Debug for LeasePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LeasePolicy { <redacted> }")
    }
}

/// Whether a frame describes the top-level route or child activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameScope {
    /// Provider's selected top-level route.
    TopLevel,
    /// Activity belonging to a child or subagent route.
    ChildActivity,
}

/// Provider's selected route state carried by one frame.
#[derive(Clone, Eq, PartialEq)]
pub enum Selection {
    /// Exact selected top-level session identifier and evidence source.
    Session {
        session_id: String,
        source: VerificationSource,
    },
    /// Provider is at its Home view, which has no selected session.
    Home,
    /// A child route is selected or reported.
    Child,
    /// Selection is ambiguous or cannot be identified.
    Unknown,
    /// Provider bridge cannot currently observe a route.
    Unavailable,
}

impl Selection {
    /// Construct a bounded exact session selection.
    pub fn session(session_id: &str, source: VerificationSource) -> Result<Self, BridgeError> {
        let project = ProjectIdentity::new(b"bridge-frame-validation")
            .map_err(|_| BridgeError::InvalidFrame)?;
        let version = SupportedVersion::new(SUPPORTED_PROVIDER_VERSION)
            .map_err(|_| BridgeError::InvalidFrame)?;
        Binding::new(
            dummy_target(),
            &project,
            &version,
            1,
            1,
            session_id,
            1,
            source,
        )
        .map_err(|_| BridgeError::InvalidFrame)?;
        Ok(Self::Session {
            session_id: session_id.to_owned(),
            source,
        })
    }
}

impl fmt::Debug for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Selection { <redacted> }")
    }
}

/// Validated, bounded bridge observation. All fields are immutable externally.
#[derive(Clone, Eq, PartialEq)]
pub struct Frame {
    protocol_version: u16,
    target: TargetRef,
    project_identity: ProjectIdentity,
    provider_version: SupportedVersion,
    bridge_instance: u64,
    sequence: u64,
    scope: FrameScope,
    selection: Selection,
}

impl Frame {
    /// Validate and own a complete frame for the typed version-1 contract.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        protocol_version: u16,
        target: TargetRef,
        project_identity: &[u8],
        provider_version: &str,
        bridge_instance: u64,
        sequence: u64,
        scope: FrameScope,
        selection: Selection,
    ) -> Result<Self, BridgeError> {
        if protocol_version != BRIDGE_PROTOCOL_VERSION || bridge_instance == 0 || sequence == 0 {
            return Err(BridgeError::InvalidFrame);
        }
        if target.generation == 0
            || target.workspace == 0
            || target.pane == 0
            || target.session == 0
            || target.occupant_revision == 0
        {
            return Err(BridgeError::InvalidFrame);
        }
        let project_identity =
            ProjectIdentity::new(project_identity).map_err(|_| BridgeError::InvalidFrame)?;
        let provider_version =
            SupportedVersion::new(provider_version).map_err(|_| BridgeError::InvalidFrame)?;
        if let Selection::Session { session_id, source } = &selection {
            Binding::new(
                target,
                &project_identity,
                &provider_version,
                bridge_instance,
                sequence,
                session_id,
                1,
                *source,
            )
            .map_err(|_| BridgeError::InvalidFrame)?;
        }
        Ok(Self {
            protocol_version,
            target,
            project_identity,
            provider_version,
            bridge_instance,
            sequence,
            scope,
            selection,
        })
    }

    /// Typed protocol contract version.
    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    /// Complete immutable managed target.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Exact caller-canonicalized project identity bytes.
    pub fn project_identity(&self) -> &[u8] {
        self.project_identity.as_bytes()
    }

    /// Exact provider version carried by the observation.
    pub fn provider_version(&self) -> &str {
        self.provider_version.as_str()
    }

    /// Explicit top-level or child observation scope.
    pub const fn scope(&self) -> FrameScope {
        self.scope
    }

    /// Borrow the selected route state without copying its identity.
    pub const fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Source sequence number, starting at one for each bridge instance.
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Source bridge instance identifier.
    pub const fn bridge_instance(&self) -> u64 {
        self.bridge_instance
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Frame { <redacted> }")
    }
}

/// Fixed error categories; no error includes provider IDs, project bytes, or versions.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum BridgeError {
    /// Frame fields or protocol version are invalid.
    InvalidFrame,
    /// A selected-session observation needs a nonzero trusted wall timestamp.
    InvalidVerificationTime,
    /// Lease duration is outside the prototype's bounded range.
    InvalidLeasePolicy,
    /// Frame target, project, provider version, or bridge differs from configuration.
    WrongConfiguration,
    /// Frame sequence regressed or reused with different contents.
    SequenceConflict,
    /// Frame sequence skipped one or more source frames.
    SequenceGap,
    /// Supplied host monotonic time regressed.
    ClockRegression,
    /// Lease has expired and requires explicit reconciliation.
    LeaseExpired,
    /// Binding revision counter cannot advance without wrapping.
    RevisionExhausted,
    /// No eligible pending identity can be confirmed.
    NoEligiblePending,
    /// Acknowledgment does not name the current pending revision.
    StaleAcknowledgment,
    /// Durable writer failed; the pending identity remains unconfirmed.
    StorageFailure,
    /// Restored revision floor is lower than the restored confirmed revision.
    InvalidRevisionFloor,
    /// Host time plus the lease length cannot be represented.
    HostTimeOverflow,
    /// Underlying binding core rejected an observation.
    BindingRejected,
}

impl fmt::Debug for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeError { <redacted> }")
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidFrame => "invalid provider bridge frame",
            Self::InvalidVerificationTime => "invalid provider verification timestamp",
            Self::InvalidLeasePolicy => "invalid provider bridge lease policy",
            Self::WrongConfiguration => "provider bridge frame does not match configuration",
            Self::SequenceConflict => "provider bridge sequence conflicts with prior frame",
            Self::SequenceGap => "provider bridge sequence is not contiguous",
            Self::ClockRegression => "provider bridge host clock regressed",
            Self::LeaseExpired => "provider bridge lease expired",
            Self::RevisionExhausted => "provider binding revision exhausted",
            Self::NoEligiblePending => "no eligible provider binding is pending",
            Self::StaleAcknowledgment => "stale provider binding acknowledgment",
            Self::StorageFailure => "provider binding storage failed",
            Self::InvalidRevisionFloor => "invalid provider binding revision floor",
            Self::HostTimeOverflow => "provider bridge host time overflowed",
            Self::BindingRejected => "provider binding observation was rejected",
        };
        f.write_str(text)
    }
}

impl Error for BridgeError {}

/// Result of admitting a frame into the in-process receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    /// Exact repeat of the latest frame; it never refreshes a lease.
    Duplicate,
    /// A distinct selected top-level session is pending durable acknowledgment.
    Pending { revision: u64 },
    /// A distinct fresh row kept the current identity and refreshed its lease.
    Renewed,
    /// A Home, child, unknown, or unavailable row revoked current eligibility.
    Revoked,
}

/// Single-frame receiver around the existing provider binding core.
///
/// The core is private, so callers cannot bypass frame admission. Only one
/// latest frame is retained, in addition to the core's one pending and one
/// last-confirmed binding.
pub struct BridgeReceiver {
    target: TargetRef,
    project_identity: ProjectIdentity,
    provider_version: SupportedVersion,
    bridge_instance: u64,
    policy: LeasePolicy,
    binding: ProviderBinding,
    last_frame: Option<Frame>,
    last_sequence: u64,
    last_call_ms: Option<u64>,
    lease_deadline_ms: Option<u64>,
    reconciliation_required: bool,
    reconciliation_requested: bool,
    reconcile_watermark: u64,
    revision_floor: u64,
}

impl BridgeReceiver {
    /// Configure a new bridge. An explicit reconciliation request is required before admission.
    pub fn new(
        target: TargetRef,
        project_identity: &[u8],
        bridge_instance: u64,
        policy: LeasePolicy,
    ) -> Result<Self, BridgeError> {
        let binding = ProviderBinding::new(
            target,
            project_identity,
            SUPPORTED_PROVIDER_VERSION,
            bridge_instance,
        )
        .map_err(|_| BridgeError::InvalidFrame)?;
        Ok(Self {
            target,
            project_identity: ProjectIdentity::new(project_identity)
                .map_err(|_| BridgeError::InvalidFrame)?,
            provider_version: SupportedVersion::new(SUPPORTED_PROVIDER_VERSION)
                .map_err(|_| BridgeError::InvalidFrame)?,
            bridge_instance,
            policy,
            binding,
            last_frame: None,
            last_sequence: 0,
            last_call_ms: None,
            lease_deadline_ms: None,
            reconciliation_required: true,
            reconciliation_requested: false,
            reconcile_watermark: 0,
            revision_floor: 0,
        })
    }

    /// Restore the last-confirmed record as stale and preserve its revision floor.
    pub fn restore(
        target: TargetRef,
        project_identity: &[u8],
        bridge_instance: u64,
        policy: LeasePolicy,
        record: LastConfirmedRecord,
        revision_floor: u64,
    ) -> Result<Self, BridgeError> {
        if revision_floor < record.binding().revision() {
            return Err(BridgeError::InvalidRevisionFloor);
        }
        let binding = ProviderBinding::restore(
            target,
            project_identity,
            SUPPORTED_PROVIDER_VERSION,
            bridge_instance,
            record,
        )
        .map_err(|_| BridgeError::InvalidFrame)?;
        Ok(Self {
            target,
            project_identity: ProjectIdentity::new(project_identity)
                .map_err(|_| BridgeError::InvalidFrame)?,
            provider_version: SupportedVersion::new(SUPPORTED_PROVIDER_VERSION)
                .map_err(|_| BridgeError::InvalidFrame)?,
            bridge_instance,
            policy,
            binding,
            last_frame: None,
            last_sequence: 0,
            last_call_ms: None,
            lease_deadline_ms: None,
            reconciliation_required: true,
            reconciliation_requested: false,
            reconcile_watermark: 0,
            revision_floor,
        })
    }

    /// Replace a bridge instance while preserving confirmed identity and revision ordering.
    pub fn replace_bridge(&mut self, bridge_instance: u64) -> Result<(), BridgeError> {
        if bridge_instance == 0 || bridge_instance == self.bridge_instance {
            return Err(BridgeError::InvalidFrame);
        }
        let binding = match self.binding.last_confirmed() {
            Some(previous) => ProviderBinding::restore(
                self.target,
                self.project_identity.as_bytes(),
                self.provider_version.as_str(),
                bridge_instance,
                LastConfirmedRecord::from_validated_binding(copy_binding(previous)?),
            ),
            None => ProviderBinding::new(
                self.target,
                self.project_identity.as_bytes(),
                self.provider_version.as_str(),
                bridge_instance,
            ),
        }
        .map_err(|_| BridgeError::InvalidFrame)?;
        self.binding = binding;
        self.bridge_instance = bridge_instance;
        self.last_frame = None;
        self.last_sequence = 0;
        self.lease_deadline_ms = None;
        self.reconciliation_required = true;
        self.reconciliation_requested = false;
        self.reconcile_watermark = 0;
        Ok(())
    }

    /// Current core lifecycle state, exposed read-only.
    pub const fn state(&self) -> BindingState {
        self.binding.state()
    }

    /// Current eligible pending binding, exposed read-only.
    pub fn pending(&self) -> Option<&Binding> {
        if self.reconciliation_required || self.lease_deadline_ms.is_none() {
            return None;
        }
        self.binding.pending()
    }

    /// Last durably confirmed binding, exposed read-only.
    pub fn last_confirmed(&self) -> Option<&Binding> {
        self.binding.last_confirmed()
    }

    /// Highest binding revision issued by this receiver, including unconfirmed observations.
    pub const fn revision_floor(&self) -> u64 {
        self.revision_floor
    }

    /// Require a new distinct frame after the latest admitted source sequence.
    pub fn request_reconciliation(&mut self) {
        self.reconciliation_required = true;
        self.reconciliation_requested = true;
        self.reconcile_watermark = self.last_sequence;
        self.lease_deadline_ms = None;
        self.binding.mark_stale();
    }

    /// Admit one typed frame with host monotonic time and trusted wall verification time.
    pub fn receive(
        &mut self,
        frame: Frame,
        now_ms: u64,
        verified_at_unix_ms: u64,
    ) -> Result<Admission, BridgeError> {
        self.receive_at(frame, now_ms, now_ms, verified_at_unix_ms)
    }

    /// Admit a frame received earlier but processed on the owner clock now.
    /// Clock ordering uses `processing_ms`; freshness and the lease use the
    /// original receipt time so queue delay cannot extend identity validity.
    pub fn receive_at(
        &mut self,
        frame: Frame,
        processing_ms: u64,
        received_ms: u64,
        verified_at_unix_ms: u64,
    ) -> Result<Admission, BridgeError> {
        self.check_clock(processing_ms)?;
        if received_ms > processing_ms
            || processing_ms.saturating_sub(received_ms) >= self.policy.lease_ms
        {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::LeaseExpired);
        }
        self.expire_if_needed(processing_ms);

        if frame.bridge_instance != self.bridge_instance
            || frame.target != self.target
            || frame.project_identity != self.project_identity
            || frame.provider_version != self.provider_version
            || frame.protocol_version != BRIDGE_PROTOCOL_VERSION
        {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::WrongConfiguration);
        }

        if frame.sequence == self.last_sequence && self.last_sequence != 0 {
            if self.last_frame.as_ref() == Some(&frame) {
                return Ok(Admission::Duplicate);
            }
            self.invalidate(BindingState::Ambiguous);
            return Err(BridgeError::SequenceConflict);
        }
        if frame.sequence < self.last_sequence {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::SequenceConflict);
        }
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(BridgeError::SequenceGap)?;
        if frame.sequence != expected {
            self.invalidate(BindingState::Ambiguous);
            return Err(BridgeError::SequenceGap);
        }

        self.last_sequence = frame.sequence;
        self.last_frame = Some(frame.clone());
        if frame.scope != FrameScope::TopLevel {
            self.revoke(BindingState::Stale);
            return Ok(Admission::Revoked);
        }

        let session = match &frame.selection {
            Selection::Session { session_id, source } => (session_id.as_str(), *source),
            Selection::Home | Selection::Child => {
                self.revoke(BindingState::Stale);
                return Ok(Admission::Revoked);
            }
            Selection::Unknown => {
                self.revoke(BindingState::Ambiguous);
                return Ok(Admission::Revoked);
            }
            Selection::Unavailable => {
                self.revoke(BindingState::Unavailable);
                return Ok(Admission::Revoked);
            }
        };
        if verified_at_unix_ms == 0 {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::InvalidVerificationTime);
        }

        let was_reconciliation = self.reconciliation_required;
        if was_reconciliation
            && (!self.reconciliation_requested || frame.sequence <= self.reconcile_watermark)
        {
            self.invalidate(BindingState::Stale);
            return Ok(Admission::Revoked);
        }
        let current_id = self
            .binding
            .pending()
            .or_else(|| self.binding.verified_current())
            .map(Binding::session_id);
        let Some(next_revision) = self.revision_floor.checked_add(1) else {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::RevisionExhausted);
        };
        let same_current = !was_reconciliation && current_id == Some(session.0);
        let Some(deadline) = received_ms.checked_add(self.policy.lease_ms) else {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::HostTimeOverflow);
        };
        let outcome = if same_current {
            Admission::Renewed
        } else {
            let observation = BindingObservation::new(
                ObservationScope::SelectedTopLevel,
                self.target,
                &self.project_identity,
                &self.provider_version,
                self.bridge_instance,
                next_revision,
                session.0,
                verified_at_unix_ms,
                session.1,
            )
            .map_err(|_| BridgeError::InvalidFrame)?;
            match self.binding.observe(observation) {
                Ok(ObserveOutcome::Pending) => {
                    self.revision_floor = next_revision;
                    Admission::Pending {
                        revision: next_revision,
                    }
                }
                Ok(ObserveOutcome::Duplicate) => Admission::Renewed,
                Err(_) => {
                    self.invalidate(BindingState::Stale);
                    return Err(BridgeError::BindingRejected);
                }
            }
        };
        self.lease_deadline_ms = Some(deadline);
        if was_reconciliation {
            self.reconciliation_required = false;
            self.reconciliation_requested = false;
        }
        Ok(outcome)
    }

    /// Consume a contiguous frame sampled before the latest owner-requested
    /// reconciliation epoch. It advances the source sequence but leaves the
    /// reconciliation request eligible for a later fresh snapshot.
    pub(crate) fn discard_for_reconciliation(
        &mut self,
        frame: Frame,
        processing_ms: u64,
        received_ms: u64,
    ) -> Result<(), BridgeError> {
        self.check_clock(processing_ms)?;
        if received_ms > processing_ms
            || processing_ms.saturating_sub(received_ms) >= self.policy.lease_ms
        {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::LeaseExpired);
        }
        if frame.bridge_instance != self.bridge_instance
            || frame.target != self.target
            || frame.project_identity != self.project_identity
            || frame.provider_version != self.provider_version
            || frame.protocol_version != BRIDGE_PROTOCOL_VERSION
        {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::WrongConfiguration);
        }
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(BridgeError::SequenceGap)?;
        if frame.sequence != expected {
            self.invalidate(BindingState::Ambiguous);
            return Err(if frame.sequence < expected {
                BridgeError::SequenceConflict
            } else {
                BridgeError::SequenceGap
            });
        }
        if !self.reconciliation_required || !self.reconciliation_requested {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::NoEligiblePending);
        }
        self.last_sequence = frame.sequence;
        self.last_frame = Some(frame);
        self.lease_deadline_ms = None;
        Ok(())
    }

    /// Confirm only an eligible, unexpired pending binding through an immutable writer.
    pub fn confirm_pending<F, E>(
        &mut self,
        revision: u64,
        now_ms: u64,
        writer: F,
    ) -> Result<(), BridgeError>
    where
        F: FnOnce(&Binding) -> Result<(), E>,
    {
        self.check_clock(now_ms)?;
        if self.expire_if_needed(now_ms) {
            return Err(BridgeError::LeaseExpired);
        }
        if self.reconciliation_required || self.lease_deadline_ms.is_none() {
            return Err(BridgeError::NoEligiblePending);
        }
        match self.binding.confirm_pending(revision, writer) {
            Ok(()) => Ok(()),
            Err(BindingError::StaleAcknowledgment) => Err(BridgeError::StaleAcknowledgment),
            Err(BindingError::StorageFailure) => Err(BridgeError::StorageFailure),
            Err(_) => Err(BridgeError::NoEligiblePending),
        }
    }

    /// Advance the monotonic clock and expire identity while the bridge is idle.
    pub fn tick(&mut self, now_ms: u64) -> Result<bool, BridgeError> {
        self.check_clock(now_ms)?;
        Ok(self.expire_if_needed(now_ms))
    }

    /// Fail closed when transport or frame decoding cannot produce a valid frame.
    pub fn mark_unavailable(&mut self, now_ms: u64) -> Result<(), BridgeError> {
        self.check_clock(now_ms)?;
        self.invalidate(BindingState::Unavailable);
        Ok(())
    }

    fn check_clock(&mut self, now_ms: u64) -> Result<(), BridgeError> {
        if self.last_call_ms.is_some_and(|last| now_ms < last) {
            self.invalidate(BindingState::Stale);
            return Err(BridgeError::ClockRegression);
        }
        self.last_call_ms = Some(now_ms);
        Ok(())
    }

    fn expire_if_needed(&mut self, now_ms: u64) -> bool {
        if self
            .lease_deadline_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.invalidate(BindingState::Stale);
            true
        } else {
            false
        }
    }

    fn revoke(&mut self, state: BindingState) {
        self.invalidate(state);
    }

    fn invalidate(&mut self, state: BindingState) {
        match state {
            BindingState::Unavailable => self.binding.mark_unavailable(),
            BindingState::Ambiguous => self.binding.mark_ambiguous(),
            _ => self.binding.mark_stale(),
        }
        self.reconciliation_required = true;
        self.reconciliation_requested = false;
        self.reconcile_watermark = self.last_sequence;
        self.lease_deadline_ms = None;
    }
}

impl fmt::Debug for BridgeReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeReceiver { <redacted> }")
    }
}

fn copy_binding(binding: &Binding) -> Result<Binding, BridgeError> {
    Binding::new(
        binding.target(),
        &ProjectIdentity::new(binding.project_identity()).map_err(|_| BridgeError::InvalidFrame)?,
        &SupportedVersion::new(binding.supported_version())
            .map_err(|_| BridgeError::InvalidFrame)?,
        binding.bridge_instance(),
        binding.revision(),
        binding.session_id(),
        binding.verified_at_unix_ms(),
        binding.source(),
    )
    .map_err(|_| BridgeError::InvalidFrame)
}

fn dummy_target() -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &[u8] = b"canonical-project-secret";
    const BRIDGE: u64 = 41;
    const LEASE: u64 = 100;

    fn target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 12,
            pane: 13,
            session: 14,
            occupant_revision: 15,
        }
    }

    fn receiver() -> BridgeReceiver {
        BridgeReceiver::new(target(), PROJECT, BRIDGE, LeasePolicy::new(LEASE).unwrap()).unwrap()
    }

    fn selection(id: &str) -> Selection {
        Selection::session(id, VerificationSource::TuiRouteSnapshot).unwrap()
    }

    fn frame(sequence: u64, selection: Selection) -> Frame {
        frame_for(
            BRIDGE,
            sequence,
            BRIDGE_PROTOCOL_VERSION,
            target(),
            PROJECT,
            SUPPORTED_PROVIDER_VERSION,
            FrameScope::TopLevel,
            selection,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_for(
        bridge: u64,
        sequence: u64,
        protocol: u16,
        target: TargetRef,
        project: &[u8],
        provider: &str,
        scope: FrameScope,
        selection: Selection,
    ) -> Frame {
        Frame::new(
            protocol, target, project, provider, bridge, sequence, scope, selection,
        )
        .unwrap()
    }

    fn admit_first(receiver: &mut BridgeReceiver, id: &str) -> u64 {
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive(frame(1, selection(id)), 0, 1_760_000_000_000),
            Ok(Admission::Pending { revision: 1 })
        );
        1
    }

    #[test]
    fn starts_closed_and_needs_explicit_reconciliation() {
        let mut receiver = receiver();
        assert_eq!(
            receiver.receive(frame(1, selection("session-a")), 0, 1_760_000_000_000),
            Ok(Admission::Revoked)
        );
        assert!(receiver.pending().is_none());
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive(frame(2, selection("session-a")), 1, 1_760_000_000_001),
            Ok(Admission::Pending { revision: 1 })
        );
    }

    #[test]
    fn exact_duplicate_is_idempotent_and_does_not_renew_receipt() {
        let mut receiver = receiver();
        admit_first(&mut receiver, "session-a");
        let duplicate = frame(1, selection("session-a"));
        assert_eq!(
            receiver.receive(duplicate, 50, 9_999_999_999_999),
            Ok(Admission::Duplicate)
        );
        assert_eq!(
            receiver.confirm_pending(1, 100, |_| Ok::<(), ()>(())),
            Err(BridgeError::LeaseExpired)
        );
        assert!(receiver.pending().is_none());
        assert_eq!(receiver.last_confirmed(), None);
    }

    #[test]
    fn idle_tick_expires_and_zero_wall_timestamp_never_becomes_pending() {
        let mut receiver = receiver();
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive(frame(1, selection("session-a")), 0, 0),
            Err(BridgeError::InvalidVerificationTime)
        );
        assert!(receiver.pending().is_none());
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive(frame(2, selection("session-a")), 1, 1_760_000_000_002),
            Ok(Admission::Pending { revision: 1 })
        );
        assert_eq!(receiver.tick(101), Ok(true));
        assert_eq!(receiver.state(), BindingState::Stale);
        assert!(receiver.pending().is_none());
        assert_eq!(receiver.tick(102), Ok(false));
        assert_eq!(
            receiver.receive(frame(3, selection("session-a")), 103, 1_760_000_000_003),
            Ok(Admission::Revoked)
        );
    }

    #[test]
    fn changed_duplicate_and_sequence_regression_or_gap_fail_closed() {
        let mut changed = receiver();
        admit_first(&mut changed, "session-a");
        assert_eq!(
            changed.receive(frame(1, selection("session-b")), 1, 1_760_000_000_001),
            Err(BridgeError::SequenceConflict)
        );
        assert!(changed.pending().is_none());

        let mut regression = receiver();
        admit_first(&mut regression, "session-a");
        assert_eq!(
            regression.receive(frame(2, selection("session-b")), 1, 1_760_000_000_001),
            Ok(Admission::Pending { revision: 2 })
        );
        assert_eq!(
            regression.receive(frame(1, selection("session-a")), 2, 1_760_000_000_002),
            Err(BridgeError::SequenceConflict)
        );
        assert!(regression.pending().is_none());

        let mut gap = receiver();
        admit_first(&mut gap, "session-a");
        assert_eq!(
            gap.receive(frame(3, selection("session-b")), 1, 1_760_000_000_001),
            Err(BridgeError::SequenceGap)
        );
        assert!(gap.pending().is_none());
    }

    #[test]
    fn wrong_scope_version_instance_project_or_target_invalidates() {
        let mut child = receiver();
        admit_first(&mut child, "session-a");
        assert_eq!(
            child.receive(
                frame_for(
                    BRIDGE,
                    2,
                    BRIDGE_PROTOCOL_VERSION,
                    target(),
                    PROJECT,
                    SUPPORTED_PROVIDER_VERSION,
                    FrameScope::ChildActivity,
                    selection("child-session"),
                ),
                1,
                1_760_000_000_001,
            ),
            Ok(Admission::Revoked)
        );
        assert!(child.pending().is_none());

        for (bridge, project, provider, observed_target) in [
            (BRIDGE + 1, PROJECT, SUPPORTED_PROVIDER_VERSION, target()),
            (
                BRIDGE,
                b"other-project".as_slice(),
                SUPPORTED_PROVIDER_VERSION,
                target(),
            ),
            (BRIDGE, PROJECT, "2.0.31", target()),
            (
                BRIDGE,
                PROJECT,
                SUPPORTED_PROVIDER_VERSION,
                TargetRef {
                    pane: 99,
                    ..target()
                },
            ),
        ] {
            let mut receiver = receiver();
            admit_first(&mut receiver, "session-a");
            assert_eq!(
                receiver.receive(
                    frame_for(
                        bridge,
                        2,
                        BRIDGE_PROTOCOL_VERSION,
                        observed_target,
                        project,
                        provider,
                        FrameScope::TopLevel,
                        selection("session-b"),
                    ),
                    1,
                    1_760_000_000_001,
                ),
                Err(BridgeError::WrongConfiguration)
            );
            assert!(receiver.pending().is_none());
            assert_eq!(receiver.last_confirmed(), None);
        }
    }

    #[test]
    fn lease_expiry_requires_explicit_new_row_and_cannot_auto_revive() {
        let mut receiver = receiver();
        admit_first(&mut receiver, "session-a");
        assert_eq!(
            receiver.receive(frame(2, selection("session-a")), 100, 1_760_000_000_001),
            Ok(Admission::Revoked)
        );
        assert!(receiver.pending().is_none());
        assert_eq!(
            receiver.receive(frame(2, selection("session-a")), 101, 1_760_000_000_002),
            Ok(Admission::Duplicate)
        );
        assert!(receiver.pending().is_none());
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive(frame(3, selection("session-a")), 102, 1_760_000_000_003),
            Ok(Admission::Pending { revision: 2 })
        );
    }

    #[test]
    fn child_home_unknown_and_unavailable_revoke_top_level_identity() {
        for (selection, expected_state) in [
            (Selection::Child, BindingState::Stale),
            (Selection::Home, BindingState::Stale),
            (Selection::Unknown, BindingState::Ambiguous),
            (Selection::Unavailable, BindingState::Unavailable),
        ] {
            let mut receiver = receiver();
            admit_first(&mut receiver, "session-a");
            assert_eq!(
                receiver.receive(frame(2, selection), 1, 1_760_000_000_001),
                Ok(Admission::Revoked)
            );
            assert_eq!(receiver.state(), expected_state);
            assert!(receiver.pending().is_none());
        }
    }

    #[test]
    fn distinct_fresh_same_identity_renews_without_new_pending_revision() {
        let mut receiver = receiver();
        admit_first(&mut receiver, "session-a");
        assert_eq!(
            receiver.receive(frame(2, selection("session-a")), 50, 1_760_000_000_050),
            Ok(Admission::Renewed)
        );
        assert_eq!(receiver.pending().map(Binding::revision), Some(1));
        assert_eq!(receiver.revision_floor(), 1);
    }

    #[test]
    fn writer_failure_and_stale_ack_do_not_promote_or_replace_pending() {
        let mut receiver = receiver();
        admit_first(&mut receiver, "session-a");
        assert_eq!(
            receiver.confirm_pending(99, 1, |_| Ok::<(), ()>(())),
            Err(BridgeError::StaleAcknowledgment)
        );
        assert_eq!(
            receiver.confirm_pending(1, 2, |_| Err::<(), _>(())),
            Err(BridgeError::StorageFailure)
        );
        assert_eq!(
            receiver.pending().map(Binding::session_id),
            Some("session-a")
        );
        assert!(receiver.last_confirmed().is_none());
        receiver
            .confirm_pending(1, 3, |_| Ok::<(), ()>(()))
            .unwrap();
        assert!(receiver.pending().is_none());
        assert_eq!(
            receiver.last_confirmed().map(Binding::session_id),
            Some("session-a")
        );
        assert_eq!(
            receiver.last_confirmed().map(Binding::verified_at_unix_ms),
            Some(1_760_000_000_000)
        );
    }

    #[test]
    fn restored_and_replaced_bridges_keep_the_binding_revision_floor() {
        let mut first = receiver();
        admit_first(&mut first, "session-a");
        first.confirm_pending(1, 1, |_| Ok::<(), ()>(())).unwrap();
        let restored_record = LastConfirmedRecord::from_validated_binding(
            copy_binding(first.last_confirmed().unwrap()).unwrap(),
        );
        let mut restored = BridgeReceiver::restore(
            target(),
            PROJECT,
            42,
            LeasePolicy::new(LEASE).unwrap(),
            restored_record,
            12,
        )
        .unwrap();
        assert_eq!(restored.revision_floor(), 12);
        restored.request_reconciliation();
        assert_eq!(
            restored.receive(
                frame_for(
                    42,
                    1,
                    BRIDGE_PROTOCOL_VERSION,
                    target(),
                    PROJECT,
                    SUPPORTED_PROVIDER_VERSION,
                    FrameScope::TopLevel,
                    selection("session-b"),
                ),
                2,
                1_760_000_000_002,
            ),
            Ok(Admission::Pending { revision: 13 })
        );
        restored.replace_bridge(43).unwrap();
        assert_eq!(restored.revision_floor(), 13);
        restored.request_reconciliation();
        assert_eq!(
            restored.receive(
                frame_for(
                    43,
                    1,
                    BRIDGE_PROTOCOL_VERSION,
                    target(),
                    PROJECT,
                    SUPPORTED_PROVIDER_VERSION,
                    FrameScope::TopLevel,
                    selection("session-c"),
                ),
                3,
                1_760_000_000_003,
            ),
            Ok(Admission::Pending { revision: 14 })
        );
    }

    #[test]
    fn bounds_versions_sequences_lease_and_debug_redaction() {
        assert!(LeasePolicy::new(99).is_err());
        assert!(LeasePolicy::new(60_001).is_err());
        assert!(Frame::new(
            2,
            target(),
            PROJECT,
            SUPPORTED_PROVIDER_VERSION,
            BRIDGE,
            1,
            FrameScope::TopLevel,
            selection("secret-session-id"),
        )
        .is_err());
        assert!(Frame::new(
            BRIDGE_PROTOCOL_VERSION,
            target(),
            &vec![b'x'; 4_097],
            SUPPORTED_PROVIDER_VERSION,
            BRIDGE,
            1,
            FrameScope::TopLevel,
            selection("session-a"),
        )
        .is_err());
        assert!(
            Selection::session(&"x".repeat(129), VerificationSource::TuiRouteSnapshot).is_err()
        );
        let frame = frame(1, selection("secret-session-id"));
        let debug = format!("{:?} {:?} {:?}", frame, receiver(), frame.selection);
        assert!(!debug.contains("secret-session-id"));
        assert!(!debug.contains("canonical-project-secret"));
        assert!(!format!("{:?}", BridgeError::WrongConfiguration).contains("secret"));
    }

    #[test]
    fn restored_floor_overflow_and_clock_regression_fail_closed() {
        let mut first = receiver();
        admit_first(&mut first, "session-a");
        first.confirm_pending(1, 1, |_| Ok::<(), ()>(())).unwrap();
        let record = LastConfirmedRecord::from_validated_binding(
            copy_binding(first.last_confirmed().unwrap()).unwrap(),
        );
        assert_eq!(
            BridgeReceiver::restore(
                target(),
                PROJECT,
                42,
                LeasePolicy::new(LEASE).unwrap(),
                record,
                0,
            )
            .err(),
            Some(BridgeError::InvalidRevisionFloor)
        );

        let record = LastConfirmedRecord::from_validated_binding(
            copy_binding(first.last_confirmed().unwrap()).unwrap(),
        );
        let mut overflow = BridgeReceiver::restore(
            target(),
            PROJECT,
            42,
            LeasePolicy::new(LEASE).unwrap(),
            record,
            u64::MAX,
        )
        .unwrap();
        overflow.request_reconciliation();
        assert_eq!(
            overflow.receive(
                frame_for(
                    42,
                    1,
                    BRIDGE_PROTOCOL_VERSION,
                    target(),
                    PROJECT,
                    SUPPORTED_PROVIDER_VERSION,
                    FrameScope::TopLevel,
                    selection("session-b"),
                ),
                2,
                1_760_000_000_002,
            ),
            Err(BridgeError::RevisionExhausted)
        );
        assert!(overflow.pending().is_none());

        let mut clock = receiver();
        clock.request_reconciliation();
        assert_eq!(
            clock.receive(frame(1, selection("session-a")), 10, 1_760_000_000_010),
            Ok(Admission::Pending { revision: 1 })
        );
        assert_eq!(
            clock.receive(frame(2, selection("session-b")), 9, 1_760_000_000_011),
            Err(BridgeError::ClockRegression)
        );
        assert!(clock.pending().is_none());
    }

    #[test]
    fn delayed_receipt_uses_processing_clock_and_original_lease_start() {
        let mut receiver = receiver();
        receiver.request_reconciliation();
        assert_eq!(
            receiver.receive_at(frame(1, selection("session-a")), 10, 8, 1_760_000_000_000),
            Ok(Admission::Pending { revision: 1 })
        );
        assert_eq!(receiver.tick(20), Ok(false));
        assert_eq!(
            receiver.receive_at(frame(2, selection("session-a")), 21, 19, 1_760_000_000_001),
            Ok(Admission::Renewed)
        );
        assert_eq!(receiver.tick(118), Ok(false));
        assert_eq!(receiver.tick(119), Ok(true));
        assert_eq!(receiver.state(), BindingState::Stale);
    }

    #[test]
    fn stale_queued_receipt_is_rejected_and_old_epoch_discard_keeps_barrier() {
        let mut stale = receiver();
        stale.request_reconciliation();
        assert_eq!(
            stale.receive_at(frame(1, selection("session-a")), 100, 0, 1_760_000_000_000),
            Err(BridgeError::LeaseExpired)
        );
        assert_eq!(stale.state(), BindingState::Stale);

        let mut barrier = receiver();
        barrier.request_reconciliation();
        barrier
            .discard_for_reconciliation(frame(1, selection("session-old")), 10, 9)
            .unwrap();
        assert_eq!(barrier.pending(), None);
        assert_eq!(
            barrier.receive_at(
                frame(2, selection("session-new")),
                11,
                10,
                1_760_000_000_001
            ),
            Ok(Admission::Pending { revision: 1 })
        );
    }
}
