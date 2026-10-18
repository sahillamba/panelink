//! Finite, in-memory ownership for one session's non-interleaved input stream.
//!
//! This module has no UI, process, or persistence effects. A caller must finish
//! a pinned operation as `Partial` or `Unknown` before clearing an expired owner;
//! this state machine never drops a pending input or hands it to another owner.

use crate::protocol::OperationKey;
use std::error::Error;
use std::fmt;

/// Maximum prototype ownership lease duration.
pub const MAX_OWNER_TTL_SECS: u64 = 30;
/// Minimum prototype ownership lease duration.
pub const MIN_OWNER_TTL_SECS: u64 = 1;

/// Client or human attachment that currently owns a session's input stream.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum Owner {
    /// Reusable client lease identifier.
    Client(u64),
    /// Interactive human attachment identifier.
    Human(u64),
}

impl fmt::Debug for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(_) => f.write_str("Client(..)"),
            Self::Human(_) => f.write_str("Human(..)"),
        }
    }
}

impl Owner {
    fn is_valid(self) -> bool {
        match self {
            Self::Client(id) | Self::Human(id) => id != 0,
        }
    }
}

/// Immutable view of the current owner lease and pinned input operation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OwnerSnapshot {
    /// Current owner, if a lease is held.
    pub holder: Option<Owner>,
    /// Compare-and-set revision; the initial empty state is revision 1.
    pub revision: u64,
    /// Absolute caller-supplied monotonic time when the lease expires; zero if vacant.
    pub expires_secs: u64,
    /// The one operation whose input may currently be in progress.
    pub pending: Option<OperationKey>,
}

impl fmt::Debug for OwnerSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerSnapshot")
            .field("has_owner", &self.holder.is_some())
            .field("revision", &self.revision)
            .field("expires_secs", &self.expires_secs)
            .field("has_pending_input", &self.pending.is_some())
            .finish()
    }
}

/// Result of checking an ownership lease's expiry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpireResult {
    /// No lease is held.
    Vacant,
    /// The current lease has not reached its expiry time.
    NotDue,
    /// The lease expired but a pinned operation still needs resolution.
    PendingNeedsResolution(OperationKey),
    /// The expired lease was released and its revision advanced.
    Released(OwnerSnapshot),
}

/// State-machine errors for input ownership and pinning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerError {
    /// An owner identifier, revision, or operation-key component is zero.
    ZeroIdentifier,
    /// The requested lease duration is outside the 1–30 second prototype range.
    InvalidTtl,
    /// The requested compare-and-set revision is stale.
    StaleRevision { expected: u64, current: u64 },
    /// Another owner currently holds the input lease.
    HeldByAnotherOwner,
    /// No input lease is currently held.
    Vacant,
    /// The caller does not match the current owner.
    WrongOwner,
    /// Handoff recipient is already the current owner.
    RecipientIsCurrentOwner,
    /// The current lease has expired.
    Expired,
    /// One input operation is already pinned.
    InputPending,
    /// There is no pinned input operation to finish.
    NoPendingInput,
    /// The supplied key does not match the pinned operation.
    PendingKeyMismatch,
    /// Adding a TTL to the supplied logical time would overflow.
    TimeOverflow,
    /// Advancing the ownership revision would overflow.
    RevisionExhausted,
}

impl fmt::Display for OwnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroIdentifier => f.write_str("owner and operation identifiers must be nonzero"),
            Self::InvalidTtl => f.write_str("owner TTL must be between 1 and 30 seconds"),
            Self::StaleRevision { .. } => f.write_str("owner revision is stale"),
            Self::HeldByAnotherOwner => f.write_str("input is owned by another attachment"),
            Self::Vacant => f.write_str("input has no current owner"),
            Self::WrongOwner => f.write_str("caller is not the current input owner"),
            Self::RecipientIsCurrentOwner => {
                f.write_str("handoff recipient is already the current input owner")
            }
            Self::Expired => f.write_str("input owner lease has expired"),
            Self::InputPending => f.write_str("an input operation is still pending"),
            Self::NoPendingInput => f.write_str("no input operation is pending"),
            Self::PendingKeyMismatch => f.write_str("operation key does not match pending input"),
            Self::TimeOverflow => f.write_str("owner expiry time overflowed"),
            Self::RevisionExhausted => f.write_str("owner revision space is exhausted"),
        }
    }
}

impl Error for OwnerError {}

/// Pure ownership and single-input-operation state for one session.
#[derive(Clone)]
pub struct InputOwner {
    holder: Option<Owner>,
    revision: u64,
    expires_secs: u64,
    pending: Option<OperationKey>,
}

impl fmt::Debug for InputOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputOwner")
            .field("has_owner", &self.holder.is_some())
            .field("revision", &self.revision)
            .field("expires_secs", &self.expires_secs)
            .field("has_pending_input", &self.pending.is_some())
            .finish()
    }
}

impl Default for InputOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl InputOwner {
    /// Create a vacant input owner at revision 1.
    pub const fn new() -> Self {
        Self {
            holder: None,
            revision: 1,
            expires_secs: 0,
            pending: None,
        }
    }

    /// Return the current state without changing it.
    pub const fn snapshot(&self) -> OwnerSnapshot {
        OwnerSnapshot {
            holder: self.holder,
            revision: self.revision,
            expires_secs: self.expires_secs,
            pending: self.pending,
        }
    }

    /// Claim a vacant lease or explicitly renew the same owner at the exact revision.
    ///
    /// Every successful claim/renewal advances the revision. `now_secs` must come
    /// from a monotonic server clock. Expired holders are released separately by
    /// [`InputOwner::expire`]; a new claimant cannot silently replace one.
    pub fn claim(
        &mut self,
        owner: Owner,
        expected_revision: u64,
        ttl_secs: u64,
        now_secs: u64,
    ) -> Result<OwnerSnapshot, OwnerError> {
        validate_owner(owner)?;
        validate_ttl(ttl_secs)?;
        let expires_secs = now_secs
            .checked_add(ttl_secs)
            .ok_or(OwnerError::TimeOverflow)?;
        self.ensure_revision(expected_revision)?;
        if self.pending.is_some() {
            return Err(OwnerError::InputPending);
        }
        match self.holder {
            Some(current) if current != owner => return Err(OwnerError::HeldByAnotherOwner),
            Some(_) if now_secs >= self.expires_secs => return Err(OwnerError::Expired),
            _ => {}
        }
        let revision = self.next_revision()?;
        self.holder = Some(owner);
        self.revision = revision;
        self.expires_secs = expires_secs;
        Ok(self.snapshot())
    }

    /// Release the lease if caller identity and compare-and-set revision match.
    ///
    /// A pending operation blocks release even if its lease has expired.
    pub fn release(
        &mut self,
        caller: Owner,
        expected_revision: u64,
    ) -> Result<OwnerSnapshot, OwnerError> {
        validate_owner(caller)?;
        self.ensure_revision(expected_revision)?;
        if self.pending.is_some() {
            return Err(OwnerError::InputPending);
        }
        self.ensure_current_owner(caller)?;
        let revision = self.next_revision()?;
        self.holder = None;
        self.revision = revision;
        self.expires_secs = 0;
        Ok(self.snapshot())
    }

    /// Hand the lease to another attachment using exact caller and revision checks.
    pub fn handoff(
        &mut self,
        caller: Owner,
        expected_revision: u64,
        recipient: Owner,
        ttl_secs: u64,
        now_secs: u64,
    ) -> Result<OwnerSnapshot, OwnerError> {
        validate_owner(caller)?;
        validate_owner(recipient)?;
        validate_ttl(ttl_secs)?;
        let expires_secs = now_secs
            .checked_add(ttl_secs)
            .ok_or(OwnerError::TimeOverflow)?;
        self.ensure_revision(expected_revision)?;
        if self.pending.is_some() {
            return Err(OwnerError::InputPending);
        }
        self.ensure_current_owner(caller)?;
        if now_secs >= self.expires_secs {
            return Err(OwnerError::Expired);
        }
        if recipient == caller {
            return Err(OwnerError::RecipientIsCurrentOwner);
        }
        let revision = self.next_revision()?;
        self.holder = Some(recipient);
        self.revision = revision;
        self.expires_secs = expires_secs;
        Ok(self.snapshot())
    }

    /// Verify exact owner/revision and that the lease is still live.
    pub fn validate(
        &self,
        owner: Owner,
        expected_revision: u64,
        now_secs: u64,
    ) -> Result<(), OwnerError> {
        validate_owner(owner)?;
        self.ensure_revision(expected_revision)?;
        self.ensure_current_owner(owner)?;
        if now_secs >= self.expires_secs {
            return Err(OwnerError::Expired);
        }
        Ok(())
    }

    /// Validate and pin one non-replayable input operation.
    pub fn begin_input(
        &mut self,
        owner: Owner,
        expected_revision: u64,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<OwnerSnapshot, OwnerError> {
        if !valid_key(key) {
            return Err(OwnerError::ZeroIdentifier);
        }
        if self.pending.is_some() {
            return Err(OwnerError::InputPending);
        }
        self.validate(owner, expected_revision, now_secs)?;
        self.pending = Some(key);
        Ok(self.snapshot())
    }

    /// Clear only the exact pinned operation after its effect owner resolves it.
    pub fn finish_input(&mut self, key: OperationKey) -> Result<OwnerSnapshot, OwnerError> {
        if !valid_key(key) {
            return Err(OwnerError::ZeroIdentifier);
        }
        match self.pending {
            None => Err(OwnerError::NoPendingInput),
            Some(pending) if pending != key => Err(OwnerError::PendingKeyMismatch),
            Some(_) => {
                self.pending = None;
                Ok(self.snapshot())
            }
        }
    }

    /// Release a due lease, retaining it while an input effect still needs resolution.
    pub fn expire(&mut self, now_secs: u64) -> Result<ExpireResult, OwnerError> {
        if self.holder.is_none() {
            return Ok(ExpireResult::Vacant);
        }
        if now_secs < self.expires_secs {
            return Ok(ExpireResult::NotDue);
        }
        if let Some(key) = self.pending {
            return Ok(ExpireResult::PendingNeedsResolution(key));
        }
        let revision = self.next_revision()?;
        self.holder = None;
        self.revision = revision;
        self.expires_secs = 0;
        Ok(ExpireResult::Released(self.snapshot()))
    }

    fn ensure_revision(&self, expected_revision: u64) -> Result<(), OwnerError> {
        if expected_revision == self.revision {
            Ok(())
        } else {
            Err(OwnerError::StaleRevision {
                expected: expected_revision,
                current: self.revision,
            })
        }
    }

    fn ensure_current_owner(&self, caller: Owner) -> Result<(), OwnerError> {
        match self.holder {
            None => Err(OwnerError::Vacant),
            Some(current) if current != caller => Err(OwnerError::WrongOwner),
            Some(_) => Ok(()),
        }
    }

    fn next_revision(&self) -> Result<u64, OwnerError> {
        self.revision
            .checked_add(1)
            .ok_or(OwnerError::RevisionExhausted)
    }
}

fn validate_owner(owner: Owner) -> Result<(), OwnerError> {
    if owner.is_valid() {
        Ok(())
    } else {
        Err(OwnerError::ZeroIdentifier)
    }
}

fn validate_ttl(ttl_secs: u64) -> Result<(), OwnerError> {
    if (MIN_OWNER_TTL_SECS..=MAX_OWNER_TTL_SECS).contains(&ttl_secs) {
        Ok(())
    } else {
        Err(OwnerError::InvalidTtl)
    }
}

fn valid_key(key: OperationKey) -> bool {
    key.generation != 0 && key.lease != 0 && key.sequence != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: Owner = Owner::Client(2);
    const OTHER_CLIENT: Owner = Owner::Client(3);
    const HUMAN: Owner = Owner::Human(4);
    const KEY: OperationKey = OperationKey {
        generation: 5,
        lease: 2,
        sequence: 1,
    };

    #[test]
    fn claim_renew_and_release_advance_revision_and_expiry() {
        let mut owner = InputOwner::new();
        assert_eq!(owner.snapshot().revision, 1);
        let claimed = owner.claim(CLIENT, 1, 30, 10).unwrap();
        assert_eq!(claimed.holder, Some(CLIENT));
        assert_eq!(claimed.revision, 2);
        assert_eq!(claimed.expires_secs, 40);
        let renewed = owner.claim(CLIENT, 2, 1, 11).unwrap();
        assert_eq!(renewed.revision, 3);
        assert_eq!(renewed.expires_secs, 12);
        let released = owner.release(CLIENT, 3).unwrap();
        assert_eq!(released.holder, None);
        assert_eq!(released.revision, 4);
        assert_eq!(released.expires_secs, 0);
    }

    #[test]
    fn claim_while_held_requires_same_owner_exact_revision() {
        let mut owner = InputOwner::new();
        owner.claim(CLIENT, 1, 5, 1).unwrap();
        assert_eq!(
            owner.claim(OTHER_CLIENT, 2, 5, 2),
            Err(OwnerError::HeldByAnotherOwner)
        );
        assert!(matches!(
            owner.claim(CLIENT, 1, 5, 2),
            Err(OwnerError::StaleRevision { .. })
        ));
        assert_eq!(owner.snapshot().revision, 2);
    }

    #[test]
    fn stale_expired_and_wrong_owner_calls_are_rejected() {
        let mut owner = InputOwner::new();
        owner.claim(CLIENT, 1, 2, 10).unwrap();
        assert_eq!(owner.validate(CLIENT, 2, 12), Err(OwnerError::Expired));
        assert_eq!(owner.release(OTHER_CLIENT, 2), Err(OwnerError::WrongOwner));
        assert_eq!(
            owner.release(CLIENT, 1),
            Err(OwnerError::StaleRevision {
                expected: 1,
                current: 2
            })
        );
        assert_eq!(
            owner.expire(12),
            Ok(ExpireResult::Released(OwnerSnapshot {
                holder: None,
                revision: 3,
                expires_secs: 0,
                pending: None
            }))
        );
    }

    #[test]
    fn handoff_checks_owner_revision_expiry_and_recipient() {
        let mut owner = InputOwner::new();
        owner.claim(CLIENT, 1, 10, 5).unwrap();
        assert_eq!(
            owner.handoff(OTHER_CLIENT, 2, HUMAN, 5, 6),
            Err(OwnerError::WrongOwner)
        );
        assert_eq!(
            owner.handoff(CLIENT, 2, HUMAN, 5, 15),
            Err(OwnerError::Expired)
        );
        assert_eq!(
            owner.handoff(CLIENT, 2, CLIENT, 5, 6),
            Err(OwnerError::RecipientIsCurrentOwner)
        );
        let handed = owner.handoff(CLIENT, 2, HUMAN, 5, 6).unwrap();
        assert_eq!(handed.holder, Some(HUMAN));
        assert_eq!(handed.revision, 3);
        assert_eq!(handed.expires_secs, 11);
    }

    #[test]
    fn one_input_is_pinned_until_its_exact_key_finishes() {
        let mut owner = InputOwner::new();
        let claimed = owner.claim(CLIENT, 1, 1, 20).unwrap();
        let pending = owner
            .begin_input(CLIENT, claimed.revision, KEY, 20)
            .unwrap();
        assert_eq!(pending.pending, Some(KEY));
        assert_eq!(
            owner.begin_input(
                CLIENT,
                claimed.revision,
                OperationKey { sequence: 2, ..KEY },
                20
            ),
            Err(OwnerError::InputPending)
        );
        assert_eq!(
            owner.finish_input(OperationKey { sequence: 2, ..KEY }),
            Err(OwnerError::PendingKeyMismatch)
        );
        assert_eq!(
            owner.release(CLIENT, claimed.revision),
            Err(OwnerError::InputPending)
        );
        assert_eq!(
            owner.handoff(CLIENT, claimed.revision, HUMAN, 1, 20),
            Err(OwnerError::InputPending)
        );
        assert_eq!(
            owner.claim(CLIENT, claimed.revision, 1, 20),
            Err(OwnerError::InputPending)
        );
        assert_eq!(owner.finish_input(KEY).unwrap().pending, None);
        assert_eq!(owner.finish_input(KEY), Err(OwnerError::NoPendingInput));
    }

    #[test]
    fn expiry_retains_identity_when_pending_even_after_ttl() {
        let mut owner = InputOwner::new();
        let claimed = owner.claim(CLIENT, 1, 1, 8).unwrap();
        owner.begin_input(CLIENT, claimed.revision, KEY, 8).unwrap();
        assert_eq!(
            owner.expire(100),
            Ok(ExpireResult::PendingNeedsResolution(KEY))
        );
        assert_eq!(owner.snapshot().holder, Some(CLIENT));
        owner.finish_input(KEY).unwrap();
        assert!(matches!(owner.expire(100), Ok(ExpireResult::Released(_))));
    }

    #[test]
    fn rejects_zero_ids_invalid_ttl_and_time_overflow_without_mutation() {
        let mut owner = InputOwner::new();
        assert_eq!(
            owner.claim(Owner::Client(0), 1, 1, 0),
            Err(OwnerError::ZeroIdentifier)
        );
        assert_eq!(owner.claim(CLIENT, 1, 0, 0), Err(OwnerError::InvalidTtl));
        assert_eq!(
            owner.claim(CLIENT, 1, 1, u64::MAX),
            Err(OwnerError::TimeOverflow)
        );
        assert_eq!(owner.snapshot(), InputOwner::new().snapshot());
        let bad_key = OperationKey { sequence: 0, ..KEY };
        assert_eq!(
            owner.begin_input(CLIENT, 1, bad_key, 0),
            Err(OwnerError::ZeroIdentifier)
        );
    }

    #[test]
    fn revision_overflow_and_time_overflow_leave_state_unchanged() {
        let mut owner = InputOwner::new();
        owner.revision = u64::MAX;
        assert_eq!(
            owner.claim(CLIENT, u64::MAX, 1, 0),
            Err(OwnerError::RevisionExhausted)
        );
        assert_eq!(owner.holder, None);
        assert_eq!(owner.revision, u64::MAX);

        owner.holder = Some(CLIENT);
        owner.expires_secs = 1;
        assert_eq!(
            owner.handoff(CLIENT, u64::MAX, HUMAN, 1, u64::MAX),
            Err(OwnerError::TimeOverflow)
        );
        assert_eq!(owner.holder, Some(CLIENT));
        assert_eq!(owner.revision, u64::MAX);
    }

    #[test]
    fn pending_expiry_does_not_consume_revision() {
        let mut owner = InputOwner::new();
        let claimed = owner.claim(CLIENT, 1, 1, 0).unwrap();
        owner.begin_input(CLIENT, claimed.revision, KEY, 0).unwrap();
        let before = owner.snapshot();
        assert_eq!(
            owner.expire(1),
            Ok(ExpireResult::PendingNeedsResolution(KEY))
        );
        assert_eq!(owner.snapshot(), before);
    }

    #[test]
    fn ttl_bounds_are_exactly_one_through_thirty_seconds() {
        let mut owner = InputOwner::new();
        assert_eq!(owner.claim(CLIENT, 1, 31, 0), Err(OwnerError::InvalidTtl));
        let result = owner.claim(CLIENT, 1, 30, 0).unwrap();
        assert_eq!(result.expires_secs, 30);
    }
}
