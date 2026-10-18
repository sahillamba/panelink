//! Fixed-capacity session identity and lifecycle storage for the managed-launch
//! prototype.
//!
//! This module owns exactly two non-reusable session records for one nonzero
//! generation. It does not create processes, persist intents, authenticate
//! callers, manage grants, or own a command ledger. Its caller must keep one
//! generation-wide ledger and grant table and durably record an intent before
//! spawning a child.
//!
//! The target mapping is synthetic prototype scaffolding only: workspace 1,
//! pane equal to session ID, session ID allocated from 1, and occupant revision
//! 1. It does not define a user-visible workspace or pane hierarchy.

use crate::control_protocol::TargetRef;
use std::error::Error;
use std::fmt;

/// Maximum number of session identities retained in one generation.
pub const SESSION_CAPACITY: usize = 2;

/// Lifecycle state recorded for one reserved session identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    /// Identity reserved in memory; no durable launch intent exists yet.
    Reserved,
    /// The caller reports that a durable intent was recorded before spawn.
    Intent,
    /// A spawned process handle is owned by this registry.
    Spawned,
    /// Registration and the launch result were committed by the caller.
    Applied,
    /// A known pre-spawn failure occurred before a process handle was acquired.
    Failed,
    /// A post-spawn result is uncertain; an acquired handle remains retained.
    Unknown,
    /// The child exited; its handle remains retained as a tombstone.
    Exited,
}

/// Fixed, redacted registry errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    /// A registry generation must be nonzero.
    InvalidGeneration,
    /// Both session identities for this generation have been spent.
    Capacity,
    /// The next session identity cannot be advanced without wrapping.
    IdentifierExhausted,
    /// The complete supplied target does not identify a reserved record.
    InvalidTarget,
    /// The requested lifecycle transition is not permitted.
    IllegalTransition,
    /// The requested record does not own a process-effects handle.
    EffectsUnavailable,
    /// A process-effects handle is already attached to this record.
    HandleAlreadyAttached,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidGeneration => "invalid registry generation",
            Self::Capacity => "session registry capacity exhausted",
            Self::IdentifierExhausted => "session identifier space exhausted",
            Self::InvalidTarget => "session target is invalid or stale",
            Self::IllegalTransition => "session lifecycle transition is not permitted",
            Self::EffectsUnavailable => "session effects are unavailable",
            Self::HandleAlreadyAttached => "session already owns an effects handle",
        })
    }
}

impl Error for RegistryError {}

/// A rejected handle attachment, returning ownership of the supplied handle.
///
/// The handle is intentionally omitted from `Debug` and is returned to the
/// caller through [`AttachError::into_parts`], so rejection never drops it
/// implicitly.
pub struct AttachError<E> {
    error: RegistryError,
    effects: E,
}

impl<E> AttachError<E> {
    /// Return the fixed error and the unchanged handle supplied by the caller.
    pub fn into_parts(self) -> (RegistryError, E) {
        (self.error, self.effects)
    }

    /// Return the fixed error without exposing the retained handle.
    pub const fn error(&self) -> RegistryError {
        self.error
    }
}

impl<E> fmt::Debug for AttachError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachError")
            .field("error", &self.error)
            .field("effects", &"<redacted>")
            .finish()
    }
}

impl<E> fmt::Display for AttachError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

impl<E: 'static> Error for AttachError<E> {}

/// Immutable information about a reserved identity.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SessionRecordInfo {
    /// The full target identity assigned to this record.
    pub target: TargetRef,
    /// Current lifecycle state.
    pub state: SessionState,
    /// Whether the registry retains the process-effects handle.
    pub has_effects: bool,
}

impl fmt::Debug for SessionRecordInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionRecordInfo")
            .field("target", &"<redacted>")
            .field("state", &self.state)
            .field("has_effects", &self.has_effects)
            .finish()
    }
}

struct SessionSlot<E> {
    target: TargetRef,
    state: SessionState,
    effects: Option<E>,
}

/// Two fixed session slots and a monotonic identity counter for one generation.
pub struct SessionRegistry<E> {
    generation: u64,
    slots: [Option<SessionSlot<E>>; SESSION_CAPACITY],
    len: usize,
    next_session: u64,
}

impl<E> fmt::Debug for SessionRegistry<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionRegistry")
            .field("generation", &"<redacted>")
            .field("used_slots", &self.len)
            .field("records", &"<redacted>")
            .finish()
    }
}

impl<E> SessionRegistry<E> {
    /// Create an empty registry for one nonzero server generation.
    pub fn new(generation: u64) -> Result<Self, RegistryError> {
        if generation == 0 {
            return Err(RegistryError::InvalidGeneration);
        }

        Ok(Self {
            generation,
            slots: std::array::from_fn(|_| None),
            len: 0,
            next_session: 1,
        })
    }

    /// Return the generation for which this registry was created.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Return the full identity that the next reservation would receive.
    ///
    /// This is a pure preflight: it applies the same capacity and checked-ID
    /// checks as [`Self::reserve`] without changing registry state.
    pub fn next_target(&self) -> Result<TargetRef, RegistryError> {
        if self.len >= SESSION_CAPACITY {
            return Err(RegistryError::Capacity);
        }
        self.next_session
            .checked_add(1)
            .ok_or(RegistryError::IdentifierExhausted)?;
        if self.next_session == 0 {
            return Err(RegistryError::IdentifierExhausted);
        }

        Ok(TargetRef {
            generation: self.generation,
            workspace: 1,
            pane: self.next_session,
            session: self.next_session,
            occupant_revision: 1,
        })
    }

    /// Reserve the next complete target identity without performing effects.
    ///
    /// Capacity and checked-ID availability are verified before any slot or
    /// counter is mutated. Reserved IDs remain spent through every later state.
    pub fn reserve(&mut self) -> Result<TargetRef, RegistryError> {
        let target = self.next_target()?;
        let next_session = target
            .session
            .checked_add(1)
            .ok_or(RegistryError::IdentifierExhausted)?;
        self.slots[self.len] = Some(SessionSlot {
            target,
            state: SessionState::Reserved,
            effects: None,
        });
        self.len += 1;
        self.next_session = next_session;
        Ok(target)
    }

    /// Advance one exact target through a permitted non-spawn transition.
    ///
    /// `Intent -> Spawned` is only available through [`Self::attach_spawned`],
    /// which installs the owned handle in the same mutation. Repeating the
    /// current state is idempotent. `Intent -> Unknown` represents uncertainty
    /// before a handle has been acquired; after a handle is acquired, callers
    /// must first attach it and then mark the record unknown. An exited record
    /// retains its original handle as a tombstone for the generation lifetime.
    pub fn transition(
        &mut self,
        target: TargetRef,
        next: SessionState,
    ) -> Result<(), RegistryError> {
        let index = self.index_for(target)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(RegistryError::InvalidTarget)?;

        if slot.state == next {
            return Ok(());
        }
        if !transition_allowed(slot.state, next) {
            return Err(RegistryError::IllegalTransition);
        }
        if next == SessionState::Failed && slot.effects.is_some() {
            return Err(RegistryError::IllegalTransition);
        }
        if matches!(next, SessionState::Applied | SessionState::Exited) && slot.effects.is_none() {
            return Err(RegistryError::IllegalTransition);
        }

        self.slots[index]
            .as_mut()
            .ok_or(RegistryError::InvalidTarget)?
            .state = next;
        Ok(())
    }

    /// Attach the spawned child handle while atomically advancing Intent to Spawned.
    ///
    /// This method never replaces an existing handle. On every rejection the
    /// exact supplied value is returned to the caller inside [`AttachError`].
    pub fn attach_spawned(&mut self, target: TargetRef, effects: E) -> Result<(), AttachError<E>> {
        let index = match self.index_for(target) {
            Ok(index) => index,
            Err(error) => return Err(AttachError { error, effects }),
        };
        let slot = match self.slots[index].as_ref() {
            Some(slot) => slot,
            None => {
                return Err(AttachError {
                    error: RegistryError::InvalidTarget,
                    effects,
                });
            }
        };

        if slot.effects.is_some() {
            return Err(AttachError {
                error: RegistryError::HandleAlreadyAttached,
                effects,
            });
        }
        if slot.state != SessionState::Intent {
            return Err(AttachError {
                error: RegistryError::IllegalTransition,
                effects,
            });
        }

        let slot = self.slots[index]
            .as_mut()
            .expect("the indexed registry slot remains present");
        slot.effects = Some(effects);
        slot.state = SessionState::Spawned;
        Ok(())
    }

    /// Return the state for an exact, complete target identity.
    pub fn state(&self, target: TargetRef) -> Result<SessionState, RegistryError> {
        let index = self.index_for(target)?;
        self.slots[index]
            .as_ref()
            .map(|slot| slot.state)
            .ok_or(RegistryError::InvalidTarget)
    }

    /// Borrow the effects handle retained by an exact target.
    pub fn effects(&self, target: TargetRef) -> Result<&E, RegistryError> {
        let index = self.index_for(target)?;
        self.slots[index]
            .as_ref()
            .and_then(|slot| slot.effects.as_ref())
            .ok_or(RegistryError::EffectsUnavailable)
    }

    /// Mutably borrow the effects interface retained by an exact target.
    ///
    /// The trait object prevents replacing or taking the concrete owned `E`
    /// through this accessor. An `EffectSink` implementation is trusted code
    /// and may still shut down or internally mutate its own resources; this is
    /// an ownership invariant, not a security boundary.
    pub fn effects_mut(
        &mut self,
        target: TargetRef,
    ) -> Result<&mut dyn crate::server::EffectSink, RegistryError>
    where
        E: crate::server::EffectSink,
    {
        let index = self.index_for(target)?;
        self.slots[index]
            .as_mut()
            .and_then(|slot| slot.effects.as_mut())
            .map(|effects| effects as &mut dyn crate::server::EffectSink)
            .ok_or(RegistryError::EffectsUnavailable)
    }

    /// Borrow the concrete handle retained by an exact target without taking
    /// ownership or exposing the containing slot.
    pub(crate) fn retained_mut(&mut self, target: TargetRef) -> Result<&mut E, RegistryError> {
        let index = self.index_for(target)?;
        self.slots[index]
            .as_mut()
            .and_then(|slot| slot.effects.as_mut())
            .ok_or(RegistryError::EffectsUnavailable)
    }

    /// Iterate over the at-most-two immutable identity records.
    pub fn records(&self) -> impl ExactSizeIterator<Item = SessionRecordInfo> + '_ {
        self.slots[..self.len].iter().map(|slot| {
            let slot = slot
                .as_ref()
                .expect("reserved registry slots remain contiguous");
            SessionRecordInfo {
                target: slot.target,
                state: slot.state,
                has_effects: slot.effects.is_some(),
            }
        })
    }

    fn index_for(&self, target: TargetRef) -> Result<usize, RegistryError> {
        self.slots[..self.len]
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|record| record.target == target))
            .ok_or(RegistryError::InvalidTarget)
    }
}

fn transition_allowed(current: SessionState, next: SessionState) -> bool {
    match current {
        SessionState::Reserved => matches!(next, SessionState::Intent | SessionState::Failed),
        SessionState::Intent => matches!(next, SessionState::Failed | SessionState::Unknown),
        SessionState::Spawned => matches!(
            next,
            SessionState::Applied | SessionState::Unknown | SessionState::Exited
        ),
        SessionState::Applied => matches!(next, SessionState::Unknown | SessionState::Exited),
        SessionState::Unknown => next == SessionState::Exited,
        SessionState::Failed | SessionState::Exited => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{AttachError, RegistryError, SessionRegistry, SessionState, SESSION_CAPACITY};
    use crate::control_protocol::{Command, ReadSource, TargetRef};
    use crate::protocol::Outcome;
    use crate::response_protocol::RejectionCode;
    use crate::server::{EffectSink, ReadResult};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct TrackedHandle {
        id: u64,
        secret: &'static str,
        drops: Arc<AtomicUsize>,
    }

    impl TrackedHandle {
        fn new(id: u64, secret: &'static str, drops: &Arc<AtomicUsize>) -> Self {
            Self {
                id,
                secret,
                drops: Arc::clone(drops),
            }
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
            false
        }
    }

    fn empty_registry() -> (SessionRegistry<TrackedHandle>, Arc<AtomicUsize>) {
        let drops = Arc::new(AtomicUsize::new(0));
        (SessionRegistry::new(9).unwrap(), drops)
    }

    fn intent_target(registry: &mut SessionRegistry<TrackedHandle>) -> TargetRef {
        let target = registry.reserve().unwrap();
        registry.transition(target, SessionState::Intent).unwrap();
        target
    }

    fn registry_in_state(
        desired: SessionState,
    ) -> (SessionRegistry<TrackedHandle>, TargetRef, Arc<AtomicUsize>) {
        let (mut registry, drops) = empty_registry();
        let target = registry.reserve().unwrap();
        match desired {
            SessionState::Reserved => {}
            SessionState::Intent => registry.transition(target, SessionState::Intent).unwrap(),
            SessionState::Failed => registry.transition(target, SessionState::Failed).unwrap(),
            SessionState::Spawned => {
                registry.transition(target, SessionState::Intent).unwrap();
                registry
                    .attach_spawned(target, TrackedHandle::new(11, "secret-handle", &drops))
                    .unwrap();
            }
            SessionState::Applied | SessionState::Unknown | SessionState::Exited => {
                registry.transition(target, SessionState::Intent).unwrap();
                registry
                    .attach_spawned(target, TrackedHandle::new(12, "secret-handle", &drops))
                    .unwrap();
                if desired != SessionState::Spawned {
                    registry.transition(target, desired).unwrap();
                }
            }
        }
        (registry, target, drops)
    }

    #[test]
    fn new_requires_nonzero_generation_and_reserves_the_synthetic_full_target() {
        assert!(matches!(
            SessionRegistry::<TrackedHandle>::new(0),
            Err(RegistryError::InvalidGeneration)
        ));

        let (mut registry, _) = empty_registry();
        assert_eq!(registry.generation(), 9);
        assert_eq!(registry.records().len(), 0);
        let target = registry.reserve().unwrap();
        assert_eq!(
            target,
            TargetRef {
                generation: 9,
                workspace: 1,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            }
        );
        assert_eq!(registry.state(target), Ok(SessionState::Reserved));
        assert_eq!(
            registry.effects(target).err(),
            Some(RegistryError::EffectsUnavailable)
        );
    }

    #[test]
    fn repeated_next_target_preflight_is_pure_and_matches_reserve() {
        let (mut registry, drops) = empty_registry();
        let first_peek = registry.next_target().unwrap();
        let second_peek = registry.next_target().unwrap();
        assert_eq!(first_peek, second_peek);
        assert_eq!(registry.records().len(), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        let reserved = registry.reserve().unwrap();
        assert_eq!(reserved, first_peek);
        assert_eq!(registry.records().len(), 1);
        assert_eq!(registry.state(reserved), Ok(SessionState::Reserved));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_and_exited_records_spend_capacity_without_reuse() {
        assert_eq!(SESSION_CAPACITY, 2);
        let (mut registry, drops) = empty_registry();
        let first = registry.reserve().unwrap();
        registry.transition(first, SessionState::Failed).unwrap();

        let second = intent_target(&mut registry);
        registry
            .attach_spawned(second, TrackedHandle::new(20, "secret", &drops))
            .unwrap();
        registry.transition(second, SessionState::Exited).unwrap();

        assert_eq!(registry.records().len(), 2);
        let records: Vec<_> = registry.records().collect();
        assert_eq!(records[0].state, SessionState::Failed);
        assert_eq!(records[1].state, SessionState::Exited);
        assert!(records[1].has_effects);
        assert_eq!(registry.reserve(), Err(RegistryError::Capacity));
        assert_eq!(registry.state(first), Ok(SessionState::Failed));
        assert_eq!(registry.state(second), Ok(SessionState::Exited));
        assert_eq!(registry.effects(second).unwrap().id, 20);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reserve_preflights_capacity_and_checked_ids_before_mutation() {
        let (mut empty, _) = empty_registry();
        empty.next_session = u64::MAX;
        assert_eq!(empty.next_target(), Err(RegistryError::IdentifierExhausted));
        assert_eq!(empty.reserve(), Err(RegistryError::IdentifierExhausted));
        assert_eq!(empty.records().len(), 0);
        assert_eq!(empty.next_session, u64::MAX);

        let (mut full, _) = empty_registry();
        let first = full.reserve().unwrap();
        let second = full.reserve().unwrap();
        full.next_session = u64::MAX;
        assert_eq!(full.next_target(), Err(RegistryError::Capacity));
        assert_eq!(full.reserve(), Err(RegistryError::Capacity));
        assert_eq!(full.records().len(), SESSION_CAPACITY);
        assert_eq!(full.state(first), Ok(SessionState::Reserved));
        assert_eq!(full.state(second), Ok(SessionState::Reserved));
        assert_eq!(full.next_session, u64::MAX);
    }

    #[test]
    fn all_target_fields_are_checked_and_sessions_cannot_alias() {
        let (mut registry, _) = empty_registry();
        let first = registry.reserve().unwrap();
        let second = registry.reserve().unwrap();
        assert_ne!(first, second);

        let mutations = [
            TargetRef {
                generation: 10,
                ..first
            },
            TargetRef {
                workspace: 2,
                ..first
            },
            TargetRef { pane: 2, ..first },
            TargetRef {
                session: 2,
                ..first
            },
            TargetRef {
                occupant_revision: 2,
                ..first
            },
        ];
        for stale in mutations {
            assert_eq!(registry.state(stale), Err(RegistryError::InvalidTarget));
            assert_eq!(
                registry.transition(stale, SessionState::Intent),
                Err(RegistryError::InvalidTarget)
            );
            assert_eq!(
                registry.effects(stale).err(),
                Some(RegistryError::InvalidTarget)
            );
            assert_eq!(
                registry.effects_mut(stale).err(),
                Some(RegistryError::InvalidTarget)
            );
        }

        assert_eq!(registry.state(first), Ok(SessionState::Reserved));
        assert_eq!(registry.state(second), Ok(SessionState::Reserved));
        assert_eq!(first.session, 1);
        assert_eq!(
            second,
            TargetRef {
                generation: 9,
                workspace: 1,
                pane: 2,
                session: 2,
                occupant_revision: 1,
            }
        );
    }

    #[test]
    fn only_the_explicit_transition_matrix_is_accepted() {
        let states = [
            SessionState::Reserved,
            SessionState::Intent,
            SessionState::Spawned,
            SessionState::Applied,
            SessionState::Failed,
            SessionState::Unknown,
            SessionState::Exited,
        ];

        for current in states {
            for next in states {
                let (mut registry, target, _) = registry_in_state(current);
                let expected = current == next
                    || matches!(
                        (current, next),
                        (SessionState::Reserved, SessionState::Intent)
                            | (SessionState::Reserved, SessionState::Failed)
                            | (SessionState::Intent, SessionState::Failed)
                            | (SessionState::Intent, SessionState::Unknown)
                            | (SessionState::Spawned, SessionState::Applied)
                            | (SessionState::Spawned, SessionState::Unknown)
                            | (SessionState::Spawned, SessionState::Exited)
                            | (SessionState::Applied, SessionState::Unknown)
                            | (SessionState::Applied, SessionState::Exited)
                            | (SessionState::Unknown, SessionState::Exited)
                    );

                if expected {
                    registry.transition(target, next).unwrap();
                    assert_eq!(registry.state(target), Ok(next));
                    // Exact duplicates are idempotent, including terminal states.
                    registry.transition(target, next).unwrap();
                } else {
                    assert_eq!(
                        registry.transition(target, next),
                        Err(RegistryError::IllegalTransition),
                        "{current:?} -> {next:?}"
                    );
                    assert_eq!(registry.state(target), Ok(current));
                }
            }
        }
    }

    #[test]
    fn attach_is_the_only_spawn_transition_and_rejections_return_the_handle() {
        let (mut registry, drops) = empty_registry();
        let target = intent_target(&mut registry);
        registry
            .attach_spawned(target, TrackedHandle::new(31, "retained-secret", &drops))
            .unwrap();
        assert_eq!(registry.state(target), Ok(SessionState::Spawned));

        let rejected = registry
            .attach_spawned(target, TrackedHandle::new(32, "returned-secret", &drops))
            .unwrap_err();
        assert_eq!(rejected.error(), RegistryError::HandleAlreadyAttached);
        let (reason, returned) = rejected.into_parts();
        assert_eq!(reason, RegistryError::HandleAlreadyAttached);
        assert_eq!(returned.id, 32);
        assert_eq!(returned.secret, "returned-secret");
        assert_eq!(registry.effects(target).unwrap().id, 31);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(returned);
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        let bad_target = TargetRef {
            occupant_revision: 99,
            ..target
        };
        let rejected = registry
            .attach_spawned(
                bad_target,
                TrackedHandle::new(33, "bad-target-secret", &drops),
            )
            .unwrap_err();
        let (reason, returned) = rejected.into_parts();
        assert_eq!(reason, RegistryError::InvalidTarget);
        assert_eq!(returned.id, 33);
        drop(returned);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        assert_eq!(registry.effects(target).unwrap().id, 31);
    }

    #[test]
    fn spawn_handle_attachment_rejects_non_intent_state_without_dropping_value() {
        let (mut registry, drops) = empty_registry();
        let target = registry.reserve().unwrap();
        let rejected = registry
            .attach_spawned(target, TrackedHandle::new(35, "returned", &drops))
            .unwrap_err();
        let (reason, returned) = rejected.into_parts();
        assert_eq!(reason, RegistryError::IllegalTransition);
        assert_eq!(returned.id, 35);
        assert_eq!(registry.state(target), Ok(SessionState::Reserved));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(returned);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unknown_and_exited_keep_the_original_spawned_handle() {
        let (mut registry, drops) = empty_registry();
        let target = intent_target(&mut registry);
        registry
            .attach_spawned(target, TrackedHandle::new(41, "secret", &drops))
            .unwrap();
        registry.effects_mut(target).unwrap().poll().unwrap();
        registry.transition(target, SessionState::Unknown).unwrap();
        assert_eq!(registry.effects(target).unwrap().id, 41);
        assert!(registry.records().next().unwrap().has_effects);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        registry.transition(target, SessionState::Exited).unwrap();
        assert_eq!(registry.effects(target).unwrap().id, 41);
        assert_eq!(
            registry.records().next().unwrap().state,
            SessionState::Exited
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(registry);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unknown_without_an_acquired_handle_cannot_become_exited() {
        let (mut registry, drops) = empty_registry();
        let target = intent_target(&mut registry);
        registry.transition(target, SessionState::Unknown).unwrap();
        assert_eq!(
            registry.transition(target, SessionState::Exited),
            Err(RegistryError::IllegalTransition)
        );
        assert_eq!(
            registry.effects(target).err(),
            Some(RegistryError::EffectsUnavailable)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reserve_and_intent_have_no_effect_handle_or_drop_side_effect() {
        let (mut registry, drops) = empty_registry();
        let target = registry.reserve().unwrap();
        assert_eq!(
            registry.effects(target).err(),
            Some(RegistryError::EffectsUnavailable)
        );
        registry.transition(target, SessionState::Intent).unwrap();
        assert_eq!(
            registry.effects(target).err(),
            Some(RegistryError::EffectsUnavailable)
        );
        assert_eq!(registry.state(target), Ok(SessionState::Intent));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn custom_debug_redacts_identity_and_does_not_format_handles() {
        let (mut registry, drops) = empty_registry();
        let target = intent_target(&mut registry);
        let registry_debug = format!("{registry:?}");
        assert!(registry_debug.contains("<redacted>"));
        assert!(!registry_debug.contains("secret-handle"));
        assert!(!registry_debug.contains("generation: 9"));

        let rejected: AttachError<TrackedHandle> = registry
            .attach_spawned(
                TargetRef {
                    session: 99,
                    ..target
                },
                TrackedHandle::new(51, "never-print-this", &drops),
            )
            .unwrap_err();
        let error_debug = format!("{rejected:?}");
        assert!(error_debug.contains("<redacted>"));
        assert!(!error_debug.contains("never-print-this"));
        assert!(!error_debug.contains("session: 99"));

        let info_debug = format!("{:?}", registry.records().next().unwrap());
        assert!(info_debug.contains("<redacted>"));
        assert!(!info_debug.contains("generation: 9"));
    }
}
