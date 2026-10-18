//! In-memory operation admission and receipt tracking for one server generation.
//!
//! This module does not execute operations, perform I/O, serialize records, or
//! promise recovery across a server restart. Payloads are kept only in volatile
//! memory so a repeated operation key can be compared byte-for-byte.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

/// Maximum semantic payload retained for one operation, in bytes.
pub const MAX_OPERATION_PAYLOAD_BYTES: usize = 65_536;

/// Maximum number of unreceipted operations retained for one lease.
pub const MAX_UNRECEIPTED_OPERATIONS: usize = 1_024;

/// Lease lifetime in seconds. Callers provide monotonic logical timestamps.
pub const LEASE_TTL_SECS: u64 = 1_800;

/// Maximum number of lease records retained for one generation.
///
/// Expired records count toward this permanent per-generation limit.
pub const MAX_LEASES_PER_GENERATION: usize = 4_096;

/// Maximum number of leases not yet marked expired by explicit maintenance.
pub const MAX_ACTIVE_LEASES: usize = 64;

/// Maximum number of retained operations across every lease in one generation.
pub const MAX_RETAINED_OPERATIONS: usize = 4_096;

/// Maximum exact operation payload bytes retained across one generation.
pub const MAX_RETAINED_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Stored ledger resource counts.
///
/// Payload bytes are logical payload lengths retained for duplicate comparison,
/// not allocator capacity or a process resident-memory measurement. A lease is
/// active until explicit maintenance marks it expired, even after its TTL has
/// elapsed according to a caller-supplied timestamp.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LedgerUsage {
    /// Lease records retained permanently for this generation.
    pub lease_records: usize,
    /// Leases not yet marked expired by [`Ledger::expire_leases`].
    pub active_leases: usize,
    /// Operation records retained across all leases, including expired pending work.
    pub retained_operations: usize,
    /// Logical payload bytes retained across all operation records.
    pub retained_payload_bytes: usize,
}

/// Stable identity for an operation within one server generation and lease.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperationKey {
    /// Unique server-start generation supplied by the caller.
    pub generation: u64,
    /// Lease identity within this generation.
    pub lease: u64,
    /// Strictly increasing sequence for new operations in this lease.
    pub sequence: u64,
}

/// Side-effect category used to classify reconnect behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationClass {
    /// Read-only list, status, snapshot, or history observation.
    Query,
    /// Durable workspace changes and launch/resume requests.
    DurableMutation,
    /// Keystrokes, paste, mouse input, or prompts submitted to a child.
    Input,
    /// Interrupt or termination signal.
    Signal,
    /// Clipboard read or write side effect.
    Clipboard,
    /// Absolute desired viewport size.
    AbsoluteResize,
}

/// Safe automatic reconnect treatment for an operation class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDisposition {
    /// Reissue as a new observation; it does not repeat a side effect.
    Reissue,
    /// Retransmit only the identical key and payload to the same live lease.
    SameKeySamePayload,
    /// Never resend automatically; ask for a fresh explicit user action.
    Never,
    /// Reconcile ownership and current state before issuing a new observation.
    Reconcile,
}

impl OperationClass {
    /// Return the reconnect classification required for this operation class.
    pub const fn retry_disposition(self) -> RetryDisposition {
        match self {
            Self::Query => RetryDisposition::Reissue,
            Self::DurableMutation => RetryDisposition::SameKeySamePayload,
            Self::Input | Self::Signal | Self::Clipboard => RetryDisposition::Never,
            Self::AbsoluteResize => RetryDisposition::Reconcile,
        }
    }
}

/// Terminal operation result retained until a cumulative receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The server reserved and accepted the operation; execution may be pending.
    Accepted,
    /// The defined side effect finished.
    Applied,
    /// The operation failed before any side effect, when that is known.
    Failed,
    /// A partial effect occurred or was reported.
    Partial,
    /// The outcome cannot be determined safely.
    Unknown,
}

/// Result of an operation admission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    /// This call reserved a new operation key. The caller may now dispatch it.
    Execute,
    /// The operation is recorded and still pending.
    InProgress,
    /// The operation has a terminal result.
    Completed(Outcome),
}

/// Read-only view of an operation's admission state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationStatus {
    /// No operation with this sequence is recorded in the still-valid lease.
    /// This observation does not reserve the key or authorize execution.
    NotAdmitted,
    /// The operation is recorded and still pending.
    InProgress,
    /// The operation has a terminal result.
    Completed(Outcome),
}

/// Errors produced by ledger admission, completion, status, and receipt calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// A server generation of zero is not valid.
    ZeroGeneration,
    /// A lease identifier of zero is not valid.
    ZeroLease,
    /// An operation sequence or receipt sequence of zero is not valid.
    ZeroSequence,
    /// The payload exceeds [`MAX_OPERATION_PAYLOAD_BYTES`].
    PayloadTooLarge {
        /// Maximum accepted payload size.
        max_bytes: usize,
        /// Supplied payload size.
        actual_bytes: usize,
    },
    /// The key belongs to a different server generation.
    WrongGeneration {
        /// Generation owned by this ledger.
        expected: u64,
        /// Generation carried by the key.
        received: u64,
    },
    /// The lease ID is already present in this generation.
    LeaseAlreadyOpen,
    /// No lease with this ID is present in this generation.
    UnknownLease,
    /// The lease has expired and cannot admit or report operations.
    LeaseExpired,
    /// Adding the lease TTL would overflow the timestamp representation.
    TimeOverflow,
    /// The requested operation sequence was already retired by a receipt.
    ResultExpired,
    /// An unknown sequence is at or below the high-water mark, so admission
    /// cannot safely determine whether it was previously used.
    UnknownOldSequence,
    /// A new operation sequence must be greater than the lease high-water mark.
    SequenceNotIncreasing {
        /// Greatest sequence admitted so far.
        highest: u64,
        /// Sequence submitted for admission.
        received: u64,
    },
    /// The same key was submitted with different payload bytes.
    OperationConflict,
    /// The lease has reached its unreceipted operation limit.
    CapacityExceeded,
    /// This generation has reached its permanent lease-record limit.
    LeaseCapacityExceeded,
    /// This generation has reached its active lease limit.
    ActiveLeaseCapacityExceeded,
    /// This generation has reached its retained operation-record limit.
    GlobalOperationCapacityExceeded,
    /// This generation has reached its retained payload-byte limit.
    PayloadCapacityExceeded,
    /// No greater nonzero lease identifier can be issued.
    LeaseIdExhausted,
    /// No admitted operation exists for this key to complete.
    UnknownOperation,
    /// A completion cannot use `Outcome::Accepted` as a terminal result.
    NotTerminalOutcome,
    /// A completed operation cannot be changed to a different outcome.
    CompletionConflict,
    /// A receipt names a sequence beyond the greatest admitted sequence.
    ReceiptBeyondHighest {
        /// Greatest sequence admitted so far.
        highest: u64,
        /// Requested receipt sequence.
        received: u64,
    },
    /// A receipt cannot retire an operation that is still pending.
    ReceiptIncludesPending {
        /// Pending operation sequence blocking the receipt.
        sequence: u64,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroGeneration => f.write_str("generation must be nonzero"),
            Self::ZeroLease => f.write_str("lease ID must be nonzero"),
            Self::ZeroSequence => f.write_str("sequence must be nonzero"),
            Self::PayloadTooLarge {
                max_bytes,
                actual_bytes,
            } => write!(
                f,
                "operation payload is {actual_bytes} bytes; maximum is {max_bytes}"
            ),
            Self::WrongGeneration { expected, received } => write!(
                f,
                "operation generation {received} does not match ledger generation {expected}"
            ),
            Self::LeaseAlreadyOpen => f.write_str("lease ID has already been opened"),
            Self::UnknownLease => f.write_str("lease ID is not known to this generation"),
            Self::LeaseExpired => f.write_str("lease has expired"),
            Self::TimeOverflow => f.write_str("lease expiry timestamp overflowed"),
            Self::ResultExpired => f.write_str("operation result has been retired"),
            Self::UnknownOldSequence => {
                f.write_str("unknown sequence is below the admission high-water mark")
            }
            Self::SequenceNotIncreasing { highest, received } => write!(
                f,
                "new sequence {received} must be greater than high-water mark {highest}"
            ),
            Self::OperationConflict => {
                f.write_str("operation key was reused with different payload bytes")
            }
            Self::CapacityExceeded => f.write_str("lease has reached its unreceipted limit"),
            Self::LeaseCapacityExceeded => {
                f.write_str("generation has reached its lease record limit")
            }
            Self::ActiveLeaseCapacityExceeded => {
                f.write_str("generation has reached its active lease limit")
            }
            Self::GlobalOperationCapacityExceeded => {
                f.write_str("generation has reached its retained operation limit")
            }
            Self::PayloadCapacityExceeded => {
                f.write_str("generation has reached its retained payload limit")
            }
            Self::LeaseIdExhausted => f.write_str("lease ID space is exhausted"),
            Self::UnknownOperation => f.write_str("operation is not admitted"),
            Self::NotTerminalOutcome => f.write_str("completion outcome must be terminal"),
            Self::CompletionConflict => f.write_str("operation already has a different outcome"),
            Self::ReceiptBeyondHighest { highest, received } => write!(
                f,
                "receipt sequence {received} exceeds high-water mark {highest}"
            ),
            Self::ReceiptIncludesPending { sequence } => write!(
                f,
                "receipt cannot retire pending operation sequence {sequence}"
            ),
        }
    }
}

impl Error for ProtocolError {}

struct OperationRecord {
    payload: Vec<u8>,
    state: OperationState,
}

#[derive(Clone, Copy)]
enum OperationState {
    Accepted,
    Completed(Outcome),
}

struct LeaseState {
    expires_at_secs: u64,
    expired: bool,
    highest_sequence: u64,
    retired_through: u64,
    operations: BTreeMap<u64, OperationRecord>,
}

/// Volatile operation ledger for one server generation.
///
/// The ledger stores exact payload bytes to compare duplicates, but does not
/// expose or format them. Create a fresh ledger for each generation. This type
/// is neither persistent nor synchronized for concurrent access; a server must
/// synchronize access to mutable ledger operations, serialize calls, and
/// reserve an operation before dispatching side effects.
pub struct Ledger {
    generation: u64,
    leases: HashMap<u64, LeaseState>,
    issued_through: u64,
    usage: LedgerUsage,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("generation", &self.generation)
            .field("lease_count", &self.usage.lease_records)
            .field("active_lease_count", &self.usage.active_leases)
            .field("retained_operation_count", &self.usage.retained_operations)
            .field("retained_payload_bytes", &self.usage.retained_payload_bytes)
            .finish()
    }
}

impl Ledger {
    /// Create an empty ledger for a nonzero server generation.
    pub fn new(generation: u64) -> Result<Self, ProtocolError> {
        if generation == 0 {
            return Err(ProtocolError::ZeroGeneration);
        }
        Ok(Self {
            generation,
            leases: HashMap::new(),
            issued_through: 0,
            usage: LedgerUsage::default(),
        })
    }

    /// Return the generation owned by this ledger.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Return stored ledger resource counts without consulting a clock.
    pub const fn usage(&self) -> LedgerUsage {
        self.usage
    }

    /// Open a lease with a fixed 30-minute TTL.
    ///
    /// Lease IDs remain reserved for the lifetime of this ledger, including
    /// after expiry, so an expired ID can never be reopened.
    pub fn open_lease(&mut self, id: u64, now_secs: u64) -> Result<(), ProtocolError> {
        if id == 0 {
            return Err(ProtocolError::ZeroLease);
        }
        if let Some(existing) = self.leases.get(&id) {
            if existing.expired || now_secs >= existing.expires_at_secs {
                return Err(ProtocolError::LeaseExpired);
            }
            return Err(ProtocolError::LeaseAlreadyOpen);
        }
        let expires_at_secs = now_secs
            .checked_add(LEASE_TTL_SECS)
            .ok_or(ProtocolError::TimeOverflow)?;
        if self.usage.lease_records >= MAX_LEASES_PER_GENERATION {
            return Err(ProtocolError::LeaseCapacityExceeded);
        }
        if self.usage.active_leases >= MAX_ACTIVE_LEASES {
            return Err(ProtocolError::ActiveLeaseCapacityExceeded);
        }
        self.leases.insert(
            id,
            LeaseState {
                expires_at_secs,
                expired: false,
                highest_sequence: 0,
                retired_through: 0,
                operations: BTreeMap::new(),
            },
        );
        self.usage.lease_records += 1;
        self.usage.active_leases += 1;
        self.issued_through = self.issued_through.max(id);
        Ok(())
    }

    /// Issue the next lease ID above every successfully opened ID.
    ///
    /// Failed issuance does not consume an ID. This volatile per-ledger
    /// allocator is not a durable generation allocator or an authentication
    /// mechanism; a future server must provide its own generation lifecycle.
    pub fn issue_lease(&mut self, now_secs: u64) -> Result<u64, ProtocolError> {
        let id = self
            .issued_through
            .checked_add(1)
            .ok_or(ProtocolError::LeaseIdExhausted)?;
        self.open_lease(id, now_secs)?;
        Ok(id)
    }

    /// Mark due leases expired and reclaim their completed metadata and payloads.
    ///
    /// The scan is bounded by [`MAX_LEASES_PER_GENERATION`] lease records and
    /// [`MAX_RETAINED_OPERATIONS`] operation records. Pending operation
    /// identities remain so already accepted work can complete; expiry does
    /// not cancel that work. Calling maintenance again also removes terminal
    /// records completed after the lease was first marked expired.
    pub fn expire_leases(&mut self, now_secs: u64) -> usize {
        let mut newly_expired = 0;
        let mut removed_operations = 0;
        let mut released_payload_bytes = 0;

        for lease in self.leases.values_mut() {
            if !lease.expired && now_secs < lease.expires_at_secs {
                continue;
            }

            if !lease.expired {
                lease.expired = true;
                newly_expired += 1;
            }

            let operation_count = lease.operations.len();
            for record in lease.operations.values_mut() {
                let payload = std::mem::take(&mut record.payload);
                released_payload_bytes += payload.len();
                drop(payload);
            }
            lease
                .operations
                .retain(|_, record| matches!(record.state, OperationState::Accepted));
            removed_operations += operation_count - lease.operations.len();
        }

        self.usage.active_leases -= newly_expired;
        self.usage.retained_operations -= removed_operations;
        self.usage.retained_payload_bytes -= released_payload_bytes;
        newly_expired
    }

    /// Renew a still-active lease for another 30 minutes from `now_secs`.
    pub fn renew_lease(&mut self, id: u64, now_secs: u64) -> Result<(), ProtocolError> {
        if id == 0 {
            return Err(ProtocolError::ZeroLease);
        }
        let lease = self
            .leases
            .get_mut(&id)
            .ok_or(ProtocolError::UnknownLease)?;
        if lease.expired || now_secs >= lease.expires_at_secs {
            return Err(ProtocolError::LeaseExpired);
        }
        lease.expires_at_secs = now_secs
            .checked_add(LEASE_TTL_SECS)
            .ok_or(ProtocolError::TimeOverflow)?;
        Ok(())
    }

    /// Admit an operation or return the record already held for its key.
    ///
    /// Admission validates the lease expiry before checking for duplicates.
    /// New keys must increase the sequence high-water mark. A duplicate returns
    /// the saved state only when its exact payload bytes match. Per-lease and
    /// global capacity failures happen before modifying the ledger.
    pub fn admit(
        &mut self,
        key: OperationKey,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Admission, ProtocolError> {
        if let Some(existing) = self.existing_admission(key, payload, now_secs)? {
            return Ok(existing);
        }
        let lease = self.leases.get(&key.lease).expect("lease was validated");
        if lease.operations.len() >= MAX_UNRECEIPTED_OPERATIONS {
            return Err(ProtocolError::CapacityExceeded);
        }
        if self.usage.retained_operations >= MAX_RETAINED_OPERATIONS {
            return Err(ProtocolError::GlobalOperationCapacityExceeded);
        }
        let retained_payload_bytes = self
            .usage
            .retained_payload_bytes
            .checked_add(payload.len())
            .filter(|bytes| *bytes <= MAX_RETAINED_PAYLOAD_BYTES)
            .ok_or(ProtocolError::PayloadCapacityExceeded)?;

        let lease = self
            .leases
            .get_mut(&key.lease)
            .expect("lease was checked before admission");
        lease.operations.insert(
            key.sequence,
            OperationRecord {
                payload: payload.to_vec(),
                state: OperationState::Accepted,
            },
        );
        lease.highest_sequence = key.sequence;
        self.usage.retained_operations += 1;
        self.usage.retained_payload_bytes = retained_payload_bytes;
        Ok(Admission::Execute)
    }

    /// Observe an exact duplicate before checking mutable target preconditions.
    ///
    /// `None` is not an admission: callers must still validate live authority,
    /// target/owner revisions and effect capacity, then call [`Self::admit`].
    /// This never reserves a key or returns [`Admission::Execute`].
    pub fn existing_admission(
        &self,
        key: OperationKey,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Option<Admission>, ProtocolError> {
        self.validate_key(key)?;
        let lease = self
            .leases
            .get(&key.lease)
            .ok_or(ProtocolError::UnknownLease)?;
        if lease.expired || now_secs >= lease.expires_at_secs {
            return Err(ProtocolError::LeaseExpired);
        }
        if payload.len() > MAX_OPERATION_PAYLOAD_BYTES {
            return Err(ProtocolError::PayloadTooLarge {
                max_bytes: MAX_OPERATION_PAYLOAD_BYTES,
                actual_bytes: payload.len(),
            });
        }
        if key.sequence <= lease.retired_through {
            return Err(ProtocolError::ResultExpired);
        }
        if let Some(record) = lease.operations.get(&key.sequence) {
            if record.payload.as_slice() != payload {
                return Err(ProtocolError::OperationConflict);
            }
            return Ok(Some(admission_for(&record.state)));
        }
        if key.sequence <= lease.highest_sequence {
            return Err(ProtocolError::SequenceNotIncreasing {
                highest: lease.highest_sequence,
                received: key.sequence,
            });
        }
        Ok(None)
    }

    /// Record a terminal outcome for a previously admitted operation.
    ///
    /// Completion remains allowed after the lease expires because accepted
    /// work is not canceled by expiry. Completion never reopens the lease.
    pub fn complete(&mut self, key: OperationKey, outcome: Outcome) -> Result<(), ProtocolError> {
        self.validate_key(key)?;
        if outcome == Outcome::Accepted {
            return Err(ProtocolError::NotTerminalOutcome);
        }
        let lease = self
            .leases
            .get_mut(&key.lease)
            .ok_or(ProtocolError::UnknownLease)?;
        if let Some(record) = lease.operations.get_mut(&key.sequence) {
            return match record.state {
                OperationState::Accepted => {
                    record.state = OperationState::Completed(outcome);
                    Ok(())
                }
                OperationState::Completed(previous) if previous == outcome => Ok(()),
                OperationState::Completed(_) => Err(ProtocolError::CompletionConflict),
            };
        }
        if key.sequence <= lease.retired_through
            || (lease.expired && key.sequence <= lease.highest_sequence)
        {
            return Err(ProtocolError::ResultExpired);
        }
        Err(ProtocolError::UnknownOperation)
    }

    /// Read an operation's recorded admission state without changing the ledger.
    ///
    /// Expired leases always return [`ProtocolError::LeaseExpired`], even when
    /// a record exists. For an unknown sequence above the high-water mark,
    /// [`OperationStatus::NotAdmitted`] means it has not been admitted in this
    /// live ledger. This observation never reserves a key or authorizes
    /// execution; dispatch requires [`Admission::Execute`] from [`Self::admit`].
    /// An unknown sequence below the high-water mark is conservatively rejected.
    pub fn status(
        &self,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<OperationStatus, ProtocolError> {
        self.validate_key(key)?;
        let lease = self
            .leases
            .get(&key.lease)
            .ok_or(ProtocolError::UnknownLease)?;
        if lease.expired || now_secs >= lease.expires_at_secs {
            return Err(ProtocolError::LeaseExpired);
        }
        if key.sequence <= lease.retired_through {
            return Err(ProtocolError::ResultExpired);
        }
        if let Some(record) = lease.operations.get(&key.sequence) {
            return Ok(status_for(&record.state));
        }
        if key.sequence <= lease.highest_sequence {
            return Err(ProtocolError::UnknownOldSequence);
        }
        Ok(OperationStatus::NotAdmitted)
    }

    /// Retire completed entries cumulatively through `through_sequence`.
    ///
    /// A receipt is rejected when it includes a pending operation or advances
    /// beyond the highest admitted sequence. Repeated or older receipts are
    /// idempotent. Retired sequence keys can never be admitted again.
    pub fn receipt(
        &mut self,
        lease_id: u64,
        through_sequence: u64,
        now_secs: u64,
    ) -> Result<(), ProtocolError> {
        if lease_id == 0 {
            return Err(ProtocolError::ZeroLease);
        }
        if through_sequence == 0 {
            return Err(ProtocolError::ZeroSequence);
        }
        let lease = self
            .leases
            .get_mut(&lease_id)
            .ok_or(ProtocolError::UnknownLease)?;
        if lease.expired || now_secs >= lease.expires_at_secs {
            return Err(ProtocolError::LeaseExpired);
        }
        if through_sequence <= lease.retired_through {
            return Ok(());
        }
        if through_sequence > lease.highest_sequence {
            return Err(ProtocolError::ReceiptBeyondHighest {
                highest: lease.highest_sequence,
                received: through_sequence,
            });
        }
        if let Some((&sequence, _)) = lease
            .operations
            .range(..=through_sequence)
            .find(|(_, record)| matches!(record.state, OperationState::Accepted))
        {
            return Err(ProtocolError::ReceiptIncludesPending { sequence });
        }

        let retired: Vec<(u64, usize)> = lease
            .operations
            .range(..=through_sequence)
            .map(|(&sequence, record)| (sequence, record.payload.len()))
            .collect();
        let retired_payload_bytes = retired.iter().map(|(_, bytes)| bytes).sum::<usize>();
        for (sequence, _) in &retired {
            lease.operations.remove(sequence);
        }
        lease.retired_through = through_sequence;
        self.usage.retained_operations -= retired.len();
        self.usage.retained_payload_bytes -= retired_payload_bytes;
        Ok(())
    }

    fn validate_key(&self, key: OperationKey) -> Result<(), ProtocolError> {
        if key.generation == 0 {
            return Err(ProtocolError::ZeroGeneration);
        }
        if key.lease == 0 {
            return Err(ProtocolError::ZeroLease);
        }
        if key.sequence == 0 {
            return Err(ProtocolError::ZeroSequence);
        }
        if key.generation != self.generation {
            return Err(ProtocolError::WrongGeneration {
                expected: self.generation,
                received: key.generation,
            });
        }
        Ok(())
    }
}

fn admission_for(state: &OperationState) -> Admission {
    match state {
        OperationState::Accepted => Admission::InProgress,
        OperationState::Completed(outcome) => Admission::Completed(*outcome),
    }
}

fn status_for(state: &OperationState) -> OperationStatus {
    match state {
        OperationState::Accepted => OperationStatus::InProgress,
        OperationState::Completed(outcome) => OperationStatus::Completed(*outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_ledger() -> Ledger {
        let mut ledger = Ledger::new(7).expect("nonzero generation");
        ledger.open_lease(11, 100).expect("valid lease");
        ledger
    }

    fn key(sequence: u64) -> OperationKey {
        key_for(11, sequence)
    }

    fn key_for(lease: u64, sequence: u64) -> OperationKey {
        OperationKey {
            generation: 7,
            lease,
            sequence,
        }
    }

    fn independently_count_usage(ledger: &Ledger) -> LedgerUsage {
        let mut usage = LedgerUsage {
            lease_records: ledger.leases.len(),
            ..LedgerUsage::default()
        };
        for lease in ledger.leases.values() {
            if !lease.expired {
                usage.active_leases += 1;
            }
            usage.retained_operations += lease.operations.len();
            for record in lease.operations.values() {
                usage.retained_payload_bytes += record.payload.len();
            }
        }
        usage
    }

    fn assert_usage_matches_records(ledger: &Ledger) {
        assert_eq!(ledger.usage(), independently_count_usage(ledger));
    }

    #[test]
    fn duplicates_share_record_and_changed_payload_conflicts() {
        let mut ledger = open_ledger();
        assert_eq!(ledger.admit(key(1), b"launch", 101), Ok(Admission::Execute));
        assert_eq!(
            ledger.admit(key(1), b"launch", 102),
            Ok(Admission::InProgress)
        );
        assert_eq!(
            ledger.admit(key(1), b"different", 103),
            Err(ProtocolError::OperationConflict)
        );
        ledger.complete(key(1), Outcome::Applied).unwrap();
        assert_eq!(
            ledger.admit(key(1), b"launch", 104),
            Ok(Admission::Completed(Outcome::Applied))
        );
    }

    #[test]
    fn payload_and_unreceipted_limits_reject_without_mutation() {
        let mut ledger = open_ledger();
        let oversized = vec![0; MAX_OPERATION_PAYLOAD_BYTES + 1];
        assert!(matches!(
            ledger.admit(key(1), &oversized, 101),
            Err(ProtocolError::PayloadTooLarge { .. })
        ));
        assert_eq!(ledger.admit(key(1), b"ok", 101), Ok(Admission::Execute));

        for sequence in 2..=MAX_UNRECEIPTED_OPERATIONS as u64 {
            assert_eq!(
                ledger.admit(key(sequence), b"x", 101),
                Ok(Admission::Execute)
            );
        }
        assert_eq!(
            ledger.admit(key(MAX_UNRECEIPTED_OPERATIONS as u64 + 1), b"x", 101),
            Err(ProtocolError::CapacityExceeded)
        );
        assert_eq!(
            ledger.status(key(MAX_UNRECEIPTED_OPERATIONS as u64 + 1), 101),
            Ok(OperationStatus::NotAdmitted)
        );
    }

    #[test]
    fn receipts_wait_for_completion_then_retire_keys() {
        let mut ledger = open_ledger();
        ledger.admit(key(1), b"a", 101).unwrap();
        ledger.admit(key(3), b"c", 101).unwrap();
        assert_eq!(
            ledger.receipt(11, 3, 102),
            Err(ProtocolError::ReceiptIncludesPending { sequence: 1 })
        );
        assert_eq!(ledger.status(key(1), 102), Ok(OperationStatus::InProgress));
        assert_eq!(ledger.status(key(3), 102), Ok(OperationStatus::InProgress));
        ledger.complete(key(1), Outcome::Applied).unwrap();
        assert_eq!(
            ledger.receipt(11, 3, 102),
            Err(ProtocolError::ReceiptIncludesPending { sequence: 3 })
        );
        assert_eq!(
            ledger.status(key(1), 102),
            Ok(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(ledger.status(key(3), 102), Ok(OperationStatus::InProgress));
        ledger.complete(key(3), Outcome::Partial).unwrap();
        ledger.receipt(11, 3, 103).unwrap();
        ledger.receipt(11, 2, 103).unwrap();
        assert_eq!(
            ledger.status(key(1), 104),
            Err(ProtocolError::ResultExpired)
        );
        assert_eq!(
            ledger.admit(key(3), b"c", 104),
            Err(ProtocolError::ResultExpired)
        );
        assert_eq!(ledger.admit(key(4), b"d", 104), Ok(Admission::Execute));
    }

    #[test]
    fn expiry_rejects_duplicates_but_does_not_cancel_completion() {
        let mut ledger = open_ledger();
        ledger.admit(key(1), b"work", 101).unwrap();
        let expiry = 100 + LEASE_TTL_SECS;
        assert_eq!(
            ledger.admit(key(1), b"work", expiry),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(
            ledger.status(key(1), expiry),
            Err(ProtocolError::LeaseExpired)
        );
        ledger.complete(key(1), Outcome::Applied).unwrap();
        assert_eq!(
            ledger.status(key(1), expiry + 1),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(
            ledger.open_lease(11, expiry + 1),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(
            ledger.renew_lease(11, expiry),
            Err(ProtocolError::LeaseExpired)
        );
    }

    #[test]
    fn rejects_wrong_generation_and_out_of_order_new_sequences() {
        let mut ledger = open_ledger();
        let mut wrong = key(1);
        wrong.generation = 8;
        assert_eq!(
            ledger.admit(wrong, b"x", 101),
            Err(ProtocolError::WrongGeneration {
                expected: 7,
                received: 8
            })
        );
        ledger.admit(key(2), b"second", 101).unwrap();
        assert_eq!(ledger.status(key(3), 101), Ok(OperationStatus::NotAdmitted));
        assert_eq!(
            ledger.admit(key(1), b"first", 101),
            Err(ProtocolError::SequenceNotIncreasing {
                highest: 2,
                received: 1
            })
        );
        assert_eq!(
            ledger.status(key(1), 101),
            Err(ProtocolError::UnknownOldSequence)
        );
        assert_eq!(ledger.admit(key(3), b"third", 101), Ok(Admission::Execute));
    }

    #[test]
    fn retry_classes_match_side_effect_boundaries() {
        assert_eq!(
            OperationClass::Query.retry_disposition(),
            RetryDisposition::Reissue
        );
        assert_eq!(
            OperationClass::DurableMutation.retry_disposition(),
            RetryDisposition::SameKeySamePayload
        );
        for class in [
            OperationClass::Input,
            OperationClass::Signal,
            OperationClass::Clipboard,
        ] {
            assert_eq!(class.retry_disposition(), RetryDisposition::Never);
        }
        assert_eq!(
            OperationClass::AbsoluteResize.retry_disposition(),
            RetryDisposition::Reconcile
        );
    }

    #[test]
    fn ledger_debug_does_not_reveal_payload_bytes() {
        let mut ledger = open_ledger();
        ledger
            .admit(key(1), b"sensitive-input-marker", 101)
            .unwrap();
        let debug = format!("{ledger:?}");
        assert!(!debug.contains("sensitive-input-marker"));
        assert!(debug.contains("lease_count"));
    }

    #[test]
    fn global_operation_limit_charges_empty_payloads_and_receipts_free_slots() {
        let mut ledger = Ledger::new(7).unwrap();
        for lease in 1..=5 {
            ledger.open_lease(lease, 100).unwrap();
        }
        for lease in 1..=4 {
            for sequence in 1..=MAX_UNRECEIPTED_OPERATIONS as u64 {
                assert_eq!(
                    ledger.admit(key_for(lease, sequence), b"", 101),
                    Ok(Admission::Execute)
                );
            }
        }
        assert_eq!(
            ledger.usage(),
            LedgerUsage {
                lease_records: 5,
                active_leases: 5,
                retained_operations: MAX_RETAINED_OPERATIONS,
                retained_payload_bytes: 0,
            }
        );
        assert_usage_matches_records(&ledger);

        let before = ledger.usage();
        assert_eq!(
            ledger.admit(key_for(1, 1), b"", 102),
            Ok(Admission::InProgress)
        );
        assert_eq!(
            ledger.admit(key_for(1, 1), b"changed", 102),
            Err(ProtocolError::OperationConflict)
        );
        assert_eq!(
            ledger.admit(key_for(5, 1), b"", 102),
            Err(ProtocolError::GlobalOperationCapacityExceeded)
        );
        assert_eq!(
            ledger.status(key_for(5, 1), 102),
            Ok(OperationStatus::NotAdmitted)
        );
        assert_eq!(ledger.usage(), before);

        for sequence in 1..=MAX_UNRECEIPTED_OPERATIONS as u64 {
            ledger
                .complete(key_for(1, sequence), Outcome::Applied)
                .unwrap();
        }
        ledger
            .receipt(1, MAX_UNRECEIPTED_OPERATIONS as u64, 103)
            .unwrap();
        assert_eq!(ledger.usage().retained_operations, 3_072);
        assert_eq!(
            ledger.admit(key_for(5, 1), b"", 104),
            Ok(Admission::Execute)
        );
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn global_payload_limit_precedes_copy_and_reuses_capacity_after_receipt() {
        let mut ledger = Ledger::new(7).unwrap();
        for lease in 1..=3 {
            ledger.open_lease(lease, 100).unwrap();
        }
        let payload = vec![b'p'; MAX_OPERATION_PAYLOAD_BYTES];
        for lease in 1..=2 {
            for sequence in 1..=64 {
                assert_eq!(
                    ledger.admit(key_for(lease, sequence), &payload, 101),
                    Ok(Admission::Execute)
                );
            }
        }
        assert_eq!(ledger.usage().retained_operations, 128);
        assert_eq!(
            ledger.usage().retained_payload_bytes,
            MAX_RETAINED_PAYLOAD_BYTES
        );
        assert_usage_matches_records(&ledger);

        let before = ledger.usage();
        assert_eq!(
            ledger.admit(key_for(3, 1), b"small", 102),
            Err(ProtocolError::PayloadCapacityExceeded)
        );
        assert_eq!(
            ledger.status(key_for(3, 1), 102),
            Ok(OperationStatus::NotAdmitted)
        );
        assert_eq!(ledger.usage(), before);
        assert_eq!(
            ledger.admit(key_for(2, 64), &payload, 102),
            Ok(Admission::InProgress)
        );
        assert_eq!(
            ledger.admit(key_for(2, 64), b"different", 102),
            Err(ProtocolError::OperationConflict)
        );

        for sequence in 1..=64 {
            ledger
                .complete(key_for(2, sequence), Outcome::Applied)
                .unwrap();
        }
        ledger.receipt(2, 64, 103).unwrap();
        assert_eq!(ledger.usage().retained_payload_bytes, 4 * 1024 * 1024);
        assert_eq!(
            ledger.admit(key_for(3, 1), b"small", 104),
            Ok(Admission::Execute)
        );
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn active_lease_limit_does_not_consume_issued_ids_on_failure() {
        let mut ledger = Ledger::new(7).unwrap();
        for id in 1..=MAX_ACTIVE_LEASES as u64 {
            ledger.open_lease(id, 100).unwrap();
        }
        let before = ledger.usage();
        assert_eq!(
            ledger.open_lease(MAX_ACTIVE_LEASES as u64 + 1, 101),
            Err(ProtocolError::ActiveLeaseCapacityExceeded)
        );
        assert_eq!(
            ledger.issue_lease(101),
            Err(ProtocolError::ActiveLeaseCapacityExceeded)
        );
        assert_eq!(ledger.issued_through, MAX_ACTIVE_LEASES as u64);
        assert_eq!(ledger.usage(), before);

        let expiry = 100 + LEASE_TTL_SECS;
        assert_eq!(ledger.expire_leases(expiry), MAX_ACTIVE_LEASES);
        assert_eq!(ledger.issue_lease(expiry), Ok(MAX_ACTIVE_LEASES as u64 + 1));
        assert_eq!(ledger.usage().active_leases, 1);
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn lease_issuance_follows_explicit_ids_and_handles_ttl_and_id_exhaustion() {
        let mut ledger = Ledger::new(7).unwrap();
        ledger.open_lease(20, 100).unwrap();
        assert_eq!(ledger.issue_lease(101), Ok(21));
        ledger.open_lease(40, 102).unwrap();
        assert_eq!(ledger.issue_lease(103), Ok(41));
        ledger.open_lease(30, 104).unwrap();
        assert_eq!(ledger.issue_lease(105), Ok(42));
        assert_eq!(ledger.issued_through, 42);
        assert_usage_matches_records(&ledger);

        let mut overflow = Ledger::new(7).unwrap();
        let before = overflow.usage();
        assert_eq!(
            overflow.issue_lease(u64::MAX),
            Err(ProtocolError::TimeOverflow)
        );
        assert_eq!(overflow.issued_through, 0);
        assert_eq!(overflow.usage(), before);
        assert_eq!(overflow.issue_lease(0), Ok(1));

        let mut exhausted = Ledger::new(7).unwrap();
        exhausted.open_lease(u64::MAX, 0).unwrap();
        let before = exhausted.usage();
        assert_eq!(
            exhausted.issue_lease(1),
            Err(ProtocolError::LeaseIdExhausted)
        );
        assert_eq!(exhausted.issued_through, u64::MAX);
        assert_eq!(exhausted.usage(), before);
    }

    #[test]
    fn permanent_lease_record_limit_includes_expired_records() {
        let mut ledger = Ledger::new(7).unwrap();
        let mut now = 0;
        for start in (0..MAX_LEASES_PER_GENERATION).step_by(MAX_ACTIVE_LEASES) {
            for _ in start..(start + MAX_ACTIVE_LEASES) {
                ledger.issue_lease(now).unwrap();
            }
            assert_eq!(
                ledger.expire_leases(now + LEASE_TTL_SECS),
                MAX_ACTIVE_LEASES
            );
            now += LEASE_TTL_SECS;
        }

        assert_eq!(
            ledger.usage(),
            LedgerUsage {
                lease_records: MAX_LEASES_PER_GENERATION,
                active_leases: 0,
                retained_operations: 0,
                retained_payload_bytes: 0,
            }
        );
        let before = ledger.usage();
        assert_eq!(
            ledger.issue_lease(now),
            Err(ProtocolError::LeaseCapacityExceeded)
        );
        assert_eq!(
            ledger.open_lease(u64::MAX, now),
            Err(ProtocolError::LeaseCapacityExceeded)
        );
        assert_eq!(ledger.issued_through, MAX_LEASES_PER_GENERATION as u64);
        assert_eq!(ledger.usage(), before);
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn expiry_reclaims_payloads_and_terminal_records_but_preserves_pending_identity() {
        let mut ledger = open_ledger();
        ledger
            .admit(key(1), b"pending-sensitive-bytes", 101)
            .unwrap();
        ledger
            .admit(key(2), b"completed-sensitive-bytes", 101)
            .unwrap();
        ledger.complete(key(2), Outcome::Applied).unwrap();
        let expiry = 100 + LEASE_TTL_SECS;

        assert_eq!(ledger.expire_leases(expiry), 1);
        assert_eq!(
            ledger.usage(),
            LedgerUsage {
                lease_records: 1,
                active_leases: 0,
                retained_operations: 1,
                retained_payload_bytes: 0,
            }
        );
        assert_eq!(ledger.leases[&11].operations.len(), 1);
        assert_eq!(ledger.leases[&11].operations[&1].payload.capacity(), 0);
        assert!(!ledger.leases[&11].operations.contains_key(&2));
        assert_usage_matches_records(&ledger);

        assert_eq!(
            ledger.admit(key(1), b"pending-sensitive-bytes", 101),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(
            ledger.admit(key(3), b"never-admitted", 101),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(ledger.status(key(1), 101), Err(ProtocolError::LeaseExpired));
        assert_eq!(
            ledger.renew_lease(11, 101),
            Err(ProtocolError::LeaseExpired)
        );
        assert_eq!(ledger.receipt(11, 1, 101), Err(ProtocolError::LeaseExpired));
        assert_eq!(ledger.open_lease(11, 101), Err(ProtocolError::LeaseExpired));
        assert_eq!(
            ledger.complete(key(2), Outcome::Applied),
            Err(ProtocolError::ResultExpired)
        );

        ledger.complete(key(1), Outcome::Applied).unwrap();
        ledger.complete(key(1), Outcome::Applied).unwrap();
        assert_eq!(
            ledger.complete(key(1), Outcome::Failed),
            Err(ProtocolError::CompletionConflict)
        );
        assert_eq!(ledger.usage().retained_payload_bytes, 0);
        assert_eq!(ledger.expire_leases(expiry - 100), 0);
        assert_eq!(ledger.usage().retained_operations, 0);
        assert_eq!(
            ledger.complete(key(1), Outcome::Applied),
            Err(ProtocolError::ResultExpired)
        );
        assert_eq!(ledger.status(key(1), 0), Err(ProtocolError::LeaseExpired));
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn expired_pending_metadata_keeps_global_slots_until_completion_is_reclaimed() {
        let mut ledger = Ledger::new(7).unwrap();
        ledger.open_lease(1, 0).unwrap();
        for lease in 2..=5 {
            ledger.open_lease(lease, 100).unwrap();
        }
        for lease in 1..=4 {
            for sequence in 1..=MAX_UNRECEIPTED_OPERATIONS as u64 {
                ledger.admit(key_for(lease, sequence), b"x", 101).unwrap();
            }
        }
        assert_eq!(ledger.usage().retained_operations, MAX_RETAINED_OPERATIONS);
        assert_eq!(ledger.expire_leases(LEASE_TTL_SECS), 1);
        assert_eq!(ledger.usage().retained_operations, MAX_RETAINED_OPERATIONS);
        assert_eq!(ledger.usage().retained_payload_bytes, 3_072);
        assert_eq!(ledger.leases[&1].operations[&1].payload.capacity(), 0);
        assert_usage_matches_records(&ledger);

        assert_eq!(
            ledger.admit(key_for(5, 1), b"new", LEASE_TTL_SECS + 1),
            Err(ProtocolError::GlobalOperationCapacityExceeded)
        );
        for sequence in 1..=MAX_UNRECEIPTED_OPERATIONS as u64 {
            ledger
                .complete(key_for(1, sequence), Outcome::Applied)
                .unwrap();
        }
        assert_eq!(ledger.usage().retained_operations, MAX_RETAINED_OPERATIONS);
        assert_eq!(ledger.expire_leases(LEASE_TTL_SECS - 1), 0);
        assert_eq!(ledger.usage().retained_operations, 3_072);
        assert_eq!(
            ledger.admit(key_for(5, 1), b"new", LEASE_TTL_SECS + 2),
            Ok(Admission::Execute)
        );
        assert_usage_matches_records(&ledger);
    }

    #[test]
    fn stored_usage_matches_independent_traversal_after_mixed_transitions() {
        let mut ledger = Ledger::new(7).unwrap();
        ledger.open_lease(10, 100).unwrap();
        assert_usage_matches_records(&ledger);
        ledger.open_lease(5, 500).unwrap();
        assert_eq!(ledger.issue_lease(501), Ok(11));
        assert_usage_matches_records(&ledger);

        ledger.admit(key_for(10, 1), b"one", 101).unwrap();
        assert_usage_matches_records(&ledger);
        assert_eq!(
            ledger.admit(key_for(10, 1), b"one", 102),
            Ok(Admission::InProgress)
        );
        assert_usage_matches_records(&ledger);
        assert_eq!(
            ledger.admit(key_for(10, 1), b"other", 102),
            Err(ProtocolError::OperationConflict)
        );
        assert_usage_matches_records(&ledger);
        ledger.complete(key_for(10, 1), Outcome::Applied).unwrap();
        assert_usage_matches_records(&ledger);
        ledger.receipt(10, 1, 103).unwrap();
        assert_usage_matches_records(&ledger);

        ledger.admit(key_for(5, 1), b"two", 501).unwrap();
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.usage().active_leases, 3);
        assert_eq!(ledger.expire_leases(100 + LEASE_TTL_SECS), 1);
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.usage().active_leases, 2);

        let second_expiry = 500 + LEASE_TTL_SECS;
        assert_eq!(ledger.usage().active_leases, 2);
        assert_eq!(ledger.expire_leases(second_expiry), 1);
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.usage().active_leases, 1);
        ledger.complete(key_for(5, 1), Outcome::Unknown).unwrap();
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.expire_leases(second_expiry - 1), 0);
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.usage().retained_operations, 0);
        assert_eq!(ledger.expire_leases(second_expiry + 1), 1);
        assert_usage_matches_records(&ledger);
        assert_eq!(ledger.usage().active_leases, 0);
    }

    #[test]
    fn new_capacity_errors_have_fixed_redacted_labels() {
        let errors = [
            (
                ProtocolError::LeaseCapacityExceeded,
                "generation has reached its lease record limit",
            ),
            (
                ProtocolError::ActiveLeaseCapacityExceeded,
                "generation has reached its active lease limit",
            ),
            (
                ProtocolError::GlobalOperationCapacityExceeded,
                "generation has reached its retained operation limit",
            ),
            (
                ProtocolError::PayloadCapacityExceeded,
                "generation has reached its retained payload limit",
            ),
            (
                ProtocolError::LeaseIdExhausted,
                "lease ID space is exhausted",
            ),
        ];
        for (error, label) in errors {
            assert_eq!(error.to_string(), label);
        }
    }

    #[test]
    fn zero_ids_and_nonterminal_completion_are_rejected() {
        assert_eq!(Ledger::new(0).err(), Some(ProtocolError::ZeroGeneration));
        let mut ledger = Ledger::new(7).unwrap();
        assert_eq!(ledger.open_lease(0, 100), Err(ProtocolError::ZeroLease));
        ledger.open_lease(11, 100).unwrap();
        assert_eq!(
            ledger.admit(key(0), b"x", 101),
            Err(ProtocolError::ZeroSequence)
        );
        ledger.admit(key(1), b"x", 101).unwrap();
        assert_eq!(
            ledger.complete(key(1), Outcome::Accepted),
            Err(ProtocolError::NotTerminalOutcome)
        );
    }

    #[test]
    fn duplicate_observation_never_reserves_or_authorizes_new_effects() {
        let mut ledger = Ledger::new(7).unwrap();
        ledger.open_lease(11, 100).unwrap();
        let before = ledger.usage();
        assert_eq!(ledger.existing_admission(key(1), b"input", 101), Ok(None));
        assert_eq!(ledger.usage(), before);
        assert_eq!(ledger.status(key(1), 101), Ok(OperationStatus::NotAdmitted));
        ledger.admit(key(1), b"input", 101).unwrap();
        assert_eq!(
            ledger.existing_admission(key(1), b"input", 101),
            Ok(Some(Admission::InProgress))
        );
        assert_eq!(
            ledger.existing_admission(key(1), b"other", 101),
            Err(ProtocolError::OperationConflict)
        );
        ledger.complete(key(1), Outcome::Applied).unwrap();
        assert_eq!(
            ledger.existing_admission(key(1), b"input", 101),
            Ok(Some(Admission::Completed(Outcome::Applied)))
        );
        ledger.receipt(11, 1, 101).unwrap();
        assert_eq!(
            ledger.existing_admission(key(1), b"input", 101),
            Err(ProtocolError::ResultExpired)
        );
    }
}
