//! Bounded synchronous transaction core for schema-6 managed launches.
//!
//! The coordinator owns four session identities and borrows the caller's one
//! generation-wide [`Ledger`] for each operation. It has no socket, server
//! loop, grant table, persistence policy, or automatic recovery authority.
//! Callers must validate authentication and launch authority before invoking
//! this trusted core.

use crate::control_protocol::TargetRef;
use crate::managed_launch_protocol::{
    decode_launch_request, CodecError, LaunchRequestView, LaunchResult,
};
use crate::protocol::{Admission, Ledger, OperationKey, OperationStatus, Outcome, ProtocolError};
use crate::server::EffectSink;
use crate::server_storage::{LaunchJournal, LaunchRecord, LaunchState};
use crate::session_registry::{
    AttachError, RegistryError, SessionRecordInfo, SessionRegistry, SessionState, SESSION_CAPACITY,
};
use std::fmt;
use std::io;

/// Maximum launch results one coordinator retains for exact-duplicate lookup.
///
/// Results outlive retired session slots so an exact retry of a launch whose
/// session was later closed still returns its original result. This matches
/// the launch journal's own key bound, so it never limits launches further
/// than the journal already does.
pub const MAX_RETAINED_LAUNCH_RESULTS: usize = 4_096;

/// Narrow journal operations needed by one launch transaction.
///
/// The real implementation delegates to [`LaunchJournal`]. Implementations
/// used for fault injection are not evidence of disk durability.
pub trait LaunchJournalPort {
    /// Check identity and available three-record headroom without reserving it.
    fn preflight_launch(&self, key: OperationKey, session: u64) -> io::Result<()>;

    /// Append and flush one lifecycle fact.
    fn record(&mut self, record: LaunchRecord) -> io::Result<()>;
}

impl LaunchJournalPort for LaunchJournal<'_> {
    fn preflight_launch(&self, key: OperationKey, session: u64) -> io::Result<()> {
        LaunchJournal::preflight_launch(self, key, session)
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        LaunchJournal::record(self, record)
    }
}

/// Fixed, redacted transaction failures.
pub enum ManagedLaunchError {
    /// The exact schema-6 request was malformed.
    InvalidPayload(CodecError),
    /// The caller key, coordinator and borrowed ledger do not share a generation.
    GenerationMismatch,
    /// A ledger validation/admission/status error; inspect with `ledger_error`.
    Ledger(ProtocolError),
    /// The exact key is known globally but not owned by this coordinator.
    KeyOwnedElsewhere,
    /// The request key has no retained result in this coordinator.
    UnknownKey,
    /// An asynchronous split temporarily owns the launch slot.
    InProgress,
    /// The bounded coordinator retained a lifecycle invariant failure.
    Unhealthy,
    /// Session capacity or checked identity allocation failed.
    Registry(RegistryError),
    /// Journal capacity or identity preflight failed.
    JournalPreflightFailed,
    /// A journal append/flush failed.
    JournalWriteFailed,
    /// Pure launch preparation failed; its diagnostic is discarded.
    PreparationFailed,
    /// Spawn failed without acquiring an owned child; its diagnostic is discarded.
    SpawnFailed,
    /// An internal invariant failed after an effect may have occurred.
    InternalInvariant,
}

impl ManagedLaunchError {
    /// Return the underlying protocol classification without formatting payload data.
    pub fn ledger_error(&self) -> Option<&ProtocolError> {
        match self {
            Self::Ledger(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Debug for ManagedLaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::InvalidPayload(_) => "InvalidPayload(<redacted>)",
            Self::GenerationMismatch => "GenerationMismatch",
            Self::Ledger(_) => "Ledger(<redacted>)",
            Self::KeyOwnedElsewhere => "KeyOwnedElsewhere",
            Self::UnknownKey => "UnknownKey",
            Self::InProgress => "InProgress",
            Self::Unhealthy => "Unhealthy",
            Self::Registry(error) => return f.debug_tuple("Registry").field(error).finish(),
            Self::JournalPreflightFailed => "JournalPreflightFailed",
            Self::JournalWriteFailed => "JournalWriteFailed",
            Self::PreparationFailed => "PreparationFailed",
            Self::SpawnFailed => "SpawnFailed",
            Self::InternalInvariant => "InternalInvariant",
        };
        f.write_str(label)
    }
}

impl fmt::Display for ManagedLaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPayload(_) => "managed launch payload is invalid",
            Self::GenerationMismatch => "managed launch generation is invalid",
            Self::Ledger(_) => "managed launch ledger validation failed",
            Self::KeyOwnedElsewhere => "operation key is not owned by this coordinator",
            Self::UnknownKey => "managed launch result is unavailable",
            Self::InProgress => "managed launch owner is busy",
            Self::Unhealthy => "managed launch coordinator is unhealthy",
            Self::Registry(RegistryError::Capacity) => "managed session capacity is exhausted",
            Self::Registry(RegistryError::IdentifierExhausted) => {
                "managed session IDs are exhausted"
            }
            Self::Registry(_) => "managed session registry invariant failed",
            Self::JournalPreflightFailed => "launch journal preflight failed",
            Self::JournalWriteFailed => "launch journal write failed",
            Self::PreparationFailed => "managed launch preparation failed",
            Self::SpawnFailed => "managed process spawn failed",
            Self::InternalInvariant => "managed launch invariant failed",
        })
    }
}

impl std::error::Error for ManagedLaunchError {}

#[derive(Clone, Copy)]
struct OperationEntry {
    key: OperationKey,
    result: LaunchResult,
}

/// One generation's bounded launch transaction owner.
///
/// At most four session records are retained at once, including failed and
/// exited sessions until they are explicitly retired. A successfully returned
/// child handle is never replaced; it is only released by [`Self::retire`]
/// after its child exited. Launch results are retained up to
/// [`MAX_RETAINED_LAUNCH_RESULTS`] for exact duplicates, including results
/// of retired sessions. One emergency slot protects an unexpectedly rejected
/// handle attachment; the coordinator latches unhealthy immediately afterward.
pub struct ManagedLaunchCoordinator<E> {
    generation: u64,
    registry: SessionRegistry<E>,
    operations: Vec<OperationEntry>,
    emergency_effect: Option<E>,
    unhealthy: bool,
}

impl<E> fmt::Debug for ManagedLaunchCoordinator<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedLaunchCoordinator")
            .field("generation", &"<redacted>")
            .field("used_sessions", &self.registry.records().len())
            .field("retained_results", &self.operations.len())
            .field(
                "emergency_effect_retained",
                &self.emergency_effect.is_some(),
            )
            .field("unhealthy", &self.unhealthy)
            .field("records", &"<redacted>")
            .finish()
    }
}

impl<E> ManagedLaunchCoordinator<E> {
    /// Construct an empty four-session coordinator for one nonzero generation.
    pub fn new(generation: u64) -> Result<Self, ManagedLaunchError> {
        let registry = SessionRegistry::new(generation).map_err(ManagedLaunchError::Registry)?;
        Ok(Self {
            generation,
            registry,
            operations: Vec::with_capacity(SESSION_CAPACITY),
            emergency_effect: None,
            unhealthy: false,
        })
    }

    /// Return the coordinator's generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether new operations are accepted. Exact retained duplicates remain observable.
    pub const fn is_healthy(&self) -> bool {
        !self.unhealthy
    }

    /// Fail closed after an owner-side async attachment could not be matched.
    pub(crate) fn mark_unhealthy(&mut self) {
        self.unhealthy = true;
    }

    /// Return immutable lifecycle metadata for all retained identities.
    pub fn records(&self) -> impl ExactSizeIterator<Item = SessionRecordInfo> + '_ {
        self.registry.records()
    }

    /// Look up the lifecycle state for one complete target identity.
    pub fn state(&self, target: TargetRef) -> Result<SessionState, RegistryError> {
        self.registry.state(target)
    }

    /// Record that an owned child has exited, retaining its handle as a tombstone.
    ///
    /// The observation is accepted only when the trusted effect implementation
    /// reports the child exited. No process handle is removed or replaced.
    pub fn note_exited(&mut self, target: TargetRef) -> Result<(), RegistryError>
    where
        E: EffectSink,
    {
        if !self.registry.effects(target)?.child_exited() {
            return Err(RegistryError::IllegalTransition);
        }
        self.registry.transition(target, SessionState::Exited)
    }

    /// Release the slot of one `Exited` or `Failed` session.
    ///
    /// The returned handle (if any) is handed to the caller, which decides
    /// when it is dropped. The identity stays spent and its launch result stays
    /// retained for exact duplicates; a later launch receives a fresh session
    /// ID. Live or uncertain sessions are rejected with `IllegalTransition`.
    pub fn retire(&mut self, target: TargetRef) -> Result<Option<E>, RegistryError> {
        self.registry.retire(target)
    }

    /// Whether this exact target was issued in this generation and retired.
    pub fn is_retired(&self, target: TargetRef) -> bool {
        self.registry.is_retired(target)
    }

    /// Number of retired session records in this generation.
    pub const fn retired_count(&self) -> u64 {
        self.registry.retired()
    }

    /// Observe a retained process-effects handle without transferring ownership.
    pub fn effects(&self, target: TargetRef) -> Result<&E, RegistryError> {
        self.registry.effects(target)
    }

    /// Borrow only the effect interface; the concrete owned handle cannot be swapped out.
    ///
    /// This is an ownership invariant, not a security boundary: trusted effect
    /// implementations can still shut down or mutate their own resources.
    pub fn effects_mut(&mut self, target: TargetRef) -> Result<&mut dyn EffectSink, RegistryError>
    where
        E: EffectSink,
    {
        self.registry.effects_mut(target)
    }

    /// Borrow the concrete retained target value without transferring it.
    pub(crate) fn session_mut(&mut self, target: TargetRef) -> Result<&mut E, RegistryError> {
        self.registry.retained_mut(target)
    }

    /// Execute or observe one exact schema-6 launch request.
    ///
    /// `prepare` must be a pure, fallible resolution step. `spawn` consumes its
    /// prepared value and must return `Err` only if it acquired no child handle.
    /// The caller remains responsible for authorization and for passing its one
    /// global ledger; this method creates no per-session authority or ledger.
    pub fn execute<P, PE, SE, J, Prepare, Spawn>(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        ledger: &mut Ledger,
        journal: &mut J,
        prepare: Prepare,
        spawn: Spawn,
    ) -> Result<LaunchResult, ManagedLaunchError>
    where
        J: LaunchJournalPort,
        Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
        Spawn: FnOnce(P) -> Result<E, SE>,
    {
        let request = decode_launch_request(payload).map_err(ManagedLaunchError::InvalidPayload)?;
        let key = request.key();
        if key.generation != self.generation || ledger.generation() != self.generation {
            return Err(ManagedLaunchError::GenerationMismatch);
        }

        // Exact-key identity and byte conflicts take precedence over mutable
        // capacity, health, preparation, journal, and spawn checks.
        if ledger
            .existing_admission(key, payload, now_secs)
            .map_err(ManagedLaunchError::Ledger)?
            .is_some()
        {
            return self
                .entry(key)
                .map(|entry| entry.result)
                .ok_or(ManagedLaunchError::KeyOwnedElsewhere);
        }
        if self.unhealthy || self.emergency_effect.is_some() {
            self.unhealthy = true;
            return Err(ManagedLaunchError::Unhealthy);
        }

        // Prove that the one global ledger can still retain this exact key
        // before invoking preparation or any journal callback. This preflight
        // reserves nothing; `admit` below remains the dispatch authorization.
        if ledger
            .preflight_admission(key, payload, now_secs)
            .map_err(ManagedLaunchError::Ledger)?
            .is_some()
        {
            return self
                .entry(key)
                .map(|entry| entry.result)
                .ok_or(ManagedLaunchError::KeyOwnedElsewhere);
        }

        let target = self.next_target().map_err(ManagedLaunchError::Registry)?;
        let prepared =
            prepare(request, target).map_err(|_| ManagedLaunchError::PreparationFailed)?;
        journal
            .preflight_launch(key, target.session)
            .map_err(|_| ManagedLaunchError::JournalPreflightFailed)?;

        match ledger
            .admit(key, payload, now_secs)
            .map_err(ManagedLaunchError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                self.unhealthy = true;
                return Err(ManagedLaunchError::InternalInvariant);
            }
        }

        self.run_reserved(key, target, prepared, Some(ledger), journal, spawn)
    }

    /// Run the journaled launch path for a key the trusted caller has already
    /// admitted in the one global ledger under its own (larger) payload.
    ///
    /// This performs no ledger admission, duplicate lookup, or completion:
    /// the caller owns that key's ledger record and terminal outcome. The
    /// key must be new to this coordinator. `expected_target` must equal the
    /// next registry identity, which lets the caller plan with that identity
    /// before effects. Preparation and journal-preflight failures return an
    /// error without reserving the identity. After reservation the result
    /// follows [`Self::execute`]: Intent before spawn, an acquired handle
    /// retained through `Unknown`, and the emergency slot on an attach fault.
    pub(crate) fn execute_admitted<P, PE, SE, J, Prepare, Spawn>(
        &mut self,
        request: LaunchRequestView<'_>,
        expected_target: TargetRef,
        journal: &mut J,
        prepare: Prepare,
        spawn: Spawn,
    ) -> Result<LaunchResult, ManagedLaunchError>
    where
        J: LaunchJournalPort,
        Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
        Spawn: FnOnce(P) -> Result<E, SE>,
    {
        let key = request.key();
        if key.generation != self.generation {
            return Err(ManagedLaunchError::GenerationMismatch);
        }
        if self.entry(key).is_some() {
            return Err(ManagedLaunchError::InternalInvariant);
        }
        if self.unhealthy || self.emergency_effect.is_some() {
            self.unhealthy = true;
            return Err(ManagedLaunchError::Unhealthy);
        }
        let target = self.next_target().map_err(ManagedLaunchError::Registry)?;
        if target != expected_target {
            return Err(ManagedLaunchError::InternalInvariant);
        }
        let prepared =
            prepare(request, target).map_err(|_| ManagedLaunchError::PreparationFailed)?;
        journal
            .preflight_launch(key, target.session)
            .map_err(|_| ManagedLaunchError::JournalPreflightFailed)?;
        self.run_reserved(key, target, prepared, None, journal, spawn)
    }

    /// Reserve one coordinator identity for an already-admitted asynchronous
    /// launch. No journal or process work is performed here. The owner remains
    /// responsible for retaining any subsequently acquired effect handle.
    pub(crate) fn reserve_async(
        &mut self,
        key: OperationKey,
        expected_target: TargetRef,
    ) -> Result<(), ManagedLaunchError> {
        if key.generation != self.generation {
            return Err(ManagedLaunchError::GenerationMismatch);
        }
        if self.unhealthy || self.emergency_effect.is_some() {
            self.unhealthy = true;
            return Err(ManagedLaunchError::Unhealthy);
        }
        if self.entry(key).is_some() {
            return Err(ManagedLaunchError::InternalInvariant);
        }
        let target = self.next_target().map_err(ManagedLaunchError::Registry)?;
        if target != expected_target {
            return Err(ManagedLaunchError::InternalInvariant);
        }
        let reserved = self
            .registry
            .reserve()
            .map_err(ManagedLaunchError::Registry)?;
        if reserved != target
            || self
                .store_entry(
                    key,
                    LaunchResult {
                        key,
                        outcome: Outcome::Accepted,
                        target: Some(target),
                    },
                )
                .is_err()
        {
            self.unhealthy = true;
            let _ = self.registry.transition(reserved, SessionState::Failed);
            return Err(ManagedLaunchError::InternalInvariant);
        }
        Ok(())
    }

    /// Adopt the child returned by the asynchronous worker. An unexpected
    /// attachment failure still retains the acquired handle in the emergency
    /// slot and fails the coordinator closed.
    pub(crate) fn attach_async(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        effects: E,
    ) -> Result<(), ManagedLaunchError> {
        let valid = self.entry(key).is_some_and(|entry| {
            entry.result.target == Some(target) && entry.result.outcome == Outcome::Accepted
        });
        if !valid {
            self.unhealthy = true;
            self.emergency_effect = Some(effects);
            self.store_existing_result(
                key,
                LaunchResult {
                    key,
                    outcome: Outcome::Unknown,
                    target: Some(target),
                },
            );
            return Err(ManagedLaunchError::InternalInvariant);
        }
        if self.registry.state(target) == Ok(SessionState::Reserved)
            && self
                .registry
                .transition(target, SessionState::Intent)
                .is_err()
        {
            self.unhealthy = true;
            self.emergency_effect = Some(effects);
            self.store_existing_result(
                key,
                LaunchResult {
                    key,
                    outcome: Outcome::Unknown,
                    target: Some(target),
                },
            );
            return Err(ManagedLaunchError::InternalInvariant);
        }
        if let Err(attach_error) = self.registry.attach_spawned(target, effects) {
            let (_, effects) = attach_error.into_parts();
            self.unhealthy = true;
            debug_assert!(self.emergency_effect.is_none());
            self.emergency_effect = Some(effects);
            let _ = self.registry.transition(target, SessionState::Unknown);
            self.store_existing_result(
                key,
                LaunchResult {
                    key,
                    outcome: Outcome::Unknown,
                    target: Some(target),
                },
            );
            return Err(ManagedLaunchError::InternalInvariant);
        }
        self.store_existing_result(
            key,
            LaunchResult {
                key,
                outcome: Outcome::Accepted,
                target: Some(target),
            },
        );
        Ok(())
    }

    /// Finish a reserved asynchronous launch. The caller has already completed
    /// journal work; `child_attached` selects truthful no-child failure versus
    /// post-spawn uncertainty. An observed exit is never moved backwards.
    pub(crate) fn finish_async(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        outcome: Outcome,
        child_attached: bool,
        unhealthy: bool,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        self.unhealthy |= unhealthy;
        if self
            .entry(key)
            .is_none_or(|entry| entry.result.target != Some(target))
        {
            self.unhealthy = true;
            return Err(ManagedLaunchError::InternalInvariant);
        }
        let final_outcome = if child_attached {
            if outcome == Outcome::Applied {
                Outcome::Applied
            } else {
                Outcome::Unknown
            }
        } else if outcome == Outcome::Failed {
            Outcome::Failed
        } else {
            Outcome::Unknown
        };
        let state = self.registry.state(target).map_err(|_| {
            self.unhealthy = true;
            ManagedLaunchError::InternalInvariant
        })?;
        if child_attached {
            let next = match (state, final_outcome) {
                (SessionState::Exited, _) => SessionState::Exited,
                (_, Outcome::Applied) => SessionState::Applied,
                _ => SessionState::Unknown,
            };
            if state != next && self.registry.transition(target, next).is_err() {
                self.unhealthy = true;
            }
        } else if final_outcome == Outcome::Unknown {
            let transitioned = match state {
                SessionState::Reserved => self
                    .registry
                    .transition(target, SessionState::Intent)
                    .and_then(|()| self.registry.transition(target, SessionState::Unknown)),
                SessionState::Intent => self.registry.transition(target, SessionState::Unknown),
                SessionState::Unknown => Ok(()),
                _ => Err(RegistryError::IllegalTransition),
            };
            if transitioned.is_err() {
                self.unhealthy = true;
            }
        } else if !matches!(state, SessionState::Failed)
            && self
                .registry
                .transition(target, SessionState::Failed)
                .is_err()
        {
            self.unhealthy = true;
        }
        let result = LaunchResult {
            key,
            outcome: final_outcome,
            target: Some(target),
        };
        self.store_existing_result(key, result);
        Ok(result)
    }

    /// Observe this coordinator's retained launch result for one key.
    pub(crate) fn retained_result(&self, key: OperationKey) -> Option<LaunchResult> {
        self.entry(key).map(|entry| entry.result)
    }

    /// Pure preflight of the identity the next reservation would receive.
    ///
    /// Also requires room for one more retained launch result; exhaustion is
    /// reported as `IdentifierExhausted` because no new identity may be issued.
    pub(crate) fn next_target(&self) -> Result<TargetRef, RegistryError> {
        let target = self.registry.next_target()?;
        if self.operations.len() >= MAX_RETAINED_LAUNCH_RESULTS {
            return Err(RegistryError::IdentifierExhausted);
        }
        Ok(target)
    }

    /// Reserve, journal, spawn and attach after admission and preflight.
    ///
    /// `ledger` is `None` when the caller owns the key's completion.
    fn run_reserved<P, SE, J, Spawn>(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        prepared: P,
        mut ledger: Option<&mut Ledger>,
        journal: &mut J,
        spawn: Spawn,
    ) -> Result<LaunchResult, ManagedLaunchError>
    where
        J: LaunchJournalPort,
        Spawn: FnOnce(P) -> Result<E, SE>,
    {
        let reserved = match self.registry.reserve() {
            Ok(reserved) if reserved == target => reserved,
            _ => {
                self.unhealthy = true;
                let result = LaunchResult {
                    key,
                    outcome: Outcome::Failed,
                    target: None,
                };
                let _ = self.store_entry(key, result);
                if complete_owned(&mut ledger, key, Outcome::Failed).is_err() {
                    return Err(ManagedLaunchError::InternalInvariant);
                }
                return Err(ManagedLaunchError::InternalInvariant);
            }
        };
        let accepted = LaunchResult {
            key,
            outcome: Outcome::Accepted,
            target: Some(reserved),
        };
        if self.store_entry(key, accepted).is_err() {
            // Capacity was checked before admission and no interleaving is
            // permitted. Retain the reserved identity and fail closed.
            self.unhealthy = true;
            let _ = self.registry.transition(reserved, SessionState::Failed);
            let failed = LaunchResult {
                key,
                outcome: Outcome::Failed,
                target: Some(reserved),
            };
            self.store_existing_result(key, failed);
            let _ = complete_owned(&mut ledger, key, Outcome::Failed);
            return Err(ManagedLaunchError::InternalInvariant);
        }

        let intent = LaunchRecord {
            key,
            session: reserved.session,
            state: LaunchState::Intent,
        };
        if journal.record(intent).is_err() {
            self.unhealthy = true;
            let _ = self.registry.transition(reserved, SessionState::Failed);
            return self.finish_pre_spawn(ledger.as_deref_mut(), key, reserved, Outcome::Failed);
        }
        if self
            .registry
            .transition(reserved, SessionState::Intent)
            .is_err()
        {
            self.unhealthy = true;
            let failed = LaunchResult {
                key,
                outcome: Outcome::Failed,
                target: Some(reserved),
            };
            self.store_existing_result(key, failed);
            let _ = complete_owned(&mut ledger, key, Outcome::Failed);
            return Err(ManagedLaunchError::InternalInvariant);
        }

        let effects = match spawn(prepared) {
            Ok(effects) => effects,
            Err(_) => {
                if journal
                    .record(LaunchRecord {
                        state: LaunchState::Failed,
                        ..intent
                    })
                    .is_err()
                {
                    self.unhealthy = true;
                }
                if self
                    .registry
                    .transition(reserved, SessionState::Failed)
                    .is_err()
                {
                    self.unhealthy = true;
                    return Err(ManagedLaunchError::InternalInvariant);
                }
                return self.finish_pre_spawn(
                    ledger.as_deref_mut(),
                    key,
                    reserved,
                    Outcome::Failed,
                );
            }
        };

        // Move E into coordinator ownership before invoking any further
        // journal, result, or ledger operation.
        if let Err(attach_error) = self.registry.attach_spawned(reserved, effects) {
            return self.retain_emergency_inner(
                key,
                reserved,
                intent,
                attach_error,
                ledger.as_deref_mut(),
                journal,
            );
        }

        let spawned = LaunchRecord {
            state: LaunchState::Spawned,
            ..intent
        };
        if journal.record(spawned).is_err() {
            self.unhealthy = true;
            return self.finish_post_spawn_unknown(ledger.as_deref_mut(), key, reserved);
        }
        if journal
            .record(LaunchRecord {
                state: LaunchState::Applied,
                ..intent
            })
            .is_err()
        {
            self.unhealthy = true;
            return self.finish_post_spawn_unknown(ledger.as_deref_mut(), key, reserved);
        }
        if self
            .registry
            .transition(reserved, SessionState::Applied)
            .is_err()
        {
            self.unhealthy = true;
            let _ = self.registry.transition(reserved, SessionState::Unknown);
            let _ = journal.record(LaunchRecord {
                state: LaunchState::Unknown,
                ..intent
            });
            return self.finish_post_spawn_unknown(ledger.as_deref_mut(), key, reserved);
        }

        let applied = LaunchResult {
            key,
            outcome: Outcome::Applied,
            target: Some(reserved),
        };
        self.store_existing_result(key, applied);
        if complete_owned(&mut ledger, key, Outcome::Applied).is_err() {
            self.unhealthy = true;
            let _ = self.registry.transition(reserved, SessionState::Unknown);
            let _ = journal.record(LaunchRecord {
                state: LaunchState::Unknown,
                ..intent
            });
            let unknown = LaunchResult {
                key,
                outcome: Outcome::Unknown,
                target: Some(reserved),
            };
            self.store_existing_result(key, unknown);
            return Ok(unknown);
        }
        Ok(applied)
    }

    /// Validate the original ledger key and observe this coordinator's retained result.
    pub fn status(
        &self,
        key: OperationKey,
        now_secs: u64,
        ledger: &Ledger,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        if key.generation != self.generation || ledger.generation() != self.generation {
            return Err(ManagedLaunchError::GenerationMismatch);
        }
        let ledger_status = ledger
            .status(key, now_secs)
            .map_err(ManagedLaunchError::Ledger)?;
        let entry = self.entry(key).ok_or(ManagedLaunchError::UnknownKey)?;
        match ledger_status {
            OperationStatus::NotAdmitted => Err(ManagedLaunchError::InternalInvariant),
            OperationStatus::InProgress | OperationStatus::Completed(_) => Ok(entry.result),
        }
    }

    fn entry(&self, key: OperationKey) -> Option<&OperationEntry> {
        self.operations.iter().find(|entry| entry.key == key)
    }

    fn store_entry(&mut self, key: OperationKey, result: LaunchResult) -> Result<(), ()> {
        if self.entry(key).is_some() || self.operations.len() >= MAX_RETAINED_LAUNCH_RESULTS {
            return Err(());
        }
        self.operations.push(OperationEntry { key, result });
        Ok(())
    }

    fn store_existing_result(&mut self, key: OperationKey, result: LaunchResult) {
        if let Some(entry) = self.operations.iter_mut().find(|entry| entry.key == key) {
            entry.result = result;
        } else {
            self.unhealthy = true;
        }
    }

    fn finish_pre_spawn(
        &mut self,
        mut ledger: Option<&mut Ledger>,
        key: OperationKey,
        target: TargetRef,
        outcome: Outcome,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        let result = LaunchResult {
            key,
            outcome,
            target: Some(target),
        };
        self.store_existing_result(key, result);
        if complete_owned(&mut ledger, key, outcome).is_err() {
            // A known no-child failure is still truthful locally. The bad
            // ledger invariant blocks all future new operations.
            self.unhealthy = true;
        }
        Ok(result)
    }

    fn finish_post_spawn_unknown(
        &mut self,
        mut ledger: Option<&mut Ledger>,
        key: OperationKey,
        target: TargetRef,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        self.unhealthy |= self
            .registry
            .transition(target, SessionState::Unknown)
            .is_err();
        let unknown = LaunchResult {
            key,
            outcome: Outcome::Unknown,
            target: Some(target),
        };
        self.store_existing_result(key, unknown);
        if complete_owned(&mut ledger, key, Outcome::Unknown).is_err() {
            self.unhealthy = true;
        }
        Ok(unknown)
    }

    #[cfg(test)]
    fn retain_emergency_after_attach_error<J: LaunchJournalPort>(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        intent: LaunchRecord,
        attach_error: AttachError<E>,
        ledger: &mut Ledger,
        journal: &mut J,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        self.retain_emergency_inner(key, target, intent, attach_error, Some(ledger), journal)
    }

    fn retain_emergency_inner<J: LaunchJournalPort>(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        intent: LaunchRecord,
        attach_error: AttachError<E>,
        mut ledger: Option<&mut Ledger>,
        journal: &mut J,
    ) -> Result<LaunchResult, ManagedLaunchError> {
        let (_registry_error, effects) = attach_error.into_parts();
        self.unhealthy = true;
        // The slot is empty whenever the coordinator was healthy enough to
        // reach spawn: the only writer also latches unhealthy, and all later
        // new operations are rejected before preparation or spawn.
        debug_assert!(self.emergency_effect.is_none());
        self.emergency_effect = Some(effects);
        let _ = self.registry.transition(target, SessionState::Unknown);
        let _ = journal.record(LaunchRecord {
            state: LaunchState::Unknown,
            ..intent
        });
        let unknown = LaunchResult {
            key,
            outcome: Outcome::Unknown,
            target: Some(target),
        };
        self.store_existing_result(key, unknown);
        let _ = complete_owned(&mut ledger, key, Outcome::Unknown);
        Err(ManagedLaunchError::InternalInvariant)
    }
}

/// Complete a key in the borrowed ledger, or succeed without effect when the
/// trusted caller retains ownership of that key's terminal ledger outcome.
fn complete_owned(
    ledger: &mut Option<&mut Ledger>,
    key: OperationKey,
    outcome: Outcome,
) -> Result<(), ProtocolError> {
    match ledger.as_deref_mut() {
        Some(ledger) => ledger.complete(key, outcome),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
    use crate::control_protocol::{Command, ReadSource, TargetRef};
    use crate::managed_launch_protocol::{
        encode_launch_request, encode_status_query, LaunchRequest, LaunchStatusQuery,
    };
    use crate::protocol::{
        Admission, Ledger, OperationKey, OperationStatus, Outcome, ProtocolError,
    };
    use crate::response_protocol::RejectionCode;
    use crate::server::{EffectSink, ReadResult};
    use crate::server_storage::{LaunchRecord, LaunchState};
    use crate::session_registry::{RegistryError, SessionState};
    use std::fmt;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    const GENERATION: u64 = 91;
    const LEASE: u64 = 7;
    const NOW: u64 = 10;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Prepare,
        JournalPreflight,
        Intent,
        Spawn,
        Spawned,
        Applied,
        Failed,
        Unknown,
    }

    type Events = Arc<Mutex<Vec<Event>>>;

    struct FakeJournal {
        events: Events,
        preflight_failure: bool,
        record_failure: Option<LaunchState>,
        records: Vec<LaunchRecord>,
    }

    impl FakeJournal {
        fn new(events: &Events) -> Self {
            Self {
                events: Arc::clone(events),
                preflight_failure: false,
                record_failure: None,
                records: Vec::new(),
            }
        }

        fn fail_record(state: LaunchState, events: &Events) -> Self {
            let mut journal = Self::new(events);
            journal.record_failure = Some(state);
            journal
        }
    }

    impl LaunchJournalPort for FakeJournal {
        fn preflight_launch(&self, _key: OperationKey, _session: u64) -> io::Result<()> {
            self.events.lock().unwrap().push(Event::JournalPreflight);
            if self.preflight_failure {
                Err(io::Error::other("sensitive journal detail"))
            } else {
                Ok(())
            }
        }

        fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
            let event = match record.state {
                LaunchState::Intent => Event::Intent,
                LaunchState::Spawned => Event::Spawned,
                LaunchState::Applied => Event::Applied,
                LaunchState::Failed => Event::Failed,
                LaunchState::Unknown => Event::Unknown,
            };
            self.events.lock().unwrap().push(event);
            if self.record_failure == Some(record.state) {
                return Err(io::Error::other("sensitive journal detail"));
            }
            self.records.push(record);
            Ok(())
        }
    }

    struct TrackedHandle {
        secret: &'static str,
        child_exited: bool,
        drops: Arc<AtomicUsize>,
    }

    impl TrackedHandle {
        fn new(drops: &Arc<AtomicUsize>, child_exited: bool) -> Self {
            Self {
                secret: "CHILD-HANDLE-SECRET",
                child_exited,
                drops: Arc::clone(drops),
            }
        }
    }

    impl fmt::Debug for TrackedHandle {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TrackedHandle")
                .field("secret", &self.secret)
                .finish()
        }
    }

    impl Drop for TrackedHandle {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl EffectSink for TrackedHandle {
        fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
            Ok(())
        }

        fn read(
            &mut self,
            _source: ReadSource,
            _max_text_bytes: usize,
            _max_lines: u16,
        ) -> Result<ReadResult, Outcome> {
            Ok(ReadResult {
                text: String::new(),
                revision: 1,
                truncated: false,
            })
        }

        fn input(&mut self, _command: Command<'_>) -> Outcome {
            Outcome::Applied
        }

        fn poll(&mut self) -> Result<(), Outcome> {
            Ok(())
        }

        fn child_exited(&self) -> bool {
            self.child_exited
        }
    }

    fn key(sequence: u64) -> OperationKey {
        OperationKey {
            generation: GENERATION,
            lease: LEASE,
            sequence,
        }
    }

    fn ledger() -> Ledger {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(LEASE, NOW).unwrap();
        ledger
    }

    fn payload(key: OperationKey, executable: &[u8], args: &[&[u8]]) -> Vec<u8> {
        let request = LaunchRequest {
            key,
            rows: 8,
            columns: 40,
            requested_executable: executable,
            requested_working_directory: b"/tmp",
            client_working_directory: b"/tmp",
            arguments: args,
            environment: &[],
        };
        let mut bytes = Vec::new();
        encode_launch_request(&request, &mut bytes).unwrap();
        bytes
    }

    fn setup() -> (
        ManagedLaunchCoordinator<TrackedHandle>,
        Ledger,
        Events,
        Arc<AtomicUsize>,
    ) {
        (
            ManagedLaunchCoordinator::new(GENERATION).unwrap(),
            ledger(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicUsize::new(0)),
        )
    }

    fn result_target(result: crate::managed_launch_protocol::LaunchResult) -> TargetRef {
        result
            .target
            .expect("launch result contains its reserved identity")
    }

    #[test]
    fn construction_rejects_zero_generation() {
        assert!(matches!(
            ManagedLaunchCoordinator::<TrackedHandle>::new(0),
            Err(ManagedLaunchError::Registry(_))
        ));
    }

    #[test]
    fn success_records_intent_before_spawn_and_exact_duplicates_replay_cached_result() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        let first = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| {
                    events.lock().unwrap().push(Event::Prepare);
                    Ok::<_, ()>(())
                },
                |_| {
                    events.lock().unwrap().push(Event::Spawn);
                    Ok::<_, ()>(TrackedHandle::new(&drops, false))
                },
            )
            .unwrap();
        assert_eq!(first.outcome, Outcome::Applied);
        assert_eq!(owner.state(result_target(first)), Ok(SessionState::Applied));
        assert_eq!(
            *events.lock().unwrap(),
            [
                Event::Prepare,
                Event::JournalPreflight,
                Event::Intent,
                Event::Spawn,
                Event::Spawned,
                Event::Applied,
            ]
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        // Simulate losing the first response: an exact retry gets the same
        // stored target and result without invoking any callback or journal.
        let duplicate = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("duplicate must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("duplicate must not spawn") },
            )
            .unwrap();
        assert_eq!(duplicate, first);
        assert_eq!(owner.status(key(1), NOW, &ledger).unwrap(), first);
        assert_eq!(events.lock().unwrap().len(), 6);
        drop(owner);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn changed_bytes_conflict_globally_before_capacity_or_callbacks() {
        let (mut owner, mut ledger, events, drops) = setup();
        let original = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        owner
            .execute(
                &original,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| {
                    events.lock().unwrap().push(Event::Prepare);
                    Ok::<_, ()>(())
                },
                |_| {
                    events.lock().unwrap().push(Event::Spawn);
                    Ok::<_, ()>(TrackedHandle::new(&drops, false))
                },
            )
            .unwrap();
        let changed = payload(key(1), b"/bin/zsh", &[]);
        let err = owner
            .execute(
                &changed,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("conflict must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("conflict must not spawn") },
            )
            .unwrap_err();
        assert_eq!(err.ledger_error(), Some(&ProtocolError::OperationConflict));
        assert_eq!(events.lock().unwrap().len(), 6);
    }

    #[test]
    fn cross_kind_global_key_collision_is_rejected_before_callbacks() {
        let (mut owner, mut ledger, events, _drops) = setup();
        let launch = payload(key(1), b"/bin/sh", &[]);
        let mut other_kind = Vec::new();
        encode_status_query(LaunchStatusQuery { key: key(1) }, &mut other_kind).unwrap();
        assert_eq!(
            ledger.admit(key(1), &other_kind, NOW),
            Ok(Admission::Execute)
        );
        let mut journal = FakeJournal::new(&events);
        let error = owner
            .execute(
                &launch,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("global collision must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("global collision must not spawn") },
            )
            .unwrap_err();
        assert_eq!(
            error.ledger_error(),
            Some(&ProtocolError::OperationConflict)
        );
        assert!(events.lock().unwrap().is_empty());
        assert!(owner.records().next().is_none());
    }

    #[test]
    fn known_global_key_without_local_result_never_discloses_a_target() {
        let (mut owner, mut ledger, events, _drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        assert_eq!(ledger.admit(key(1), &request, NOW), Ok(Admission::Execute));
        let mut journal = FakeJournal::new(&events);
        assert!(matches!(
            owner.execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("foreign key must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("foreign key must not spawn") },
            ),
            Err(ManagedLaunchError::KeyOwnedElsewhere)
        ));
        assert!(owner.records().next().is_none());
    }

    #[test]
    fn preflight_and_preparation_failures_do_not_admit_or_reserve() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        assert!(matches!(
            owner.execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| {
                    events.lock().unwrap().push(Event::Prepare);
                    Err::<(), _>("secret preparation diagnostic")
                },
                |_| -> Result<TrackedHandle, ()> { panic!("failed preparation must not spawn") },
            ),
            Err(ManagedLaunchError::PreparationFailed)
        ));
        assert!(events.lock().unwrap().iter().all(|e| *e == Event::Prepare));
        assert_eq!(owner.records().len(), 0);
        assert_eq!(ledger.status(key(1), NOW), Ok(OperationStatus::NotAdmitted));

        events.lock().unwrap().clear();
        journal.preflight_failure = true;
        assert!(matches!(
            owner.execute(
                &payload(key(2), b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| {
                    events.lock().unwrap().push(Event::Prepare);
                    Ok::<_, ()>(())
                },
                |_| -> Result<TrackedHandle, ()> { panic!("preflight failure must not spawn") },
            ),
            Err(ManagedLaunchError::JournalPreflightFailed)
        ));
        assert_eq!(
            *events.lock().unwrap(),
            [Event::Prepare, Event::JournalPreflight]
        );
        assert_eq!(owner.records().len(), 0);
        assert_eq!(ledger.status(key(2), NOW), Ok(OperationStatus::NotAdmitted));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn global_ledger_capacity_preflight_runs_before_preparation_and_journal() {
        let (mut owner, mut ledger, events, _drops) = setup();
        let full = vec![b'x'; crate::protocol::MAX_OPERATION_PAYLOAD_BYTES];
        for sequence in 1..=128 {
            assert_eq!(
                ledger.admit(key(sequence), &full, NOW),
                Ok(Admission::Execute)
            );
        }
        let request = payload(key(129), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        let error = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("global capacity must preflight first") },
                |_| -> Result<TrackedHandle, ()> { panic!("global capacity must prevent spawn") },
            )
            .unwrap_err();
        assert_eq!(
            error.ledger_error(),
            Some(&ProtocolError::PayloadCapacityExceeded)
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(owner.records().len(), 0);
        assert_eq!(
            ledger.status(key(129), NOW),
            Ok(OperationStatus::NotAdmitted)
        );
    }

    #[test]
    fn intent_write_failure_marks_failed_and_never_spawns() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::fail_record(LaunchState::Intent, &events);
        let result = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| -> Result<TrackedHandle, ()> { panic!("intent failure must prevent spawn") },
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(owner.state(result_target(result)), Ok(SessionState::Failed));
        assert_eq!(
            ledger.status(key(1), NOW),
            Ok(OperationStatus::Completed(Outcome::Failed))
        );
        assert_eq!(
            *events.lock().unwrap(),
            [Event::JournalPreflight, Event::Intent]
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(!owner.is_healthy());

        let duplicate = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut FakeJournal::new(&events),
                |_, _| -> Result<(), ()> { panic!("exact failed result remains observable") },
                |_| -> Result<TrackedHandle, ()> { panic!("exact failed result must not respawn") },
            )
            .unwrap();
        assert_eq!(duplicate, result);
        let new_key = payload(key(2), b"/bin/sh", &[]);
        assert!(matches!(
            owner.execute(
                &new_key,
                NOW,
                &mut ledger,
                &mut FakeJournal::new(&events),
                |_, _| -> Result<(), ()> {
                    panic!("poisoned journal latch rejects before prepare")
                },
                |_| -> Result<TrackedHandle, ()> {
                    panic!("poisoned journal latch rejects before spawn")
                },
            ),
            Err(ManagedLaunchError::Unhealthy)
        ));
        assert_eq!(ledger.status(key(2), NOW), Ok(OperationStatus::NotAdmitted));
    }

    #[test]
    fn spawn_failure_records_known_failure_without_retaining_a_handle() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        let result = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| {
                    events.lock().unwrap().push(Event::Spawn);
                    Err::<TrackedHandle, _>("secret spawn diagnostic")
                },
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(owner.state(result_target(result)), Ok(SessionState::Failed));
        assert_eq!(
            owner.effects(result_target(result)).err(),
            Some(crate::session_registry::RegistryError::EffectsUnavailable)
        );
        assert_eq!(
            ledger.status(key(1), NOW),
            Ok(OperationStatus::Completed(Outcome::Failed))
        );
        assert_eq!(
            *events.lock().unwrap(),
            [
                Event::JournalPreflight,
                Event::Intent,
                Event::Spawn,
                Event::Failed
            ]
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_state_record_fault_latches_new_launches_but_keeps_old_result() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::fail_record(LaunchState::Failed, &events);
        let result = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| Err::<TrackedHandle, _>(()),
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Failed);
        assert!(!owner.is_healthy());
        assert!(matches!(
            owner.status(key(1), NOW, &ledger),
            Ok(status) if status == result
        ));

        let duplicate = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut FakeJournal::new(&events),
                |_, _| -> Result<(), ()> { panic!("duplicate status remains available") },
                |_| -> Result<TrackedHandle, ()> { panic!("duplicate cannot retry spawn") },
            )
            .unwrap();
        assert_eq!(duplicate, result);
        assert!(matches!(
            owner.execute(
                &payload(key(2), b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut FakeJournal::new(&events),
                |_, _| -> Result<(), ()> { panic!("write failure latches before prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("write failure latches before spawn") },
            ),
            Err(ManagedLaunchError::Unhealthy)
        ));
        assert_eq!(ledger.status(key(2), NOW), Ok(OperationStatus::NotAdmitted));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn post_spawn_journal_faults_mark_unknown_and_retain_the_exact_handle() {
        for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
            let (mut owner, mut ledger, events, drops) = setup();
            let request = payload(key(1), b"/bin/sh", &[]);
            let mut journal = FakeJournal::fail_record(failed_state, &events);
            let result = owner
                .execute(
                    &request,
                    NOW,
                    &mut ledger,
                    &mut journal,
                    |_, _| Ok::<_, ()>(()),
                    |_| {
                        events.lock().unwrap().push(Event::Spawn);
                        Ok::<_, ()>(TrackedHandle::new(&drops, false))
                    },
                )
                .unwrap();
            assert_eq!(result.outcome, Outcome::Unknown);
            assert!(!owner.is_healthy());
            let target = result_target(result);
            assert_eq!(owner.state(target), Ok(SessionState::Unknown));
            assert!(owner.effects(target).is_ok());
            assert_eq!(
                ledger.status(key(1), NOW),
                Ok(OperationStatus::Completed(Outcome::Unknown))
            );
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            let before_duplicate = events.lock().unwrap().len();
            let duplicate = owner
                .execute(
                    &request,
                    NOW,
                    &mut ledger,
                    &mut journal,
                    |_, _| -> Result<(), ()> { panic!("duplicate must not prepare") },
                    |_| -> Result<TrackedHandle, ()> { panic!("duplicate must not spawn") },
                )
                .unwrap();
            assert_eq!(duplicate, result);
            assert_eq!(events.lock().unwrap().len(), before_duplicate);
            assert!(matches!(
                owner.execute(
                    &payload(key(2), b"/bin/sh", &[]),
                    NOW,
                    &mut ledger,
                    &mut FakeJournal::new(&events),
                    |_, _| -> Result<(), ()> {
                        panic!("journal write fault latches before prepare")
                    },
                    |_| -> Result<TrackedHandle, ()> {
                        panic!("journal write fault latches before spawn")
                    },
                ),
                Err(ManagedLaunchError::Unhealthy)
            ));
            assert_eq!(ledger.status(key(2), NOW), Ok(OperationStatus::NotAdmitted));
            drop(owner);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn failed_and_exited_sessions_both_consume_capacity() {
        let (mut owner, mut ledger, events, drops) = setup();
        let mut journal = FakeJournal::new(&events);
        let failed = owner
            .execute(
                &payload(key(1), b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| Err::<TrackedHandle, _>(()),
            )
            .unwrap();
        assert_eq!(failed.outcome, Outcome::Failed);
        let mut last = None;
        for sequence in 2..=crate::session_registry::SESSION_CAPACITY as u64 {
            let exited = owner
                .execute(
                    &payload(key(sequence), b"/bin/sh", &[]),
                    NOW,
                    &mut ledger,
                    &mut journal,
                    |_, _| Ok::<_, ()>(()),
                    |_| Ok::<_, ()>(TrackedHandle::new(&drops, true)),
                )
                .unwrap();
            assert_eq!(exited.outcome, Outcome::Applied);
            assert_eq!(owner.note_exited(result_target(exited)), Ok(()));
            assert_eq!(owner.state(result_target(exited)), Ok(SessionState::Exited));
            last = Some(exited);
        }
        assert_eq!(
            last.expect("at least two retained sessions").outcome,
            Outcome::Applied
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        let third = key(crate::session_registry::SESSION_CAPACITY as u64 + 1);
        assert!(matches!(
            owner.execute(
                &payload(third, b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("capacity rejects before preparation") },
                |_| -> Result<TrackedHandle, ()> { panic!("capacity rejects before spawn") },
            ),
            Err(ManagedLaunchError::Registry(
                crate::session_registry::RegistryError::Capacity
            ))
        ));
        assert_eq!(ledger.status(third, NOW), Ok(OperationStatus::NotAdmitted));
        assert_eq!(
            owner.records().count(),
            crate::session_registry::SESSION_CAPACITY
        );
        drop(owner);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            crate::session_registry::SESSION_CAPACITY - 1
        );
    }

    #[test]
    fn retiring_ended_sessions_admits_fresh_ids_and_keeps_exact_duplicates() {
        let (mut owner, mut ledger, events, drops) = setup();
        let mut journal = FakeJournal::new(&events);
        let launch = |owner: &mut ManagedLaunchCoordinator<TrackedHandle>,
                      ledger: &mut Ledger,
                      journal: &mut FakeJournal,
                      sequence: u64,
                      exited: bool| {
            let drops = Arc::clone(&drops);
            owner.execute(
                &payload(key(sequence), b"/bin/sh", &[]),
                NOW,
                ledger,
                journal,
                |_, _| Ok::<_, ()>(()),
                move |_| Ok::<_, ()>(TrackedHandle::new(&drops, exited)),
            )
        };
        let first = launch(&mut owner, &mut ledger, &mut journal, 1, true).unwrap();
        let mut second = launch(&mut owner, &mut ledger, &mut journal, 2, false).unwrap();
        for sequence in 3..=crate::session_registry::SESSION_CAPACITY as u64 {
            second = launch(&mut owner, &mut ledger, &mut journal, sequence, false).unwrap();
        }
        let first_target = result_target(first);
        for sequence in 3..=crate::session_registry::SESSION_CAPACITY as u64 {
            launch(&mut owner, &mut ledger, &mut journal, sequence, false).unwrap();
        }
        assert!(matches!(
            launch(
                &mut owner,
                &mut ledger,
                &mut journal,
                crate::session_registry::SESSION_CAPACITY as u64 + 1,
                false
            ),
            Err(ManagedLaunchError::Registry(RegistryError::Capacity))
        ));
        // Live and not-yet-exited sessions cannot be retired.
        assert_eq!(
            owner.retire(result_target(second)).err(),
            Some(RegistryError::IllegalTransition)
        );
        assert_eq!(
            owner.retire(first_target).err(),
            Some(RegistryError::IllegalTransition)
        );
        owner.note_exited(first_target).unwrap();
        let handle = owner.retire(first_target).unwrap().expect("handle");
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(handle);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(owner.is_retired(first_target));
        assert_eq!(owner.retired_count(), 1);
        assert_eq!(owner.state(first_target), Err(RegistryError::InvalidTarget));

        // An exact duplicate of the retired launch returns its original
        // result without preparation or spawn.
        let duplicate = owner
            .execute(
                &payload(key(1), b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("duplicate must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("duplicate must not spawn") },
            )
            .unwrap();
        assert_eq!(duplicate, first);
        assert_eq!(owner.status(key(1), NOW, &ledger).unwrap(), first);

        let third = launch(
            &mut owner,
            &mut ledger,
            &mut journal,
            crate::session_registry::SESSION_CAPACITY as u64 + 1,
            false,
        )
        .unwrap();
        let third_target = result_target(third);
        assert_eq!(third.outcome, Outcome::Applied);
        assert_eq!(
            third_target.session,
            crate::session_registry::SESSION_CAPACITY as u64 + 1
        );
        assert_ne!(third_target, first_target);
        assert_eq!(
            owner.records().count(),
            crate::session_registry::SESSION_CAPACITY
        );
        assert!(matches!(
            launch(
                &mut owner,
                &mut ledger,
                &mut journal,
                crate::session_registry::SESSION_CAPACITY as u64 + 2,
                false
            ),
            Err(ManagedLaunchError::Registry(RegistryError::Capacity))
        ));
    }

    #[test]
    fn retained_launch_results_are_bounded() {
        let (mut owner, _ledger, _events, _drops) = setup();
        for sequence in 1..=super::MAX_RETAINED_LAUNCH_RESULTS as u64 {
            owner
                .store_entry(
                    key(sequence),
                    crate::managed_launch_protocol::LaunchResult {
                        key: key(sequence),
                        outcome: Outcome::Failed,
                        target: None,
                    },
                )
                .unwrap();
        }
        assert_eq!(owner.next_target(), Err(RegistryError::IdentifierExhausted));
    }

    #[test]
    fn wrong_generation_and_expired_lease_are_rejected_before_callbacks() {
        let (mut owner, mut ledger, events, _drops) = setup();
        let wrong = OperationKey {
            generation: GENERATION + 1,
            ..key(1)
        };
        let mut journal = FakeJournal::new(&events);
        assert!(matches!(
            owner.execute(
                &payload(wrong, b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("wrong generation must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("wrong generation must not spawn") },
            ),
            Err(ManagedLaunchError::GenerationMismatch)
        ));

        let expired = payload(key(1), b"/bin/sh", &[]);
        assert!(matches!(
            owner.execute(
                &expired,
                NOW + crate::protocol::LEASE_TTL_SECS,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("expired lease must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("expired lease must not spawn") },
            ),
            Err(ManagedLaunchError::Ledger(ProtocolError::LeaseExpired))
        ));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn attach_invariant_fault_retains_child_in_emergency_slot_and_latches_unhealthy() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let mut journal = FakeJournal::new(&events);
        // Exercise the exact post-spawn rejection path without mutating public
        // coordinator state: reserve, durably intent, then attach against a
        // deliberately stale full target.
        let target = owner.registry.reserve().unwrap();
        owner
            .registry
            .transition(target, SessionState::Intent)
            .unwrap();
        assert_eq!(ledger.admit(key(1), &request, NOW), Ok(Admission::Execute));
        let accepted = crate::managed_launch_protocol::LaunchResult {
            key: key(1),
            outcome: Outcome::Accepted,
            target: Some(target),
        };
        owner.store_entry(key(1), accepted).unwrap();
        let intent = LaunchRecord {
            key: key(1),
            session: 1,
            state: LaunchState::Intent,
        };
        journal.record(intent).unwrap();
        let wrong_target = TargetRef {
            occupant_revision: 2,
            ..target
        };
        let attach_error = owner
            .registry
            .attach_spawned(wrong_target, TrackedHandle::new(&drops, false))
            .unwrap_err();
        assert!(matches!(
            owner.retain_emergency_after_attach_error(
                key(1),
                target,
                intent,
                attach_error,
                &mut ledger,
                &mut journal
            ),
            Err(ManagedLaunchError::InternalInvariant)
        ));
        assert!(!owner.is_healthy());
        assert_eq!(owner.state(target), Ok(SessionState::Unknown));
        assert_eq!(
            ledger.status(key(1), NOW),
            Ok(OperationStatus::Completed(Outcome::Unknown))
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(format!("{owner:?}").contains("emergency_effect_retained: true"));
        assert!(matches!(
            owner.execute(
                &payload(key(2), b"/bin/sh", &[]),
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("unhealthy blocks before preparation") },
                |_| -> Result<TrackedHandle, ()> { panic!("unhealthy blocks before spawn") },
            ),
            Err(ManagedLaunchError::Unhealthy)
        ));
        let duplicate = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("retained duplicate is observable") },
                |_| -> Result<TrackedHandle, ()> { panic!("retained duplicate is observable") },
            )
            .unwrap();
        assert_eq!(duplicate.outcome, Outcome::Unknown);
        drop(owner);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn execute_admitted_runs_journaled_spawn_without_touching_the_callers_ledger_key() {
        let (mut owner, mut ledger, events, drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        // The caller admitted this key under its own larger payload.
        let outer = [request.as_slice(), b"outer-envelope"].concat();
        assert_eq!(ledger.admit(key(1), &outer, NOW), Ok(Admission::Execute));
        let view = crate::managed_launch_protocol::decode_launch_request(&request).unwrap();
        let expected = owner.next_target().unwrap();
        let mut journal = FakeJournal::new(&events);
        let result = owner
            .execute_admitted(
                view,
                expected,
                &mut journal,
                |_, target| {
                    assert_eq!(target, expected);
                    events.lock().unwrap().push(Event::Prepare);
                    Ok::<_, ()>(())
                },
                |_| {
                    events.lock().unwrap().push(Event::Spawn);
                    Ok::<_, ()>(TrackedHandle::new(&drops, false))
                },
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Applied);
        assert_eq!(result.target, Some(expected));
        assert_eq!(owner.retained_result(key(1)), Some(result));
        assert_eq!(
            *events.lock().unwrap(),
            [
                Event::Prepare,
                Event::JournalPreflight,
                Event::Intent,
                Event::Spawn,
                Event::Spawned,
                Event::Applied,
            ]
        );
        // The caller still owns completion of its admitted key.
        assert_eq!(ledger.status(key(1), NOW), Ok(OperationStatus::InProgress));
        // A second inner run for the same key is refused before callbacks.
        assert!(matches!(
            owner.execute_admitted(
                view,
                owner.next_target().unwrap(),
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("known key must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("known key must not spawn") },
            ),
            Err(ManagedLaunchError::InternalInvariant)
        ));
    }

    #[test]
    fn execute_admitted_rejects_unplanned_identity_and_unreserved_failures_spend_nothing() {
        let (mut owner, _ledger, events, _drops) = setup();
        let request = payload(key(1), b"/bin/sh", &[]);
        let view = crate::managed_launch_protocol::decode_launch_request(&request).unwrap();
        let planned = owner.next_target().unwrap();
        let wrong = TargetRef {
            session: planned.session + 1,
            pane: planned.pane + 1,
            ..planned
        };
        let mut journal = FakeJournal::new(&events);
        assert!(matches!(
            owner.execute_admitted(
                view,
                wrong,
                &mut journal,
                |_, _| -> Result<(), ()> { panic!("unplanned identity must not prepare") },
                |_| -> Result<TrackedHandle, ()> { panic!("unplanned identity must not spawn") },
            ),
            Err(ManagedLaunchError::InternalInvariant)
        ));
        assert!(matches!(
            owner.execute_admitted(
                view,
                planned,
                &mut journal,
                |_, _| Err::<(), _>("secret"),
                |_| -> Result<TrackedHandle, ()> { panic!("failed preparation must not spawn") },
            ),
            Err(ManagedLaunchError::PreparationFailed)
        ));
        journal.preflight_failure = true;
        assert!(matches!(
            owner.execute_admitted(
                view,
                planned,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| -> Result<TrackedHandle, ()> { panic!("failed preflight must not spawn") },
            ),
            Err(ManagedLaunchError::JournalPreflightFailed)
        ));
        assert_eq!(owner.records().len(), 0);
        assert_eq!(owner.retained_result(key(1)), None);
        assert_eq!(owner.next_target(), Ok(planned));
        assert!(owner.is_healthy());
    }

    #[test]
    fn diagnostics_redact_payload_callback_journal_and_owned_handle_data() {
        let (mut owner, mut ledger, events, drops) = setup();
        let secret = b"SECRET-ARGUMENT-AND-ENV";
        let request = payload(key(1), b"/bin/sh", &[secret]);
        let mut journal = FakeJournal::fail_record(LaunchState::Intent, &events);
        let error = owner
            .execute(
                &request,
                NOW,
                &mut ledger,
                &mut journal,
                |view, _| {
                    assert_eq!(view.arguments().next(), Some(secret.as_slice()));
                    Err::<(), _>("SECRET-CALLBACK-DIAGNOSTIC")
                },
                |_| Ok::<_, ()>(TrackedHandle::new(&drops, false)),
            )
            .unwrap_err();
        let rendered = format!("{error:?} {error} {owner:?}");
        assert!(!rendered.contains("SECRET"));
        assert!(!format!(
            "{:?}",
            crate::managed_launch_protocol::decode_launch_request(&request).unwrap()
        )
        .contains("SECRET"));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}
