//! Fail-closed decision model for explicit recovery from a last-confirmed binding.
//!
//! This module does not query providers, launch or resume processes, or persist
//! managed-session metadata. A candidate is only a proposed mapping. Callers
//! must complete the listed save, authorization, and post-start verification
//! gates before advertising recovery as available.

use crate::control_protocol::TargetRef;
use crate::provider_binding::{
    Binding, LastConfirmedRecord, ProjectIdentity, SupportedVersion, VerificationSource,
};
use crate::provider_bridge::SUPPORTED_PROVIDER_VERSION;
use std::fmt;

/// Validation failures returned by the provider lookup layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderLookupFailure {
    /// The provider interface returned no matching session.
    Missing,
    /// The provider interface confirmed that the session was deleted.
    Deleted,
    /// The session belongs to a different project.
    ForeignProject,
    /// The match describes a child/subagent route, not the selected top level.
    ChildActivity,
    /// The provider interface reports an unsupported provider version.
    UnsupportedVersion,
    /// The lookup could not produce authoritative, consistent evidence.
    Uncertain,
}

impl fmt::Display for ProviderLookupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Missing => "provider session was not found",
            Self::Deleted => "provider session was deleted",
            Self::ForeignProject => "provider session belongs to another project",
            Self::ChildActivity => "provider result is a child session",
            Self::UnsupportedVersion => "provider version is not supported",
            Self::Uncertain => "provider session could not be verified",
        };
        f.write_str(text)
    }
}

/// Explicit, bounded user-selected provider session and project.
pub struct ExplicitRecoveryChoice {
    session_id: String,
    project_identity: ProjectIdentity,
}

impl ExplicitRecoveryChoice {
    /// Validate and copy a user-supplied exact session ID and canonical project.
    pub fn new(session_id: &str, project_identity: &[u8]) -> Result<Self, RecoveryBlocker> {
        let project_identity =
            ProjectIdentity::new(project_identity).map_err(|_| RecoveryBlocker::InvalidChoice)?;
        // Reuse Binding's exact validated identifier rules without adding a
        // second, potentially drifting session-ID validator.
        let version = SupportedVersion::new(SUPPORTED_PROVIDER_VERSION)
            .map_err(|_| RecoveryBlocker::UnsupportedVersion)?;
        Binding::new(
            valid_target(),
            &project_identity,
            &version,
            1,
            1,
            session_id,
            1,
            VerificationSource::TuiRouteSnapshot,
        )
        .map_err(|_| RecoveryBlocker::InvalidChoice)?;
        Ok(Self {
            session_id: session_id.to_owned(),
            project_identity,
        })
    }
}

impl fmt::Debug for ExplicitRecoveryChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExplicitRecoveryChoice { <redacted> }")
    }
}

/// Provider-interface evidence for exactly one top-level session lookup.
///
/// Fields are private and there is no public constructor. The crate-private
/// constructor is reserved for a provider adapter after it has actually
/// queried a supported interface; a caller-supplied ID or launch argument is
/// not sufficient. This crate currently has no such verification producer.
pub struct ProviderSessionProof {
    session_id: String,
    project_identity: ProjectIdentity,
    provider_version: SupportedVersion,
    source: VerificationSource,
    verified_at_unix_ms: u64,
}

impl ProviderSessionProof {
    /// Construct proof only at an adapter call site that validated the exact
    /// provider lookup result as top-level and project-scoped.
    #[cfg(test)]
    pub(crate) fn from_provider_interface(
        session_id: &str,
        project_identity: &[u8],
        provider_version: &str,
        source: VerificationSource,
        verified_at_unix_ms: u64,
    ) -> Result<Self, RecoveryBlocker> {
        if provider_version != SUPPORTED_PROVIDER_VERSION {
            return Err(RecoveryBlocker::UnsupportedVersion);
        }
        if verified_at_unix_ms == 0 {
            return Err(RecoveryBlocker::UncertainProviderResult);
        }
        let project_identity =
            ProjectIdentity::new(project_identity).map_err(|_| RecoveryBlocker::InvalidChoice)?;
        let provider_version = SupportedVersion::new(provider_version)
            .map_err(|_| RecoveryBlocker::UnsupportedVersion)?;
        let version = SupportedVersion::new(SUPPORTED_PROVIDER_VERSION)
            .map_err(|_| RecoveryBlocker::UnsupportedVersion)?;
        Binding::new(
            valid_target(),
            &project_identity,
            &version,
            1,
            1,
            session_id,
            verified_at_unix_ms,
            source,
        )
        .map_err(|_| RecoveryBlocker::InvalidChoice)?;
        Ok(Self {
            session_id: session_id.to_owned(),
            project_identity,
            provider_version,
            source,
            verified_at_unix_ms,
        })
    }
}

impl fmt::Debug for ProviderSessionProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderSessionProof { <redacted> }")
    }
}

/// Fixed reason why a recovery candidate cannot be produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryBlocker {
    /// No exact last-confirmed record was loaded.
    NoLastConfirmed,
    /// The explicit session or project input is malformed or out of bounds.
    InvalidChoice,
    /// The saved record or user choice belongs to a different project.
    ProjectMismatch,
    /// The saved record or current adapter version is unsupported.
    UnsupportedVersion,
    /// Provider lookup reported a missing session.
    MissingSession,
    /// Provider lookup reported a deleted session.
    DeletedSession,
    /// Provider lookup reported a foreign-project session.
    ForeignProjectSession,
    /// Provider lookup found only child/subagent activity.
    ChildActivity,
    /// Provider evidence was incomplete, conflicting, or uncertain.
    UncertainProviderResult,
    /// Provider proof did not exactly match the user's choice and project.
    ChoiceNotVerified,
    /// Current managed target or new bridge identity is invalid.
    InvalidTarget,
    /// Binding revision cannot advance without wrapping.
    RevisionExhausted,
}

/// Required actions before a proposed mapping can be used for explicit resume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryGate {
    /// Commit the mapping and managed-session generation as one atomic update.
    PersistMappingWithManagedSession,
    /// Wait for an explicit user action to resume this exact chosen session.
    ExplicitResumeAuthorization,
    /// After startup, reverify the selected top-level route before marking saved.
    VerifySelectedTopLevelAfterStart,
}

const REQUIRED_GATES: [RecoveryGate; 3] = [
    RecoveryGate::PersistMappingWithManagedSession,
    RecoveryGate::ExplicitResumeAuthorization,
    RecoveryGate::VerifySelectedTopLevelAfterStart,
];

/// Proposed exact binding or a fixed fail-closed blocker.
pub enum RecoveryDecision {
    /// An explicit choice matched provider-interface evidence. This is not a
    /// durable record, a resume authorization, or proof of current TUI state.
    Candidate(RecoveryCandidate),
    /// Manual action or a stronger provider interface is required.
    NeedsManual(RecoveryBlocker),
}

impl fmt::Debug for RecoveryDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Candidate(_) => "RecoveryDecision::Candidate { <redacted> }",
            Self::NeedsManual(_) => "RecoveryDecision::NeedsManual { <fixed> }",
        })
    }
}

/// Exact binding candidate for a user-selected session on the current target.
pub struct RecoveryCandidate {
    binding: Binding,
}

impl RecoveryCandidate {
    /// Proposed mapping to persist before any explicit resume.
    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    /// Ordered save, authorization, and post-start verification requirements.
    pub const fn required_gates(&self) -> &'static [RecoveryGate] {
        &REQUIRED_GATES
    }
}

impl fmt::Debug for RecoveryCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryCandidate { <redacted> }")
    }
}

/// Evaluate an explicit choice against the exact last-confirmed record and a
/// sealed provider-interface proof. A new target generation is allowed only
/// as an explicit mapping proposal; callers still owe every returned gate.
pub fn plan_recovery(
    saved: Option<&LastConfirmedRecord>,
    current_target: TargetRef,
    current_project: &[u8],
    current_provider_version: &str,
    new_bridge_instance: u64,
    choice: &ExplicitRecoveryChoice,
    lookup: Result<&ProviderSessionProof, ProviderLookupFailure>,
) -> RecoveryDecision {
    let Some(saved) = saved else {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::NoLastConfirmed);
    };
    let saved = saved.binding();
    if current_provider_version != SUPPORTED_PROVIDER_VERSION
        || saved.supported_version() != SUPPORTED_PROVIDER_VERSION
    {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::UnsupportedVersion);
    }
    if current_project.is_empty()
        || saved.project_identity() != current_project
        || choice.project_identity.as_bytes() != current_project
    {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::ProjectMismatch);
    }
    if current_target.generation == 0
        || current_target.workspace == 0
        || current_target.pane == 0
        || current_target.session == 0
        || current_target.occupant_revision == 0
        || new_bridge_instance == 0
    {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::InvalidTarget);
    }
    let proof = match lookup {
        Ok(proof) => proof,
        Err(failure) => {
            return RecoveryDecision::NeedsManual(match failure {
                ProviderLookupFailure::Missing => RecoveryBlocker::MissingSession,
                ProviderLookupFailure::Deleted => RecoveryBlocker::DeletedSession,
                ProviderLookupFailure::ForeignProject => RecoveryBlocker::ForeignProjectSession,
                ProviderLookupFailure::ChildActivity => RecoveryBlocker::ChildActivity,
                ProviderLookupFailure::UnsupportedVersion => RecoveryBlocker::UnsupportedVersion,
                ProviderLookupFailure::Uncertain => RecoveryBlocker::UncertainProviderResult,
            })
        }
    };
    if proof.project_identity.as_bytes() != current_project
        || proof.project_identity.as_bytes() != choice.project_identity.as_bytes()
        || proof.session_id != choice.session_id
        || proof.provider_version.as_str() != current_provider_version
        || proof.verified_at_unix_ms == 0
    {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::ChoiceNotVerified);
    }
    let Some(revision) = saved.revision().checked_add(1) else {
        return RecoveryDecision::NeedsManual(RecoveryBlocker::RevisionExhausted);
    };
    let project = match ProjectIdentity::new(current_project) {
        Ok(project) => project,
        Err(_) => return RecoveryDecision::NeedsManual(RecoveryBlocker::ProjectMismatch),
    };
    let version = match SupportedVersion::new(current_provider_version) {
        Ok(version) => version,
        Err(_) => return RecoveryDecision::NeedsManual(RecoveryBlocker::UnsupportedVersion),
    };
    match Binding::new(
        current_target,
        &project,
        &version,
        new_bridge_instance,
        revision,
        &choice.session_id,
        proof.verified_at_unix_ms,
        proof.source,
    ) {
        Ok(binding) => RecoveryDecision::Candidate(RecoveryCandidate { binding }),
        Err(_) => RecoveryDecision::NeedsManual(RecoveryBlocker::InvalidChoice),
    }
}

fn valid_target() -> TargetRef {
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
    use crate::provider_binding::LastConfirmedRecord;

    const PROJECT: &[u8] = b"/canonical/project";
    const VERSION: &str = SUPPORTED_PROVIDER_VERSION;

    fn target(generation: u64, occupant_revision: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision,
        }
    }

    fn saved(project: &[u8], target: TargetRef, revision: u64) -> LastConfirmedRecord {
        let project = ProjectIdentity::new(project).unwrap();
        let version = SupportedVersion::new(VERSION).unwrap();
        LastConfirmedRecord::from_validated_binding(
            Binding::new(
                target,
                &project,
                &version,
                19,
                revision,
                "saved_session",
                1_760_000_000_000,
                VerificationSource::TuiRouteSnapshot,
            )
            .unwrap(),
        )
    }

    fn proof(session: &str, project: &[u8]) -> ProviderSessionProof {
        ProviderSessionProof::from_provider_interface(
            session,
            project,
            VERSION,
            VerificationSource::OrderedSelection,
            1_760_000_000_100,
        )
        .unwrap()
    }

    fn choice(session: &str, project: &[u8]) -> ExplicitRecoveryChoice {
        ExplicitRecoveryChoice::new(session, project).unwrap()
    }

    #[test]
    fn deleted_or_missing_session_needs_manual_recovery() {
        let saved = saved(PROJECT, target(10, 12), 7);
        let choice = choice("chosen_session", PROJECT);
        for (failure, expected) in [
            (
                ProviderLookupFailure::Deleted,
                RecoveryBlocker::DeletedSession,
            ),
            (
                ProviderLookupFailure::Missing,
                RecoveryBlocker::MissingSession,
            ),
        ] {
            assert!(matches!(
                plan_recovery(
                    Some(&saved),
                    target(11, 13),
                    PROJECT,
                    VERSION,
                    20,
                    &choice,
                    Err(failure),
                ),
                RecoveryDecision::NeedsManual(actual) if actual == expected
            ));
        }
    }

    #[test]
    fn no_saved_record_invalid_choice_and_unsupported_version_fail_closed() {
        assert!(matches!(
            plan_recovery(
                None,
                target(11, 13),
                PROJECT,
                VERSION,
                20,
                &choice("chosen_session", PROJECT),
                Err(ProviderLookupFailure::Uncertain),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::NoLastConfirmed)
        ));
        assert!(ExplicitRecoveryChoice::new("invalid/id", PROJECT).is_err());
        let saved = saved(PROJECT, target(10, 12), 7);
        assert!(matches!(
            plan_recovery(
                Some(&saved),
                target(11, 13),
                PROJECT,
                "99.99.99",
                20,
                &choice("saved_session", PROJECT),
                Err(ProviderLookupFailure::Uncertain),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::UnsupportedVersion)
        ));
    }

    #[test]
    fn invalid_current_target_and_revision_overflow_fail_closed() {
        let saved_record = saved(PROJECT, target(10, 12), 7);
        let choice = choice("saved_session", PROJECT);
        let proof = proof("saved_session", PROJECT);
        assert!(matches!(
            plan_recovery(
                Some(&saved_record),
                target(0, 13),
                PROJECT,
                VERSION,
                20,
                &choice,
                Ok(&proof),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::InvalidTarget)
        ));
        let exhausted = saved(PROJECT, target(10, 12), u64::MAX);
        assert!(matches!(
            plan_recovery(
                Some(&exhausted),
                target(11, 13),
                PROJECT,
                VERSION,
                20,
                &choice,
                Ok(&proof),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::RevisionExhausted)
        ));
    }

    #[test]
    fn explicit_choice_must_match_the_sealed_provider_proof() {
        let saved = saved(PROJECT, target(10, 12), 7);
        let choice = choice("user_chosen_session", PROJECT);
        let proof = proof("different_session", PROJECT);
        assert!(matches!(
            plan_recovery(
                Some(&saved),
                target(11, 13),
                PROJECT,
                VERSION,
                20,
                &choice,
                Ok(&proof),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::ChoiceNotVerified)
        ));
    }

    #[test]
    fn old_project_mapping_cannot_follow_a_project_switch() {
        let saved = saved(PROJECT, target(10, 12), 7);
        let new_project = b"/canonical/other-project";
        let choice = choice("saved_session", new_project);
        let proof = proof("saved_session", new_project);
        assert!(matches!(
            plan_recovery(
                Some(&saved),
                target(11, 13),
                new_project,
                VERSION,
                20,
                &choice,
                Ok(&proof),
            ),
            RecoveryDecision::NeedsManual(RecoveryBlocker::ProjectMismatch)
        ));
    }

    #[test]
    fn verified_explicit_choice_can_map_to_new_generation_with_gates() {
        let saved = saved(PROJECT, target(10, 12), 7);
        let choice = choice("saved_session", PROJECT);
        let proof = proof("saved_session", PROJECT);
        let RecoveryDecision::Candidate(candidate) = plan_recovery(
            Some(&saved),
            target(11, 13),
            PROJECT,
            VERSION,
            20,
            &choice,
            Ok(&proof),
        ) else {
            panic!("explicit verified choice should produce a mapping proposal")
        };
        let binding = candidate.binding();
        assert_eq!(binding.target(), target(11, 13));
        assert_eq!(binding.session_id(), "saved_session");
        assert_eq!(binding.revision(), 8);
        assert_eq!(binding.bridge_instance(), 20);
        assert_eq!(
            candidate.required_gates(),
            &[
                RecoveryGate::PersistMappingWithManagedSession,
                RecoveryGate::ExplicitResumeAuthorization,
                RecoveryGate::VerifySelectedTopLevelAfterStart,
            ]
        );
    }

    #[test]
    fn foreign_child_unsupported_and_uncertain_results_fail_closed() {
        let saved = saved(PROJECT, target(10, 12), 7);
        let choice = choice("saved_session", PROJECT);
        for (failure, expected) in [
            (
                ProviderLookupFailure::ForeignProject,
                RecoveryBlocker::ForeignProjectSession,
            ),
            (
                ProviderLookupFailure::ChildActivity,
                RecoveryBlocker::ChildActivity,
            ),
            (
                ProviderLookupFailure::UnsupportedVersion,
                RecoveryBlocker::UnsupportedVersion,
            ),
            (
                ProviderLookupFailure::Uncertain,
                RecoveryBlocker::UncertainProviderResult,
            ),
        ] {
            assert!(matches!(
                plan_recovery(
                    Some(&saved),
                    target(10, 12),
                    PROJECT,
                    VERSION,
                    20,
                    &choice,
                    Err(failure),
                ),
                RecoveryDecision::NeedsManual(actual) if actual == expected
            ));
        }
    }
}
