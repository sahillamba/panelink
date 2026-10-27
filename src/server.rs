//! Synchronous, single-target admission owner for the local prototype server.
//!
//! The owner coordinates bounded in-memory leases, the operation ledger, input
//! ownership, typed requests, and one effect sink. Peer authentication,
//! transport lifetime, persistent generation storage, PTY scheduling, and
//! durable recovery belong to the adapters around this module.

use crate::control_protocol::{
    decode_request_payload, Command, ControlError, ControlRequest, ReadSource, TargetRef,
};
use crate::event_journal::{EventCursor, EventJournal, JournalError};
use crate::event_protocol::{Event, EventBody};
use crate::handshake_protocol::{
    negotiate, HandshakeError, Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ,
};
use crate::input_owner::{ExpireResult, InputOwner, Owner, OwnerError, OwnerSnapshot};
use crate::protocol::{
    Admission, Ledger, OperationKey, OperationStatus, Outcome, ProtocolError, LEASE_TTL_SECS,
    MAX_ACTIVE_LEASES,
};
use crate::response_protocol::{
    encode_response_payload, RejectionCode, Response, ResponseBody, ResponseError,
};
use crate::session_protocol::{
    decode_session_request, SessionHello, SessionInfo, SessionProtocolError, SessionRequest,
};
use crate::watch_protocol::{
    encode_reconciliation, Reconciliation, MAX_RECONCILIATION_BYTES, MAX_RECONCILIATION_LINES,
    MAX_WATCH_EVENTS, RECONCILIATION_PREFIX_BYTES,
};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod multi_session;

/// Read result returned by one bounded effect-sink observation.
#[derive(Clone, Eq, PartialEq)]
pub struct ReadResult {
    /// Sanitized UTF-8 display text. Debug output never includes this field.
    pub text: String,
    /// Nonzero revision of the screen state used for this result.
    pub revision: u64,
    /// Whether the effect sink omitted additional content to fit its limit.
    pub truncated: bool,
}

impl fmt::Debug for ReadResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadResult")
            .field("text_len", &self.text.len())
            .field("revision", &"<redacted>")
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// One synchronous, preflighted effect boundary for a single target.
///
/// `validate` must be pure and bounded. Input must finish synchronously and
/// return only after any unwritten suffix has been discarded or accounted for;
/// it must never report `Outcome::Accepted` or replay a prior input. Read data
/// must be sanitized display text rather than raw terminal control sequences.
pub trait EffectSink {
    /// Check support and bounded capacity without performing an effect.
    fn validate(&self, command: Command<'_>) -> Result<(), RejectionCode>;

    /// Read one bounded screen or live-scrollback observation.
    fn read(
        &mut self,
        source: ReadSource,
        max_text_bytes: usize,
        max_lines: u16,
    ) -> Result<ReadResult, Outcome>;

    /// Apply one synchronous input or signal and report its terminal outcome.
    fn input(&mut self, command: Command<'_>) -> Outcome;

    /// Make bounded sink progress without creating a queue or background task.
    fn poll(&mut self) -> Result<(), Outcome>;

    /// Report whether the managed child has exited.
    fn child_exited(&self) -> bool;

    /// Return the opaque identity captured for this direct managed child.
    /// Sinks without process metadata remain compatible and report none.
    fn process_identity(&self) -> Option<crate::process_metadata::ProcessIdentity> {
        None
    }

    /// Return the current nonzero screen revision when metadata tracking works.
    /// Existing sinks default to no event capability.
    fn screen_revision(&self) -> Option<u64> {
        None
    }

    /// Explicitly stop the managed child within bounded windows.
    ///
    /// Implementations send HUP to the child process group, wait a bounded
    /// window, escalate to KILL, wait another bounded window, and report what
    /// was observed. This never sends caller input and never respawns. The
    /// default for sinks without process control reports `Exited` only when
    /// the child has already exited, and otherwise `Unknown`.
    fn stop(&mut self) -> crate::close_protocol::StopOutcome {
        if self.child_exited() {
            crate::close_protocol::StopOutcome::Exited
        } else {
            crate::close_protocol::StopOutcome::Unknown
        }
    }

    /// Begin a live-runtime stop without waiting. Implementations used by the
    /// multi-session server override this with a HUP-only transition. The
    /// default never calls the legacy synchronous `stop` hook.
    fn begin_stop(
        &mut self,
    ) -> Result<Option<crate::close_protocol::StopOutcome>, crate::close_protocol::StopOutcome>
    {
        if self.child_exited() {
            Ok(Some(crate::close_protocol::StopOutcome::Exited))
        } else {
            Err(crate::close_protocol::StopOutcome::Unknown)
        }
    }

    /// Advance a live-runtime stop using a nonblocking child-status poll.
    /// `kill` requests the one-time escalation after the shared HUP deadline.
    /// `None` means the stop transaction is still in progress.
    fn advance_stop(
        &mut self,
        _kill: bool,
    ) -> Result<Option<crate::close_protocol::StopOutcome>, crate::close_protocol::StopOutcome>
    {
        Ok(Some(if self.child_exited() {
            crate::close_protocol::StopOutcome::Exited
        } else {
            crate::close_protocol::StopOutcome::Unknown
        }))
    }

    /// Apply one absolute PTY size, used when a layout change happens without
    /// a caller-supplied resize port (for example automatic pane collapse).
    ///
    /// The default reports `Failed` (unsupported); the caller then records the
    /// size as resize-pending rather than assuming it was applied.
    fn apply_size(&mut self, _rows: u16, _columns: u16) -> Result<(), Outcome> {
        Err(Outcome::Failed)
    }
}

/// Opaque connection capability created only by [`StateOwner::connect`].
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Connection {
    generation: u64,
    lease: u64,
    selected_capabilities: u64,
}

impl Connection {
    /// Return the server generation that issued this connection.
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Return the reusable client lease associated with this connection.
    pub const fn lease(self) -> u64 {
        self.lease
    }

    /// Return the capabilities selected for this attachment.
    pub const fn selected_capabilities(self) -> u64 {
        self.selected_capabilities
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("generation", &"<redacted>")
            .field("lease", &"<redacted>")
            .field("selected_capabilities", &self.selected_capabilities)
            .finish()
    }
}

/// Result of handling a session-level request.
pub enum SessionReply {
    /// Read-only current target, client-owner, and child-exit information.
    Info(SessionInfo),
    /// Encoded schema-1 operation status or ownership result.
    Payload(Vec<u8>),
    /// Ask the transport to acknowledge and then close this attachment.
    Detached,
}

impl fmt::Debug for SessionReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info(info) => f.debug_tuple("Info").field(info).finish(),
            Self::Payload(payload) => f
                .debug_struct("Payload")
                .field("bytes_len", &payload.len())
                .finish(),
            Self::Detached => f.write_str("Detached"),
        }
    }
}

/// One authoritative screen and metadata observation bound to its event cursor.
#[derive(Eq, PartialEq)]
pub struct ReconcileState {
    /// Current target, input-owner and child-exit metadata.
    pub info: SessionInfo,
    /// Latest event fact recorded in the same owner turn as this observation.
    pub cursor: EventCursor,
    /// Screen revision represented by `text`.
    pub screen_revision: u64,
    /// Whether the bounded screen read omitted additional content.
    pub truncated: bool,
    /// Sanitized UTF-8 screen text. Debug output never includes this field.
    pub text: String,
}

impl fmt::Debug for ReconcileState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReconcileState")
            .field("info", &self.info)
            .field("cursor", &self.cursor)
            .field("screen_revision", &"<redacted>")
            .field("truncated", &self.truncated)
            .field("text_len", &self.text.len())
            .finish()
    }
}

/// One bounded page of metadata events for the owner's fixed target.
#[derive(Eq, PartialEq)]
pub struct EventPage {
    /// Ordered events after the requested cursor, when available.
    pub events: Vec<Event>,
    /// Last returned event cursor, or the current journal cursor when empty.
    pub cursor: EventCursor,
    /// The requested cursor expired and a fresh reconciliation is required.
    pub resync_required: bool,
}

impl fmt::Debug for EventPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventPage")
            .field("event_count", &self.events.len())
            .field("cursor", &self.cursor)
            .field("resync_required", &self.resync_required)
            .finish()
    }
}

/// Fixed/redacted errors from the pure one-session owner.
#[derive(Clone, Eq, PartialEq)]
pub enum ServerError {
    /// The configured generation or target identity is invalid.
    InvalidConfiguration,
    /// The connection or requested lease is not live in this owner.
    UnknownContext,
    /// The connection does not hold the needed capability or lease identity.
    Unauthorized,
    /// A represented owner kind is unsupported by the schema-2 info payload.
    UnsupportedOwner,
    /// The requested event cursor belongs to another generation.
    WrongGeneration,
    /// The requested event cursor is ahead of the journal.
    CursorAhead,
    /// The requested target is not this owner's current target.
    TargetStale,
    /// Metadata event tracking is unavailable for this owner.
    NotificationsUnavailable,
    /// The bounded reconciliation request limits are invalid.
    InvalidWatchRequest,
    /// The sink returned a snapshot that violates the sanitized read contract.
    InvalidSnapshot,
    /// The sink's screen revision changed during the snapshot read.
    SnapshotChanged,
    /// The bounded screen read could not be completed.
    SnapshotReadFailed,
    /// A synchronous sink poll reported a terminal outcome.
    EffectPollFailed,
    /// A handshake payload or capability request is invalid.
    Handshake(HandshakeError),
    /// A control request payload is invalid.
    Control(ControlError),
    /// A session request payload is invalid.
    Session(SessionProtocolError),
    /// An internal response could not be represented by the response codec.
    Response(ResponseError),
    /// The volatile admission ledger rejected an operation.
    Protocol(ProtocolError),
    /// The input owner rejected an internal ownership transition.
    Owner(OwnerError),
    /// A fallible in-memory invariant could not be completed.
    Internal,
}

impl ServerError {
    /// Return a stable nonempty diagnostic label without user-controlled data.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::UnknownContext => "unknown_context",
            Self::Unauthorized => "unauthorized",
            Self::UnsupportedOwner => "unsupported_owner",
            Self::WrongGeneration => "wrong_generation",
            Self::CursorAhead => "cursor_ahead",
            Self::TargetStale => "target_stale",
            Self::NotificationsUnavailable => "notifications_unavailable",
            Self::InvalidWatchRequest => "watch_request_invalid",
            Self::InvalidSnapshot => "snapshot_invalid",
            Self::SnapshotChanged => "snapshot_changed",
            Self::SnapshotReadFailed => "snapshot_read_failed",
            Self::EffectPollFailed => "effect_poll_failed",
            Self::Handshake(_) => "handshake_error",
            Self::Control(_) => "control_error",
            Self::Session(_) => "session_error",
            Self::Response(_) => "response_error",
            Self::Protocol(ProtocolError::WrongGeneration { .. }) => "wrong_generation",
            Self::Protocol(ProtocolError::LeaseExpired) => "lease_expired",
            Self::Protocol(ProtocolError::UnknownLease) => "unknown_context",
            Self::Protocol(ProtocolError::OperationConflict) => "operation_conflict",
            Self::Protocol(ProtocolError::ResultExpired) => "result_expired",
            Self::Protocol(
                ProtocolError::SequenceNotIncreasing { .. } | ProtocolError::UnknownOldSequence,
            ) => "sequence_not_increasing",
            Self::Protocol(
                ProtocolError::CapacityExceeded
                | ProtocolError::LeaseCapacityExceeded
                | ProtocolError::ActiveLeaseCapacityExceeded
                | ProtocolError::GlobalOperationCapacityExceeded
                | ProtocolError::PayloadCapacityExceeded,
            ) => "capacity_exceeded",
            Self::Protocol(_) => "protocol_error",
            Self::Owner(_) => "owner_error",
            Self::Internal => "internal_error",
        }
    }
}

impl fmt::Debug for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServerError").field(&self.code()).finish()
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl Error for ServerError {}

impl From<HandshakeError> for ServerError {
    fn from(value: HandshakeError) -> Self {
        Self::Handshake(value)
    }
}

impl From<ControlError> for ServerError {
    fn from(value: ControlError) -> Self {
        Self::Control(value)
    }
}

impl From<SessionProtocolError> for ServerError {
    fn from(value: SessionProtocolError) -> Self {
        Self::Session(value)
    }
}

impl From<ResponseError> for ServerError {
    fn from(value: ResponseError) -> Self {
        Self::Response(value)
    }
}

impl From<ProtocolError> for ServerError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

impl From<OwnerError> for ServerError {
    fn from(value: OwnerError) -> Self {
        Self::Owner(value)
    }
}

#[derive(Clone, Copy)]
struct LeaseGrant {
    /// Capabilities actually selected for this reusable lease at creation.
    capabilities: u64,
    /// Immutable full target identity selected when this lease was issued.
    target: TargetRef,
    expires_secs: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct OwnerObservation {
    revision: u64,
    owner_lease: u64,
}

#[derive(Clone, Copy)]
enum OwnerMutation {
    Claim {
        expected_revision: u64,
        ttl_seconds: u32,
    },
    Release {
        expected_revision: u64,
    },
    Handoff {
        expected_revision: u64,
        recipient_lease: u64,
        ttl_seconds: u32,
    },
}

/// Private state grouped for one managed target and its effect sink.
struct TargetState<E: EffectSink> {
    target: TargetRef,
    input_owner: InputOwner,
    effects: E,
    journal: EventJournal,
    notifications_available: bool,
    last_screen_revision: Option<u64>,
    last_owner_observation: Option<OwnerObservation>,
    last_child_exited: bool,
}

/// Pure state for one target, bounded reusable leases, and one effect sink.
///
/// A surrounding runtime must authenticate a peer before calling this owner.
/// This type neither creates server generations nor provides durable recovery.
pub struct StateOwner<E: EffectSink> {
    global: GlobalAuthority,
    session: TargetState<E>,
}

struct GlobalAuthority {
    generation: u64,
    allow_input: bool,
    ledger: Ledger,
    grants: HashMap<u64, LeaseGrant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestPreflight {
    /// Authentication and global idempotency checks passed; target checks remain.
    Continue,
    /// Return a normal schema rejection without reserving the operation.
    Rejected(RejectionCode),
    /// Return an existing global operation status without selecting a target.
    Known(OperationStatus),
}

struct SessionHandler<'a, E: EffectSink> {
    global: &'a mut GlobalAuthority,
    session: &'a mut TargetState<E>,
}

impl<E: EffectSink> fmt::Debug for StateOwner<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateOwner")
            .field("generation", &"<redacted>")
            .field("target", &"<redacted>")
            .field("allow_input", &self.global.allow_input)
            .field("active_leases", &self.global.grants.len())
            .field("owner", &self.session.input_owner.snapshot())
            .field(
                "notifications_available",
                &self.session.notifications_available,
            )
            .finish_non_exhaustive()
    }
}

impl<E: EffectSink> StateOwner<E> {
    /// Create one bounded volatile state owner for a nonzero target identity.
    pub fn new(
        generation: u64,
        target: TargetRef,
        effects: E,
        allow_input: bool,
    ) -> Result<Self, ServerError> {
        if generation == 0 || !target_is_valid(target) || target.generation != generation {
            return Err(ServerError::InvalidConfiguration);
        }
        let ledger = Ledger::new(generation)?;
        Self::with_ledger(ledger, target, effects, allow_input)
    }

    /// Adopt a parent-prepared ledger, including a reserved internal launch key.
    ///
    /// Any pre-existing lease not issued by this owner remains unavailable as
    /// a client connection unless it is explicitly represented in this owner's
    /// grant table. The ledger's permanent ID and sequence history are retained.
    pub fn with_ledger(
        ledger: Ledger,
        target: TargetRef,
        effects: E,
        allow_input: bool,
    ) -> Result<Self, ServerError> {
        let generation = ledger.generation();
        if generation == 0 || !target_is_valid(target) || target.generation != generation {
            return Err(ServerError::InvalidConfiguration);
        }
        let input_owner = InputOwner::new();
        let last_owner_observation = owner_observation(input_owner.snapshot());
        let initial_screen_revision = effects.screen_revision().filter(|revision| *revision != 0);
        let last_child_exited = effects.child_exited();
        let journal = EventJournal::new(generation).map_err(|_| ServerError::Internal)?;
        let grants = HashMap::with_capacity(MAX_ACTIVE_LEASES);
        let session = TargetState {
            target,
            input_owner,
            effects,
            journal,
            notifications_available: initial_screen_revision.is_some()
                && last_owner_observation.is_some(),
            last_screen_revision: initial_screen_revision,
            last_owner_observation,
            last_child_exited,
        };
        let global = GlobalAuthority {
            generation,
            allow_input,
            ledger,
            grants,
        };

        Ok(Self { global, session })
    }

    /// Select capabilities, then issue a lease or resume the exact live lease.
    pub fn connect(
        &mut self,
        hello: &SessionHello,
        now_secs: u64,
    ) -> Result<(Connection, Welcome), ServerError> {
        self.global.connect(&self.session, hello, now_secs)
    }

    /// Return this owner's immutable target identity.
    pub const fn target(&self) -> TargetRef {
        self.session.target
    }

    /// Return current input-owner state without changing it.
    pub const fn owner_snapshot(&self) -> OwnerSnapshot {
        self.session.input_owner.snapshot()
    }

    /// Borrow the one sink for parent-owned polling and focused tests.
    pub const fn effects(&self) -> &E {
        &self.session.effects
    }

    /// Mutably borrow the one sink for parent-owned testing and adapters.
    pub fn effects_mut(&mut self) -> &mut E {
        &mut self.session.effects
    }

    /// Complete the parent's reserved startup launch after registry/journal work.
    /// This is not a client route and never authorizes a new effect.
    pub(crate) fn complete_startup(
        &mut self,
        key: OperationKey,
        outcome: Outcome,
    ) -> Result<(), ServerError> {
        self.global.complete_startup(key, outcome)
    }

    /// Decode, authorize, preflight, admit, and synchronously handle one control request.
    pub fn handle_control(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        SessionHandler::new(&mut self.global, &mut self.session)
            .handle_control(connection, payload, now_secs)
    }

    /// Decode and handle one session request without creating process effects.
    pub fn handle_session(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        SessionHandler::new(&mut self.global, &mut self.session)
            .handle_session(connection, payload, now_secs)
    }

    /// Read one bounded screen and bind it to current metadata and journal state.
    pub fn reconcile(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        max_bytes: u32,
        max_lines: u16,
        now_secs: u64,
    ) -> Result<ReconcileState, ServerError> {
        SessionHandler::new(&mut self.global, &mut self.session)
            .reconcile(connection, target, max_bytes, max_lines, now_secs)
    }

    /// Read a bounded ordered event page for this owner's fixed target.
    pub fn read_events(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        cursor: EventCursor,
        limit: u16,
        now_secs: u64,
    ) -> Result<EventPage, ServerError> {
        SessionHandler::new(&mut self.global, &mut self.session)
            .read_events(connection, target, cursor, limit, now_secs)
    }

    /// Expire leases and input ownership, then give the sink one bounded poll.
    pub fn poll(&mut self, now_secs: u64) -> Result<(), ServerError> {
        self.global.expire(now_secs);
        SessionHandler::new(&mut self.global, &mut self.session).poll_session(now_secs)
    }
}

impl GlobalAuthority {
    fn expire(&mut self, now_secs: u64) {
        self.ledger.expire_leases(now_secs);
        self.grants.retain(|_, grant| now_secs < grant.expires_secs);
    }

    fn operation_status(
        &self,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        self.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0 {
            return Err(ServerError::Unauthorized);
        }
        if key.generation != self.generation || key.lease != connection.lease {
            return preflight_reply(key, RequestPreflight::Rejected(RejectionCode::Unauthorized))?
                .ok_or(ServerError::Internal);
        }
        let status = self.ledger.status(key, now_secs)?;
        encode_response(Response {
            key,
            body: ResponseBody::Status(status),
        })
    }

    fn receipt(
        &mut self,
        connection: &Connection,
        generation: u64,
        lease: u64,
        through_sequence: u64,
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        self.require_live_connection(connection, now_secs)?;
        if generation != self.generation || lease != connection.lease {
            return Err(ServerError::Unauthorized);
        }
        self.ledger.receipt(lease, through_sequence, now_secs)?;
        let key = OperationKey {
            generation,
            lease,
            sequence: through_sequence,
        };
        encode_response(Response {
            key,
            body: ResponseBody::Status(OperationStatus::Completed(Outcome::Applied)),
        })
    }

    fn preflight_control(
        &self,
        connection: &Connection,
        request: &ControlRequest<'_>,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<RequestPreflight, ServerError> {
        self.require_live_connection(connection, now_secs)?;
        if request.key.lease != connection.lease {
            return Ok(RequestPreflight::Rejected(RejectionCode::Unauthorized));
        }
        let required_capability = match request.command {
            Command::Read { .. } => CAPABILITY_READ,
            Command::Text { .. }
            | Command::Paste { .. }
            | Command::Submit { .. }
            | Command::Interrupt { .. } => CAPABILITY_INPUT,
        };
        if connection.selected_capabilities & required_capability == 0 {
            return Ok(RequestPreflight::Rejected(RejectionCode::Unauthorized));
        }

        match self
            .ledger
            .existing_admission(request.key, payload, now_secs)?
        {
            None => Ok(RequestPreflight::Continue),
            Some(Admission::InProgress) => Ok(RequestPreflight::Known(OperationStatus::InProgress)),
            Some(Admission::Completed(outcome)) => {
                Ok(RequestPreflight::Known(OperationStatus::Completed(outcome)))
            }
            Some(Admission::Execute) => Err(ServerError::Internal),
        }
    }

    fn preflight_owner_mutation(
        &self,
        connection: &Connection,
        key: OperationKey,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<RequestPreflight, ServerError> {
        self.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_INPUT == 0
            || key.lease != connection.lease
            || key.generation != self.generation
        {
            return Ok(RequestPreflight::Rejected(RejectionCode::Unauthorized));
        }

        match self.ledger.existing_admission(key, payload, now_secs)? {
            None => Ok(RequestPreflight::Continue),
            Some(Admission::InProgress) => Ok(RequestPreflight::Known(OperationStatus::InProgress)),
            Some(Admission::Completed(outcome)) => {
                Ok(RequestPreflight::Known(OperationStatus::Completed(outcome)))
            }
            Some(Admission::Execute) => Err(ServerError::Internal),
        }
    }

    fn connect<E: EffectSink>(
        &mut self,
        session: &TargetState<E>,
        hello: &SessionHello,
        now_secs: u64,
    ) -> Result<(Connection, Welcome), ServerError> {
        self.connect_with_capability_mask(
            session,
            hello,
            now_secs,
            CAPABILITY_READ | CAPABILITY_INPUT | CAPABILITY_EVENTS,
        )
    }

    fn connect_with_capability_mask<E: EffectSink>(
        &mut self,
        session: &TargetState<E>,
        hello: &SessionHello,
        now_secs: u64,
        capability_mask: u64,
    ) -> Result<(Connection, Welcome), ServerError> {
        hello.validate()?;
        let expires_secs = now_secs
            .checked_add(LEASE_TTL_SECS)
            .ok_or(ServerError::Protocol(ProtocolError::TimeOverflow))?;
        // Only the multi-session owner includes the workspace bit in its
        // mask. Workspace observations have an independent journal and do
        // not depend on a particular pane's notification health.
        let workspace = capability_mask & crate::handshake_protocol::CAPABILITY_WORKSPACE_EVENTS;
        let pane_metadata = capability_mask & crate::handshake_protocol::CAPABILITY_PANE_METADATA;
        let screen_row_wraps =
            capability_mask & crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS;
        let pane_operations =
            capability_mask & crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS;
        let agent_observation =
            capability_mask & crate::handshake_protocol::CAPABILITY_AGENT_OBSERVATION;
        let available = (self.available_capabilities(session)
            | workspace
            | pane_metadata
            | screen_row_wraps
            | pane_operations
            | agent_observation
            | (capability_mask & crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY))
            & capability_mask;

        let (lease, capabilities) = match hello.resume {
            Some(resume) => {
                if resume.generation != self.generation {
                    return Err(ServerError::UnknownContext);
                }
                let grant = self
                    .grants
                    .get(&resume.lease)
                    .copied()
                    .ok_or(ServerError::UnknownContext)?;
                if now_secs >= grant.expires_secs || grant.target != session.target {
                    return Err(ServerError::UnknownContext);
                }
                let welcome = negotiate(
                    &hello.hello,
                    grant.capabilities & available,
                    self.generation,
                    resume.lease,
                )?;
                self.ledger.renew_lease(resume.lease, now_secs)?;
                self.grants.insert(
                    resume.lease,
                    LeaseGrant {
                        capabilities: grant.capabilities,
                        target: grant.target,
                        expires_secs,
                    },
                );
                (resume.lease, welcome.selected_capabilities)
            }
            None => {
                // Validate required bits before consuming a new lease ID.
                negotiate(&hello.hello, available, self.generation, 1)?;
                let lease = self.ledger.issue_lease(now_secs)?;
                let welcome = negotiate(&hello.hello, available, self.generation, lease)?;
                self.grants.insert(
                    lease,
                    LeaseGrant {
                        capabilities: welcome.selected_capabilities,
                        target: session.target,
                        expires_secs,
                    },
                );
                (lease, welcome.selected_capabilities)
            }
        };

        let welcome = negotiate(&hello.hello, capabilities, self.generation, lease)?;
        Ok((
            Connection {
                generation: self.generation,
                lease,
                selected_capabilities: welcome.selected_capabilities,
            },
            welcome,
        ))
    }

    /// Resume an existing lease solely for its original workspace observer
    /// after the pane that issued it has retired. This cannot mint authority
    /// or change the grant's target or capability set.
    pub(crate) fn resume_workspace_observer(
        &mut self,
        target: TargetRef,
        hello: &SessionHello,
        now_secs: u64,
    ) -> Result<(Connection, Welcome), ServerError> {
        hello.validate()?;
        let requested = hello.hello.required_capabilities | hello.hello.optional_capabilities;
        let workspace_only =
            CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_WORKSPACE_EVENTS;
        if requested != workspace_only {
            return Err(ServerError::UnknownContext);
        }
        let resume = hello.resume.ok_or(ServerError::UnknownContext)?;
        if resume.generation != self.generation {
            return Err(ServerError::UnknownContext);
        }
        let grant = self
            .grants
            .get(&resume.lease)
            .copied()
            .ok_or(ServerError::UnknownContext)?;
        if now_secs >= grant.expires_secs
            || grant.target != target
            || grant.capabilities & workspace_only != workspace_only
        {
            return Err(ServerError::UnknownContext);
        }
        let welcome = negotiate(
            &hello.hello,
            grant.capabilities & workspace_only,
            self.generation,
            resume.lease,
        )?;
        let expires_secs = now_secs
            .checked_add(LEASE_TTL_SECS)
            .ok_or(ServerError::Protocol(ProtocolError::TimeOverflow))?;
        self.ledger.renew_lease(resume.lease, now_secs)?;
        self.grants.insert(
            resume.lease,
            LeaseGrant {
                capabilities: grant.capabilities,
                target: grant.target,
                expires_secs,
            },
        );
        Ok((
            Connection {
                generation: self.generation,
                lease: resume.lease,
                selected_capabilities: welcome.selected_capabilities,
            },
            welcome,
        ))
    }
    fn complete_startup(&mut self, key: OperationKey, outcome: Outcome) -> Result<(), ServerError> {
        self.ledger.complete(key, outcome)?;
        Ok(())
    }
    fn available_capabilities<E: EffectSink>(&self, session: &TargetState<E>) -> u64 {
        CAPABILITY_READ
            | if self.allow_input {
                CAPABILITY_INPUT
            } else {
                0
            }
            | if session.notifications_available {
                CAPABILITY_EVENTS
            } else {
                0
            }
    }
    fn require_live_connection(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<(), ServerError> {
        if connection.generation != self.generation || connection.lease == 0 {
            return Err(ServerError::UnknownContext);
        }
        let grant = self
            .grants
            .get(&connection.lease)
            .ok_or(ServerError::UnknownContext)?;
        if now_secs >= grant.expires_secs
            || connection.selected_capabilities & !grant.capabilities != 0
        {
            return Err(ServerError::UnknownContext);
        }
        Ok(())
    }
    fn require_target_scope(
        &self,
        connection: &Connection,
        target: TargetRef,
    ) -> Result<(), ServerError> {
        let grant = self
            .grants
            .get(&connection.lease)
            .ok_or(ServerError::UnknownContext)?;
        if grant.target == target {
            Ok(())
        } else {
            Err(ServerError::Unauthorized)
        }
    }
    fn active_input_grant(
        &self,
        lease: u64,
        target: TargetRef,
        now_secs: u64,
    ) -> Result<(), RejectionCode> {
        let Some(grant) = self.grants.get(&lease) else {
            return Err(RejectionCode::UnknownContext);
        };
        if now_secs >= grant.expires_secs {
            return Err(RejectionCode::UnknownContext);
        }
        if grant.target != target {
            return Err(RejectionCode::Unauthorized);
        }
        if grant.capabilities & CAPABILITY_INPUT == 0 {
            return Err(RejectionCode::Unauthorized);
        }
        Ok(())
    }
}

impl<'a, E: EffectSink> SessionHandler<'a, E> {
    fn new(global: &'a mut GlobalAuthority, session: &'a mut TargetState<E>) -> Self {
        Self { global, session }
    }

    /// Decode, authorize, preflight, admit, and synchronously handle one control request.
    fn handle_control(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        let result = self.handle_control_inner(connection, payload, now_secs);
        self.synchronize_metadata();
        result
    }

    fn handle_control_inner(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        let request = decode_request_payload(payload)?;
        let decision = self
            .global
            .preflight_control(connection, &request, payload, now_secs)?;
        if let Some(reply) = preflight_reply(request.key, decision)? {
            return Ok(reply);
        }

        self.handle_control_preflighted(connection, request, payload, now_secs)
    }

    fn handle_control_preflighted(
        &mut self,
        connection: &Connection,
        request: ControlRequest<'_>,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        if request.target != self.session.target {
            return self.rejected(request.key, RejectionCode::StaleTarget);
        }
        match self
            .global
            .require_target_scope(connection, self.session.target)
        {
            Ok(()) => {}
            Err(ServerError::Unauthorized) => {
                return self.rejected(request.key, RejectionCode::Unauthorized);
            }
            Err(error) => return Err(error),
        }

        match request.command {
            Command::Read {
                source,
                max_bytes,
                max_lines,
            } => self.handle_read(request, source, max_bytes, max_lines, payload, now_secs),
            command => self.handle_input(request, command, payload, now_secs),
        }
    }

    /// Decode and handle one session request without creating process effects.
    fn handle_session(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        let result = self.handle_session_inner(connection, payload, now_secs);
        self.synchronize_metadata();
        result
    }

    fn handle_session_inner(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        let request = decode_session_request(payload)?;
        self.global.require_live_connection(connection, now_secs)?;
        match request {
            SessionRequest::Describe => {
                self.require_capability(connection, CAPABILITY_READ)?;
                self.global
                    .require_target_scope(connection, self.session.target)?;
                Ok(SessionReply::Info(self.session_info()?))
            }
            SessionRequest::Claim {
                key,
                target,
                expected_owner_revision,
                ttl_seconds,
            } => {
                let mutation = OwnerMutation::Claim {
                    expected_revision: expected_owner_revision,
                    ttl_seconds,
                };
                self.handle_owner_mutation(connection, key, target, mutation, payload, now_secs)
            }
            SessionRequest::Release {
                key,
                target,
                expected_owner_revision,
            } => self.handle_owner_mutation(
                connection,
                key,
                target,
                OwnerMutation::Release {
                    expected_revision: expected_owner_revision,
                },
                payload,
                now_secs,
            ),
            SessionRequest::Handoff {
                key,
                target,
                expected_owner_revision,
                recipient_lease,
                ttl_seconds,
            } => self.handle_owner_mutation(
                connection,
                key,
                target,
                OwnerMutation::Handoff {
                    expected_revision: expected_owner_revision,
                    recipient_lease,
                    ttl_seconds,
                },
                payload,
                now_secs,
            ),
            SessionRequest::OperationStatus { key } => {
                self.require_capability(connection, CAPABILITY_READ)?;
                if key.generation != self.global.generation || key.lease != connection.lease {
                    return self
                        .rejected(key, RejectionCode::Unauthorized)
                        .map(SessionReply::Payload);
                }
                let status = self.global.ledger.status(key, now_secs)?;
                self.status_response(key, status).map(SessionReply::Payload)
            }
            SessionRequest::Receipt {
                generation,
                lease,
                through_sequence,
            } => {
                if generation != self.global.generation || lease != connection.lease {
                    return Err(ServerError::Unauthorized);
                }
                self.global
                    .ledger
                    .receipt(lease, through_sequence, now_secs)?;
                let key = OperationKey {
                    generation,
                    lease,
                    sequence: through_sequence,
                };
                self.status_response(key, OperationStatus::Completed(Outcome::Applied))
                    .map(SessionReply::Payload)
            }
            SessionRequest::Detach => Ok(SessionReply::Detached),
        }
    }

    /// Read one bounded screen and bind it to current metadata and journal state.
    fn reconcile(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        max_bytes: u32,
        max_lines: u16,
        now_secs: u64,
    ) -> Result<ReconcileState, ServerError> {
        self.require_watch_authority(connection, target, now_secs)?;
        self.reconcile_preflighted(max_bytes, max_lines)
    }

    fn reconcile_preflighted(
        &mut self,
        max_bytes: u32,
        max_lines: u16,
    ) -> Result<ReconcileState, ServerError> {
        if !(RECONCILIATION_PREFIX_BYTES as u32..=MAX_RECONCILIATION_BYTES as u32)
            .contains(&max_bytes)
            || !(1..=MAX_RECONCILIATION_LINES).contains(&max_lines)
        {
            return Err(ServerError::InvalidWatchRequest);
        }

        self.synchronize_metadata();
        if !self.session.notifications_available {
            return Err(ServerError::NotificationsUnavailable);
        }

        let max_text = usize::try_from(max_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_sub(RECONCILIATION_PREFIX_BYTES))
            .ok_or(ServerError::InvalidWatchRequest)?;
        let read_result = self
            .session
            .effects
            .read(ReadSource::Screen, max_text, max_lines);

        // A sink read may advance its screen. Record that fact before binding a
        // cursor, then reject a result whose revision no longer names that screen.
        self.synchronize_metadata();
        if !self.session.notifications_available {
            return Err(ServerError::NotificationsUnavailable);
        }
        let result = read_result.map_err(|_| ServerError::SnapshotReadFailed)?;
        let current_revision = self
            .session
            .last_screen_revision
            .ok_or(ServerError::NotificationsUnavailable)?;
        if result.revision == 0 {
            self.session.notifications_available = false;
            return Err(ServerError::NotificationsUnavailable);
        }
        if result.revision != current_revision {
            return Err(ServerError::SnapshotChanged);
        }
        let line_count = if result.text.is_empty() {
            0
        } else {
            1 + result.text.bytes().filter(|byte| *byte == b'\n').count()
        };
        if line_count > usize::from(max_lines) {
            return Err(ServerError::InvalidSnapshot);
        }

        let info = self.session_info()?;
        let cursor = self.session.journal.current_cursor();
        let reconciliation = Reconciliation {
            info,
            cursor,
            screen_revision: result.revision,
            truncated: result.truncated,
            text: &result.text,
        };
        let encoded =
            encode_reconciliation(&reconciliation).map_err(|_| ServerError::InvalidSnapshot)?;
        if encoded.len() > max_bytes as usize {
            return Err(ServerError::InvalidSnapshot);
        }

        Ok(ReconcileState {
            info,
            cursor,
            screen_revision: result.revision,
            truncated: result.truncated,
            text: result.text,
        })
    }

    /// Read a bounded ordered event page for this owner's fixed target.
    fn read_events(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        cursor: EventCursor,
        limit: u16,
        now_secs: u64,
    ) -> Result<EventPage, ServerError> {
        self.require_watch_authority(connection, target, now_secs)?;
        self.read_events_preflighted(cursor, limit)
    }

    fn read_events_preflighted(
        &mut self,
        cursor: EventCursor,
        limit: u16,
    ) -> Result<EventPage, ServerError> {
        if cursor.generation != self.global.generation {
            return Err(ServerError::WrongGeneration);
        }
        if !(1..=MAX_WATCH_EVENTS).contains(&limit) {
            return Err(ServerError::InvalidWatchRequest);
        }

        self.synchronize_metadata();
        if !self.session.notifications_available {
            return Err(ServerError::NotificationsUnavailable);
        }

        let latest = self.session.journal.current_cursor();
        let events = match self.session.journal.read_after(cursor, usize::from(limit)) {
            Ok(events) => events,
            Err(JournalError::CursorExpired) => {
                return Ok(EventPage {
                    events: Vec::new(),
                    cursor: latest,
                    resync_required: true,
                });
            }
            Err(error) => return Err(map_journal_error(error)),
        };
        let cursor = events
            .last()
            .map(|event| EventCursor {
                generation: self.global.generation,
                sequence: event.cursor,
            })
            .unwrap_or(latest);
        Ok(EventPage {
            events,
            cursor,
            resync_required: false,
        })
    }

    fn poll_session(&mut self, now_secs: u64) -> Result<(), ServerError> {
        let result = (|| {
            match self.session.input_owner.expire(now_secs)? {
                ExpireResult::Vacant
                | ExpireResult::NotDue
                | ExpireResult::PendingNeedsResolution(_)
                | ExpireResult::Released(_) => {}
            }
            self.session
                .effects
                .poll()
                .map_err(|_| ServerError::EffectPollFailed)
        })();
        self.synchronize_metadata();
        result
    }

    fn handle_read(
        &mut self,
        request: ControlRequest<'_>,
        source: ReadSource,
        max_bytes: u32,
        max_lines: u16,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        if let Err(code) = self.session.effects.validate(request.command) {
            return self.rejected(request.key, code);
        }
        let Some(text_budget) = usize::try_from(max_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_sub(READ_RESPONSE_METADATA_BYTES))
        else {
            return self.rejected(request.key, RejectionCode::InvalidRequest);
        };

        match self.global.ledger.admit(request.key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => {
                return self.status_response(request.key, OperationStatus::InProgress);
            }
            Ok(Admission::Completed(outcome)) => {
                return self.status_response(request.key, OperationStatus::Completed(outcome));
            }
            Err(error) if is_capacity_error(&error) => {
                return self.rejected(request.key, RejectionCode::Capacity);
            }
            Err(error) => return Err(ServerError::Protocol(error)),
        }

        let result = self.session.effects.read(source, text_budget, max_lines);
        match result {
            Ok(result)
                if valid_read_result(
                    &result,
                    self.session.target,
                    request.key,
                    source,
                    max_bytes,
                    max_lines,
                ) =>
            {
                self.global.ledger.complete(request.key, Outcome::Applied)?;
                encode_response(Response {
                    key: request.key,
                    body: ResponseBody::Read {
                        target: self.session.target,
                        source,
                        screen_revision: result.revision,
                        truncated: result.truncated,
                        text: &result.text,
                    },
                })
            }
            Ok(_) => {
                self.global.ledger.complete(request.key, Outcome::Failed)?;
                self.status_response(request.key, OperationStatus::Completed(Outcome::Failed))
            }
            Err(outcome) => {
                let outcome = terminal_outcome(outcome);
                self.global.ledger.complete(request.key, outcome)?;
                self.status_response(request.key, OperationStatus::Completed(outcome))
            }
        }
    }

    fn handle_input(
        &mut self,
        request: ControlRequest<'_>,
        command: Command<'_>,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        let owner_revision = match command {
            Command::Text { owner_revision, .. }
            | Command::Paste { owner_revision, .. }
            | Command::Submit { owner_revision, .. }
            | Command::Interrupt { owner_revision } => owner_revision,
            Command::Read { .. } => return Err(ServerError::Internal),
        };

        let caller = Owner::Client(request.key.lease);
        let mut candidate_owner = self.session.input_owner.clone();
        if let Err(error) =
            candidate_owner.begin_input(caller, owner_revision, request.key, now_secs)
        {
            return self.rejected(request.key, owner_rejection(error));
        }
        if let Err(code) = self.session.effects.validate(command) {
            return self.rejected(request.key, code);
        }

        match self.global.ledger.admit(request.key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => {
                return self.status_response(request.key, OperationStatus::InProgress);
            }
            Ok(Admission::Completed(outcome)) => {
                return self.status_response(request.key, OperationStatus::Completed(outcome));
            }
            Err(error) if is_capacity_error(&error) => {
                return self.rejected(request.key, RejectionCode::Capacity);
            }
            Err(error) => return Err(ServerError::Protocol(error)),
        }

        // The candidate was validated before admission; commit the input pin
        // only after this call reserved the unique key for execution.
        self.session.input_owner = candidate_owner;
        let outcome = terminal_outcome(self.session.effects.input(command));
        self.global.ledger.complete(request.key, outcome)?;
        self.session.input_owner.finish_input(request.key)?;
        self.status_response(request.key, OperationStatus::Completed(outcome))
    }

    fn handle_owner_mutation(
        &mut self,
        connection: &Connection,
        key: OperationKey,
        target: TargetRef,
        mutation: OwnerMutation,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        let decision = self
            .global
            .preflight_owner_mutation(connection, key, payload, now_secs)?;
        if let Some(reply) = preflight_reply(key, decision)? {
            return Ok(SessionReply::Payload(reply));
        }

        self.handle_owner_mutation_preflighted(connection, key, target, mutation, payload, now_secs)
    }

    fn handle_owner_mutation_preflighted(
        &mut self,
        connection: &Connection,
        key: OperationKey,
        target: TargetRef,
        mutation: OwnerMutation,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        if target != self.session.target {
            return self
                .rejected(key, RejectionCode::StaleTarget)
                .map(SessionReply::Payload);
        }
        match self
            .global
            .require_target_scope(connection, self.session.target)
        {
            Ok(()) => {}
            Err(ServerError::Unauthorized) => {
                return self
                    .rejected(key, RejectionCode::Unauthorized)
                    .map(SessionReply::Payload);
            }
            Err(error) => return Err(error),
        }

        let caller = Owner::Client(connection.lease);
        let mut candidate = self.session.input_owner.clone();
        let transition = match mutation {
            OwnerMutation::Claim {
                expected_revision,
                ttl_seconds,
            } => candidate.claim(caller, expected_revision, u64::from(ttl_seconds), now_secs),
            OwnerMutation::Release { expected_revision } => {
                candidate.release(caller, expected_revision)
            }
            OwnerMutation::Handoff {
                expected_revision,
                recipient_lease,
                ttl_seconds,
            } => {
                match self
                    .global
                    .active_input_grant(recipient_lease, self.session.target, now_secs)
                {
                    Ok(()) => {}
                    Err(code) => return self.rejected(key, code).map(SessionReply::Payload),
                }
                candidate.handoff(
                    caller,
                    expected_revision,
                    Owner::Client(recipient_lease),
                    u64::from(ttl_seconds),
                    now_secs,
                )
            }
        };
        if let Err(error) = transition {
            return self
                .rejected(key, owner_rejection(error))
                .map(SessionReply::Payload);
        }

        match self.global.ledger.admit(key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => {
                return self
                    .status_response(key, OperationStatus::InProgress)
                    .map(SessionReply::Payload);
            }
            Ok(Admission::Completed(outcome)) => {
                return self
                    .status_response(key, OperationStatus::Completed(outcome))
                    .map(SessionReply::Payload);
            }
            Err(error) if is_capacity_error(&error) => {
                return self
                    .rejected(key, RejectionCode::Capacity)
                    .map(SessionReply::Payload);
            }
            Err(error) => return Err(ServerError::Protocol(error)),
        }

        self.global.ledger.complete(key, Outcome::Applied)?;
        self.session.input_owner = candidate;
        self.status_response(key, OperationStatus::Completed(Outcome::Applied))
            .map(SessionReply::Payload)
    }

    fn session_info(&self) -> Result<SessionInfo, ServerError> {
        let owner = self.session.input_owner.snapshot();
        let (owner_lease, owner_expires_secs) = match owner.holder {
            None => (0, 0),
            Some(Owner::Client(lease)) => (lease, owner.expires_secs),
            Some(Owner::Human(_)) => return Err(ServerError::UnsupportedOwner),
        };
        Ok(SessionInfo {
            target: self.session.target,
            owner_revision: owner.revision,
            owner_lease,
            owner_expires_secs,
            child_exited: self.session.effects.child_exited(),
        })
    }

    fn require_watch_authority(
        &self,
        connection: &Connection,
        target: TargetRef,
        now_secs: u64,
    ) -> Result<(), ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & (CAPABILITY_READ | CAPABILITY_EVENTS)
            != (CAPABILITY_READ | CAPABILITY_EVENTS)
        {
            return Err(ServerError::Unauthorized);
        }
        self.require_watch_target_scope(connection, target)
    }

    fn require_watch_target_scope(
        &self,
        connection: &Connection,
        target: TargetRef,
    ) -> Result<(), ServerError> {
        if target != self.session.target {
            return Err(ServerError::TargetStale);
        }
        self.global
            .require_target_scope(connection, self.session.target)
    }

    /// Record committed metadata changes without allowing journal failures to
    /// change normal control, input, or polling outcomes.
    fn synchronize_metadata(&mut self) {
        if !self.session.notifications_available {
            return;
        }

        let Some(previous_screen_revision) = self.session.last_screen_revision else {
            self.session.notifications_available = false;
            return;
        };
        let Some(screen_revision) = self
            .session
            .effects
            .screen_revision()
            .filter(|revision| *revision != 0)
        else {
            self.session.notifications_available = false;
            return;
        };
        let Some(owner) = owner_observation(self.session.input_owner.snapshot()) else {
            self.session.notifications_available = false;
            return;
        };
        let Some(previous_owner) = self.session.last_owner_observation else {
            self.session.notifications_available = false;
            return;
        };
        let child_exited = self.session.effects.child_exited();

        if screen_revision < previous_screen_revision
            || owner.revision < previous_owner.revision
            || (owner.revision == previous_owner.revision
                && owner.owner_lease != previous_owner.owner_lease)
            || (self.session.last_child_exited && !child_exited)
        {
            self.session.notifications_available = false;
            return;
        }

        if screen_revision > previous_screen_revision
            && !self.record_event(EventBody::ScreenChanged {
                revision: screen_revision,
            })
        {
            return;
        }
        if owner != previous_owner
            && !self.record_event(EventBody::InputOwnerChanged {
                revision: owner.revision,
                owner_lease: owner.owner_lease,
            })
        {
            return;
        }
        if !self.session.last_child_exited
            && child_exited
            && !self.record_event(EventBody::ChildExited)
        {
            return;
        }

        self.session.last_screen_revision = Some(screen_revision);
        self.session.last_owner_observation = Some(owner);
        self.session.last_child_exited = child_exited;
    }

    fn record_event(&mut self, body: EventBody) -> bool {
        if self
            .session
            .journal
            .push(self.session.target, body)
            .is_err()
        {
            self.session.notifications_available = false;
            false
        } else {
            true
        }
    }

    fn require_capability(
        &self,
        connection: &Connection,
        capability: u64,
    ) -> Result<(), ServerError> {
        if connection.selected_capabilities & capability == 0 {
            Err(ServerError::Unauthorized)
        } else {
            Ok(())
        }
    }

    fn rejected(&self, key: OperationKey, code: RejectionCode) -> Result<Vec<u8>, ServerError> {
        encode_response(Response {
            key,
            body: ResponseBody::Rejected(code),
        })
    }

    fn status_response(
        &self,
        key: OperationKey,
        status: OperationStatus,
    ) -> Result<Vec<u8>, ServerError> {
        encode_response(Response {
            key,
            body: ResponseBody::Status(status),
        })
    }
}

fn preflight_reply(
    key: OperationKey,
    decision: RequestPreflight,
) -> Result<Option<Vec<u8>>, ServerError> {
    let body = match decision {
        RequestPreflight::Continue => return Ok(None),
        RequestPreflight::Rejected(code) => ResponseBody::Rejected(code),
        RequestPreflight::Known(status) => ResponseBody::Status(status),
    };
    Ok(Some(encode_response(Response { key, body })?))
}

fn encode_response(response: Response<'_>) -> Result<Vec<u8>, ServerError> {
    let length = response.encoded_len()?;
    let mut encoded = Vec::with_capacity(length);
    encode_response_payload(&response, &mut encoded).map_err(map_response_io_error)?;
    Ok(encoded)
}

const READ_RESPONSE_METADATA_BYTES: usize = 90;

fn target_is_valid(target: TargetRef) -> bool {
    target.generation != 0
        && target.workspace != 0
        && target.pane != 0
        && target.session != 0
        && target.occupant_revision != 0
}

fn owner_observation(snapshot: OwnerSnapshot) -> Option<OwnerObservation> {
    if snapshot.revision == 0 {
        return None;
    }
    let owner_lease = match snapshot.holder {
        None => 0,
        Some(Owner::Client(lease)) if lease != 0 => lease,
        Some(Owner::Client(_)) | Some(Owner::Human(_)) => return None,
    };
    Some(OwnerObservation {
        revision: snapshot.revision,
        owner_lease,
    })
}

fn map_journal_error(error: JournalError) -> ServerError {
    match error {
        JournalError::GenerationMismatch | JournalError::ZeroGeneration => {
            ServerError::WrongGeneration
        }
        JournalError::FutureCursor => ServerError::CursorAhead,
        JournalError::InvalidLimit => ServerError::InvalidWatchRequest,
        JournalError::CursorExpired | JournalError::SequenceExhausted => {
            ServerError::NotificationsUnavailable
        }
        JournalError::InvalidEvent => ServerError::Internal,
    }
}

fn is_capacity_error(error: &ProtocolError) -> bool {
    matches!(
        error,
        ProtocolError::CapacityExceeded
            | ProtocolError::LeaseCapacityExceeded
            | ProtocolError::ActiveLeaseCapacityExceeded
            | ProtocolError::GlobalOperationCapacityExceeded
            | ProtocolError::PayloadCapacityExceeded
    )
}

fn owner_rejection(error: OwnerError) -> RejectionCode {
    match error {
        OwnerError::StaleRevision { .. } => RejectionCode::StaleOwner,
        OwnerError::Expired => RejectionCode::ExpiredOwner,
        OwnerError::InputPending => RejectionCode::InputBusy,
        OwnerError::HeldByAnotherOwner | OwnerError::WrongOwner | OwnerError::Vacant => {
            RejectionCode::Unauthorized
        }
        OwnerError::ZeroIdentifier
        | OwnerError::InvalidTtl
        | OwnerError::RecipientIsCurrentOwner
        | OwnerError::NoPendingInput
        | OwnerError::PendingKeyMismatch
        | OwnerError::TimeOverflow
        | OwnerError::RevisionExhausted => RejectionCode::InvalidRequest,
    }
}

fn terminal_outcome(outcome: Outcome) -> Outcome {
    if outcome == Outcome::Accepted {
        Outcome::Unknown
    } else {
        outcome
    }
}

fn valid_read_result(
    result: &ReadResult,
    target: TargetRef,
    key: OperationKey,
    source: ReadSource,
    max_bytes: u32,
    max_lines: u16,
) -> bool {
    if result.revision == 0 || result.text.len() > (u32::MAX as usize) {
        return false;
    }
    let line_count = if result.text.is_empty() {
        0
    } else {
        1 + result.text.bytes().filter(|byte| *byte == b'\n').count()
    };
    if line_count > usize::from(max_lines) {
        return false;
    }
    let response = Response {
        key,
        body: ResponseBody::Read {
            target,
            source,
            screen_revision: result.revision,
            truncated: result.truncated,
            text: &result.text,
        },
    };
    response
        .encoded_len()
        .is_ok_and(|length| length <= max_bytes as usize)
}

fn map_response_io_error(_error: io::Error) -> ServerError {
    ServerError::Internal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::encode_request_payload;
    use crate::handshake_protocol::{Hello, CAPABILITY_EVENTS};
    use crate::response_protocol::decode_response_payload;
    use crate::session_protocol::{encode_session_request, LeaseRef};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const GENERATION: u64 = 7;
    const WORKSPACE: u64 = 11;
    const PANE: u64 = 13;
    const SESSION: u64 = 17;
    const OCCUPANT_REVISION: u64 = 19;

    fn target() -> TargetRef {
        TargetRef {
            generation: GENERATION,
            workspace: WORKSPACE,
            pane: PANE,
            session: SESSION,
            occupant_revision: OCCUPANT_REVISION,
        }
    }

    struct FakeSink {
        validations: Cell<usize>,
        reads: usize,
        inputs: usize,
        polls: usize,
        reject_validation: Option<RejectionCode>,
        read_result: Result<ReadResult, Outcome>,
        input_outcomes: VecDeque<Outcome>,
        poll_result: Result<(), Outcome>,
        child_exited: bool,
        screen_revision: Option<u64>,
        screen_revision_after_read: Option<u64>,
    }

    impl Default for FakeSink {
        fn default() -> Self {
            Self {
                validations: Cell::new(0),
                reads: 0,
                inputs: 0,
                polls: 0,
                reject_validation: None,
                read_result: Ok(ReadResult {
                    text: String::from("screen"),
                    revision: 1,
                    truncated: false,
                }),
                input_outcomes: VecDeque::from([Outcome::Applied]),
                poll_result: Ok(()),
                child_exited: false,
                screen_revision: None,
                screen_revision_after_read: None,
            }
        }
    }

    impl EffectSink for FakeSink {
        fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
            self.validations.set(self.validations.get() + 1);
            match self.reject_validation {
                Some(code) => Err(code),
                None => Ok(()),
            }
        }

        fn read(
            &mut self,
            _source: ReadSource,
            _max_text_bytes: usize,
            _max_lines: u16,
        ) -> Result<ReadResult, Outcome> {
            self.reads += 1;
            if let Some(revision) = self.screen_revision_after_read.take() {
                self.screen_revision = Some(revision);
            }
            self.read_result.clone()
        }

        fn input(&mut self, _command: Command<'_>) -> Outcome {
            self.inputs += 1;
            self.input_outcomes.pop_front().unwrap_or(Outcome::Applied)
        }

        fn poll(&mut self) -> Result<(), Outcome> {
            self.polls += 1;
            self.poll_result
        }

        fn child_exited(&self) -> bool {
            self.child_exited
        }

        fn screen_revision(&self) -> Option<u64> {
            self.screen_revision
        }
    }

    struct BlockingValidationSink {
        validation_calls: Arc<AtomicUsize>,
        input_calls: Arc<AtomicUsize>,
        reached_validation: SyncSender<(u64, String)>,
        release_validation: Mutex<Receiver<()>>,
    }

    impl EffectSink for BlockingValidationSink {
        fn validate(&self, command: Command<'_>) -> Result<(), RejectionCode> {
            let validation_index = self.validation_calls.fetch_add(1, Ordering::SeqCst);
            if validation_index == 0 {
                let Command::Submit {
                    owner_revision,
                    text,
                } = command
                else {
                    return Err(RejectionCode::Unsupported);
                };
                self.reached_validation
                    .send((owner_revision, text.to_owned()))
                    .map_err(|_| RejectionCode::Capacity)?;
                self.release_validation
                    .lock()
                    .expect("validation release lock poisoned")
                    .recv()
                    .map_err(|_| RejectionCode::Capacity)?;
            }
            Ok(())
        }

        fn read(
            &mut self,
            _source: ReadSource,
            _max_text_bytes: usize,
            _max_lines: u16,
        ) -> Result<ReadResult, Outcome> {
            Err(Outcome::Failed)
        }

        fn input(&mut self, _command: Command<'_>) -> Outcome {
            self.input_calls.fetch_add(1, Ordering::SeqCst);
            Outcome::Applied
        }

        fn poll(&mut self) -> Result<(), Outcome> {
            Ok(())
        }

        fn child_exited(&self) -> bool {
            false
        }
    }

    fn state() -> StateOwner<FakeSink> {
        StateOwner::new(GENERATION, target(), FakeSink::default(), true).unwrap()
    }

    fn watch_state() -> StateOwner<FakeSink> {
        let effects = FakeSink {
            screen_revision: Some(1),
            ..FakeSink::default()
        };
        StateOwner::new(GENERATION, target(), effects, true).unwrap()
    }

    fn another_target(field: usize) -> TargetRef {
        let mut target = target();
        match field {
            0 => target.generation += 1,
            1 => target.workspace += 1,
            2 => target.pane += 1,
            3 => target.session += 1,
            4 => target.occupant_revision += 1,
            _ => unreachable!(),
        }
        target
    }

    fn target_state(target: TargetRef) -> TargetState<FakeSink> {
        let input_owner = InputOwner::new();
        let last_owner_observation = owner_observation(input_owner.snapshot());
        TargetState {
            target,
            input_owner,
            effects: FakeSink {
                screen_revision: Some(1),
                ..FakeSink::default()
            },
            journal: EventJournal::new(target.generation).unwrap(),
            notifications_available: true,
            last_screen_revision: Some(1),
            last_owner_observation,
            last_child_exited: false,
        }
    }

    fn hello(required: u64, optional: u64) -> SessionHello {
        SessionHello {
            hello: Hello {
                nonce: 31,
                required_capabilities: required,
                optional_capabilities: optional,
            },
            resume: None,
        }
    }

    fn connect(owner: &mut StateOwner<FakeSink>) -> Connection {
        owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0
    }

    fn connect_with_events(owner: &mut StateOwner<FakeSink>) -> Connection {
        owner
            .connect(
                &hello(CAPABILITY_READ, CAPABILITY_INPUT | CAPABILITY_EVENTS),
                1,
            )
            .unwrap()
            .0
    }

    fn operation_key(lease: u64, sequence: u64) -> OperationKey {
        OperationKey {
            generation: GENERATION,
            lease,
            sequence,
        }
    }

    fn control_payload(
        key: OperationKey,
        request_target: TargetRef,
        command: Command<'_>,
    ) -> Vec<u8> {
        let request = ControlRequest {
            key,
            target: request_target,
            command,
        };
        let mut payload = Vec::new();
        encode_request_payload(&request, &mut payload).unwrap();
        payload
    }

    fn session_payload(request: SessionRequest) -> Vec<u8> {
        encode_session_request(&request).unwrap()
    }

    fn response(payload: &[u8]) -> Response<'_> {
        decode_response_payload(payload).unwrap()
    }

    fn claim(owner: &mut StateOwner<FakeSink>, connection: &Connection, seq: u64) -> u64 {
        let key = operation_key(connection.lease(), seq);
        let payload = session_payload(SessionRequest::Claim {
            key,
            target: target(),
            expected_owner_revision: owner.owner_snapshot().revision,
            ttl_seconds: 30,
        });
        let reply = owner.handle_session(connection, &payload, 2).unwrap();
        let SessionReply::Payload(payload) = reply else {
            panic!("claim must return an operation result");
        };
        assert_eq!(
            response(&payload).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        owner.owner_snapshot().revision
    }

    fn operation_status(
        owner: &mut StateOwner<FakeSink>,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> OperationStatus {
        let payload = session_payload(SessionRequest::OperationStatus { key });
        let SessionReply::Payload(payload) = owner
            .handle_session(connection, &payload, now_secs)
            .unwrap()
        else {
            panic!("status must return a payload");
        };
        match response(&payload).body {
            ResponseBody::Status(status) => status,
            _ => panic!("status response has the wrong kind"),
        }
    }

    fn rejection(payload: &[u8]) -> RejectionCode {
        match response(payload).body {
            ResponseBody::Rejected(code) => code,
            _ => panic!("expected a rejection"),
        }
    }

    #[test]
    fn negotiation_validates_before_lease_issue_and_never_grants_events() {
        let mut owner = state();
        let unsupported = owner.connect(&hello(CAPABILITY_EVENTS, 0), 1);
        assert!(matches!(unsupported, Err(ServerError::Handshake(_))));

        let (connection, welcome) = owner
            .connect(
                &hello(CAPABILITY_READ, CAPABILITY_INPUT | CAPABILITY_EVENTS),
                1,
            )
            .unwrap();
        assert_eq!(connection.lease(), 1);
        assert_eq!(
            welcome.selected_capabilities,
            CAPABILITY_READ | CAPABILITY_INPUT
        );
        assert_eq!(
            connection.selected_capabilities(),
            welcome.selected_capabilities
        );
        assert_eq!(owner.global.grants.len(), 1);
    }

    #[test]
    fn constructor_rejects_zero_or_mismatched_target_ids() {
        let mut invalid = target();
        invalid.session = 0;
        assert!(matches!(
            StateOwner::new(GENERATION, invalid, FakeSink::default(), true),
            Err(ServerError::InvalidConfiguration)
        ));
        assert!(matches!(
            StateOwner::new(GENERATION + 1, target(), FakeSink::default(), true),
            Err(ServerError::InvalidConfiguration)
        ));
    }

    #[test]
    fn with_ledger_preserves_internal_launch_history_and_lease_watermark() {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(1, 1).unwrap();
        let launch_key = operation_key(1, 1);
        assert_eq!(
            ledger.admit(launch_key, b"internal launch", 2),
            Ok(Admission::Execute)
        );
        ledger.complete(launch_key, Outcome::Applied).unwrap();
        let usage_before = ledger.usage();

        let mut owner =
            StateOwner::with_ledger(ledger, target(), FakeSink::default(), true).unwrap();
        assert_eq!(owner.global.ledger.usage(), usage_before);
        assert_eq!(
            owner.global.ledger.status(launch_key, 3),
            Ok(OperationStatus::Completed(Outcome::Applied))
        );
        let connection = connect(&mut owner);
        assert_eq!(connection.lease(), 2);
        let internal_resume = SessionHello {
            hello: hello(CAPABILITY_READ, 0).hello,
            resume: Some(LeaseRef {
                generation: GENERATION,
                lease: 1,
            }),
        };
        assert!(matches!(
            owner.connect(&internal_resume, 3),
            Err(ServerError::UnknownContext)
        ));
        let next_connection = owner.connect(&hello(CAPABILITY_READ, 0), 3).unwrap().0;
        assert_eq!(next_connection.lease(), 3);
        assert_eq!(
            owner.global.ledger.usage().lease_records,
            usage_before.lease_records + 2
        );
    }

    #[test]
    fn exact_input_duplicate_dispatches_once_and_changed_bytes_conflict() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let revision = claim(&mut owner, &connection, 1);
        let key = operation_key(connection.lease(), 2);
        let payload = control_payload(
            key,
            target(),
            Command::Text {
                owner_revision: revision,
                text: "one input",
            },
        );

        let first = owner.handle_control(&connection, &payload, 3).unwrap();
        assert_eq!(
            response(&first).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let validation_count = owner.effects().validations.get();
        let duplicate = owner.handle_control(&connection, &payload, 3).unwrap();
        assert_eq!(
            response(&duplicate).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.effects().inputs, 1);
        assert_eq!(owner.effects().validations.get(), validation_count);

        let conflict = control_payload(
            key,
            target(),
            Command::Text {
                owner_revision: revision,
                text: "changed",
            },
        );
        assert!(matches!(
            owner.handle_control(&connection, &conflict, 3),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));
        assert_eq!(owner.effects().inputs, 1);
    }

    #[test]
    fn submit_validation_barrier_precedes_admission_and_duplicate_does_not_replay() {
        let (reached_validation_tx, reached_validation_rx) = sync_channel(1);
        let (release_validation_tx, release_validation_rx) = sync_channel(1);
        let validation_calls = Arc::new(AtomicUsize::new(0));
        let input_calls = Arc::new(AtomicUsize::new(0));
        let sink = BlockingValidationSink {
            validation_calls: Arc::clone(&validation_calls),
            input_calls: Arc::clone(&input_calls),
            reached_validation: reached_validation_tx,
            release_validation: Mutex::new(release_validation_rx),
        };
        let mut owner = StateOwner::new(GENERATION, target(), sink, true).unwrap();
        let connection = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0;
        let claim_key = operation_key(connection.lease(), 1);
        let claim_payload = session_payload(SessionRequest::Claim {
            key: claim_key,
            target: target(),
            expected_owner_revision: owner.owner_snapshot().revision,
            ttl_seconds: 30,
        });
        let SessionReply::Payload(claim_reply) = owner
            .handle_session(&connection, &claim_payload, 2)
            .unwrap()
        else {
            panic!("claim must return an operation result");
        };
        assert_eq!(
            response(&claim_reply).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let owner_revision = owner.owner_snapshot().revision;
        let key = operation_key(connection.lease(), 2);
        let text = "barrier-before-admission";
        let payload = control_payload(
            key,
            target(),
            Command::Submit {
                owner_revision,
                text,
            },
        );

        let worker = std::thread::spawn(move || {
            let result = owner.handle_control(&connection, &payload, 3);
            (owner, result, payload)
        });

        let observed = reached_validation_rx.recv_timeout(Duration::from_secs(2));
        let no_input_at_barrier = input_calls.load(Ordering::SeqCst) == 0;
        // Always release the test sink before asserting, so a failed barrier
        // observation cannot leave the worker blocked.
        let _ = release_validation_tx.send(());
        let (mut owner, first_result, payload) = worker.join().unwrap();
        let (observed_revision, observed_text) =
            observed.expect("Submit must reach the blocking validation boundary");
        assert_eq!(observed_revision, owner_revision);
        assert_eq!(observed_text, text);
        assert!(
            no_input_at_barrier,
            "input ran before validation was released"
        );
        assert_eq!(validation_calls.load(Ordering::SeqCst), 1);

        let first = first_result.unwrap();
        assert_eq!(
            response(&first).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(input_calls.load(Ordering::SeqCst), 1);

        let duplicate = owner.handle_control(&connection, &payload, 3).unwrap();
        assert_eq!(
            response(&duplicate).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(input_calls.load(Ordering::SeqCst), 1);
        assert_eq!(validation_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn target_owner_lease_and_capability_rejections_do_not_admit_or_call_sink() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let revision = claim(&mut owner, &connection, 1);
        let ledger_before_requests = owner.global.ledger.usage();
        let before_validation = owner.effects().validations.get();

        let mut stale_target = target();
        stale_target.occupant_revision += 1;
        let stale_target_key = operation_key(connection.lease(), 2);
        let stale_target_request = control_payload(
            stale_target_key,
            stale_target,
            Command::Text {
                owner_revision: revision,
                text: "x",
            },
        );
        let rejected = owner
            .handle_control(&connection, &stale_target_request, 3)
            .unwrap();
        assert_eq!(rejection(&rejected), RejectionCode::StaleTarget);
        assert_eq!(
            operation_status(&mut owner, &connection, stale_target_key, 3),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.effects().validations.get(), before_validation);
        assert_eq!(owner.global.ledger.usage(), ledger_before_requests);

        let stale_owner_key = operation_key(connection.lease(), 3);
        let stale_owner_request = control_payload(
            stale_owner_key,
            target(),
            Command::Text {
                owner_revision: revision - 1,
                text: "x",
            },
        );
        let rejected = owner
            .handle_control(&connection, &stale_owner_request, 3)
            .unwrap();
        assert_eq!(rejection(&rejected), RejectionCode::StaleOwner);
        assert_eq!(
            operation_status(&mut owner, &connection, stale_owner_key, 3),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.effects().validations.get(), before_validation);
        assert_eq!(owner.global.ledger.usage(), ledger_before_requests);

        let foreign_key = OperationKey {
            lease: connection.lease() + 100,
            sequence: 4,
            ..operation_key(connection.lease(), 4)
        };
        let foreign_request = control_payload(
            foreign_key,
            target(),
            Command::Text {
                owner_revision: revision,
                text: "x",
            },
        );
        let rejected = owner
            .handle_control(&connection, &foreign_request, 3)
            .unwrap();
        assert_eq!(rejection(&rejected), RejectionCode::Unauthorized);
        assert_eq!(owner.effects().validations.get(), before_validation);
        assert_eq!(owner.effects().inputs, 0);
        assert_eq!(owner.global.ledger.usage(), ledger_before_requests);

        let mut read_only_owner = state();
        let read_only = read_only_owner
            .connect(&hello(CAPABILITY_READ, 0), 1)
            .unwrap()
            .0;
        let resumed_with_new_offer = SessionHello {
            hello: Hello {
                nonce: 32,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: CAPABILITY_INPUT,
            },
            resume: Some(LeaseRef {
                generation: GENERATION,
                lease: read_only.lease(),
            }),
        };
        let (resumed_read_only, welcome) =
            read_only_owner.connect(&resumed_with_new_offer, 2).unwrap();
        assert_eq!(resumed_read_only.lease(), read_only.lease());
        assert_eq!(welcome.selected_capabilities, CAPABILITY_READ);
        let read_only_ledger_before = read_only_owner.global.ledger.usage();
        let forbidden_key = operation_key(read_only.lease(), 1);
        let forbidden = control_payload(
            forbidden_key,
            target(),
            Command::Text {
                owner_revision: 1,
                text: "x",
            },
        );
        let denied = read_only_owner
            .handle_control(&read_only, &forbidden, 2)
            .unwrap();
        assert_eq!(rejection(&denied), RejectionCode::Unauthorized);
        assert_eq!(read_only_owner.effects().validations.get(), 0);
        assert_eq!(read_only_owner.effects().inputs, 0);
        assert_eq!(
            read_only_owner.global.ledger.usage(),
            read_only_ledger_before
        );
    }

    #[test]
    fn exact_owner_mutation_duplicate_precedes_changed_owner_revision() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let key = operation_key(connection.lease(), 1);
        let claim_payload = session_payload(SessionRequest::Claim {
            key,
            target: target(),
            expected_owner_revision: 1,
            ttl_seconds: 20,
        });
        owner
            .handle_session(&connection, &claim_payload, 2)
            .unwrap();
        let next_key = operation_key(connection.lease(), 2);
        let release_payload = session_payload(SessionRequest::Release {
            key: next_key,
            target: target(),
            expected_owner_revision: 2,
        });
        owner
            .handle_session(&connection, &release_payload, 3)
            .unwrap();
        assert_eq!(owner.owner_snapshot().revision, 3);

        let before = owner.global.ledger.usage();
        let duplicate = owner
            .handle_session(&connection, &claim_payload, 4)
            .unwrap();
        let SessionReply::Payload(payload) = duplicate else {
            panic!("mutation duplicate must return status");
        };
        assert_eq!(
            response(&payload).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.owner_snapshot().holder, None);
        assert_eq!(owner.global.ledger.usage(), before);
    }

    #[test]
    fn status_is_observation_only_and_does_not_dispatch() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let key = operation_key(connection.lease(), 1);
        assert_eq!(
            operation_status(&mut owner, &connection, key, 2),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.effects().reads, 0);
        let payload = control_payload(
            key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 4,
            },
        );
        let result = owner.handle_control(&connection, &payload, 2).unwrap();
        assert!(matches!(response(&result).body, ResponseBody::Read { .. }));
        assert_eq!(owner.effects().reads, 1);
        let duplicate = owner.handle_control(&connection, &payload, 2).unwrap();
        assert_eq!(
            response(&duplicate).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.effects().reads, 1);
    }

    #[test]
    fn receipt_retires_results_without_dispatch_or_replay() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let key = operation_key(connection.lease(), 1);
        let read = control_payload(
            key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 4,
            },
        );
        owner.handle_control(&connection, &read, 2).unwrap();
        assert_eq!(owner.effects().reads, 1);
        assert_eq!(owner.global.ledger.usage().retained_operations, 1);

        let receipt = session_payload(SessionRequest::Receipt {
            generation: GENERATION,
            lease: connection.lease(),
            through_sequence: key.sequence,
        });
        let SessionReply::Payload(result) = owner.handle_session(&connection, &receipt, 3).unwrap()
        else {
            panic!("receipt must return a status payload");
        };
        assert_eq!(
            response(&result).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.global.ledger.usage().retained_operations, 0);
        assert_eq!(owner.effects().reads, 1);

        let repeated = owner.handle_session(&connection, &receipt, 4).unwrap();
        assert!(matches!(repeated, SessionReply::Payload(_)));
        assert_eq!(owner.effects().reads, 1);
    }

    #[test]
    fn preflight_capacity_and_pending_input_reject_before_admission() {
        let mut owner = state();
        let connection = connect(&mut owner);
        owner.session.effects.reject_validation = Some(RejectionCode::Capacity);
        let read_key = operation_key(connection.lease(), 1);
        let read = control_payload(
            read_key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 4,
            },
        );
        let rejected = owner.handle_control(&connection, &read, 2).unwrap();
        assert_eq!(rejection(&rejected), RejectionCode::Capacity);
        assert_eq!(
            operation_status(&mut owner, &connection, read_key, 2),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.effects().reads, 0);

        owner.session.effects.reject_validation = None;
        let revision = claim(&mut owner, &connection, 2);
        let pending_key = operation_key(connection.lease(), 3);
        owner
            .session
            .input_owner
            .begin_input(
                Owner::Client(connection.lease()),
                revision,
                operation_key(connection.lease(), 99),
                3,
            )
            .unwrap();
        let input = control_payload(
            pending_key,
            target(),
            Command::Text {
                owner_revision: revision,
                text: "blocked",
            },
        );
        let rejected = owner.handle_control(&connection, &input, 3).unwrap();
        assert_eq!(rejection(&rejected), RejectionCode::InputBusy);
        assert_eq!(
            operation_status(&mut owner, &connection, pending_key, 3),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.effects().inputs, 0);
    }

    #[test]
    fn nonapplied_and_unexpected_accepted_input_are_terminal_and_never_replayed() {
        for (first_outcome, expected) in [
            (Outcome::Partial, Outcome::Partial),
            (Outcome::Unknown, Outcome::Unknown),
            (Outcome::Accepted, Outcome::Unknown),
        ] {
            let mut owner = state();
            owner.session.effects.input_outcomes = VecDeque::from([first_outcome]);
            let connection = connect(&mut owner);
            let revision = claim(&mut owner, &connection, 1);
            let key = operation_key(connection.lease(), 2);
            let payload = control_payload(
                key,
                target(),
                Command::Paste {
                    owner_revision: revision,
                    text: "authorized paste",
                },
            );
            let first = owner.handle_control(&connection, &payload, 3).unwrap();
            assert_eq!(
                response(&first).body,
                ResponseBody::Status(OperationStatus::Completed(expected))
            );
            let second = owner.handle_control(&connection, &payload, 3).unwrap();
            assert_eq!(
                response(&second).body,
                ResponseBody::Status(OperationStatus::Completed(expected))
            );
            assert_eq!(owner.effects().inputs, 1);
            assert_eq!(owner.owner_snapshot().pending, None);
        }
    }

    #[test]
    fn read_budget_includes_metadata_and_invalid_sink_results_are_hidden() {
        let mut owner = state();
        let connection = connect(&mut owner);
        owner.session.effects.read_result = Ok(ReadResult {
            text: String::from("x"),
            revision: 4,
            truncated: false,
        });
        let key = operation_key(connection.lease(), 1);
        let exact = control_payload(
            key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 91,
                max_lines: 1,
            },
        );
        let payload = owner.handle_control(&connection, &exact, 2).unwrap();
        let ResponseBody::Read {
            text,
            screen_revision,
            ..
        } = response(&payload).body
        else {
            panic!("expected bounded read");
        };
        assert_eq!(text, "x");
        assert_eq!(screen_revision, 4);

        let mut too_small_owner = state();
        let too_small_connection = connect(&mut too_small_owner);
        too_small_owner.session.effects.read_result = Ok(ReadResult {
            text: String::from("x"),
            revision: 4,
            truncated: false,
        });
        let too_small_key = operation_key(too_small_connection.lease(), 1);
        let too_small = control_payload(
            too_small_key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 90,
                max_lines: 1,
            },
        );
        let result = too_small_owner
            .handle_control(&too_small_connection, &too_small, 2)
            .unwrap();
        assert_eq!(
            response(&result).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Failed))
        );

        for bad_result in [
            ReadResult {
                text: String::from("\0"),
                revision: 1,
                truncated: false,
            },
            ReadResult {
                text: String::from("x"),
                revision: 0,
                truncated: false,
            },
            ReadResult {
                text: String::from("a\nb"),
                revision: 1,
                truncated: false,
            },
        ] {
            let mut invalid_owner = state();
            let invalid_connection = connect(&mut invalid_owner);
            invalid_owner.session.effects.read_result = Ok(bad_result);
            let invalid_key = operation_key(invalid_connection.lease(), 1);
            let invalid = control_payload(
                invalid_key,
                target(),
                Command::Read {
                    source: ReadSource::Screen,
                    max_bytes: 256,
                    max_lines: 1,
                },
            );
            let result = invalid_owner
                .handle_control(&invalid_connection, &invalid, 2)
                .unwrap();
            assert_eq!(
                response(&result).body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Failed))
            );
            assert!(!matches!(response(&result).body, ResponseBody::Read { .. }));
        }
    }

    #[test]
    fn old_generation_resume_fails_without_issuing_a_replacement_lease() {
        let mut owner = state();
        let first = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0;
        let bad_resume = SessionHello {
            hello: hello(CAPABILITY_READ, CAPABILITY_INPUT).hello,
            resume: Some(LeaseRef {
                generation: GENERATION - 1,
                lease: first.lease(),
            }),
        };
        assert!(matches!(
            owner.connect(&bad_resume, 2),
            Err(ServerError::UnknownContext)
        ));
        let next = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 2)
            .unwrap()
            .0;
        assert_eq!(next.lease(), first.lease() + 1);
        assert_eq!(owner.global.grants.len(), 2);
    }

    #[test]
    fn live_resume_keeps_lease_and_owner_and_detach_keeps_sink_alive() {
        let mut owner = state();
        let first = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0;
        claim(&mut owner, &first, 1);
        let owner_before = owner.owner_snapshot();
        let resume = SessionHello {
            hello: hello(CAPABILITY_READ, CAPABILITY_INPUT).hello,
            resume: Some(LeaseRef {
                generation: GENERATION,
                lease: first.lease(),
            }),
        };
        let (reattached, welcome) = owner.connect(&resume, 3).unwrap();
        assert_eq!(reattached.lease(), first.lease());
        assert_eq!(welcome.lease, first.lease());
        assert_eq!(owner.owner_snapshot(), owner_before);

        let detached = owner
            .handle_session(&reattached, &session_payload(SessionRequest::Detach), 4)
            .unwrap();
        assert!(matches!(detached, SessionReply::Detached));
        assert_eq!(owner.owner_snapshot(), owner_before);
        assert_eq!(owner.effects().inputs, 0);
    }

    #[test]
    fn grants_are_pinned_to_all_target_identity_fields_before_any_target_access() {
        for field in 0..5 {
            let mut owner = watch_state();
            let connection = connect_with_events(&mut owner);
            let original_grant = owner.global.grants[&connection.lease()];
            let mut other = target_state(another_target(field));

            let resume = SessionHello {
                // Even an otherwise unsupported request must not reveal target
                // capability availability through a cross-target resume.
                hello: hello(u64::MAX, 0).hello,
                resume: Some(LeaseRef {
                    generation: GENERATION,
                    lease: connection.lease(),
                }),
            };
            assert!(matches!(
                owner.global.connect(&other, &resume, 2),
                Err(ServerError::UnknownContext)
            ));
            let unchanged = owner.global.grants[&connection.lease()];
            assert_eq!(unchanged.target, original_grant.target);
            assert_eq!(unchanged.expires_secs, original_grant.expires_secs);

            let mut handler = SessionHandler::new(&mut owner.global, &mut other);
            assert!(matches!(
                handler.handle_session(&connection, &session_payload(SessionRequest::Describe), 2,),
                Err(ServerError::Unauthorized)
            ));
            assert!(matches!(
                handler.reconcile(&connection, another_target(field), 256, 1, 2),
                Err(ServerError::Unauthorized)
            ));
            assert!(matches!(
                handler.read_events(
                    &connection,
                    another_target(field),
                    EventCursor {
                        generation: GENERATION,
                        sequence: 0,
                    },
                    1,
                    2,
                ),
                Err(ServerError::Unauthorized)
            ));

            // A protocol request's target generation must match its operation
            // key generation, so the different-generation session is rejected
            // as stale before scope; the remaining four fields reach scope.
            if field != 0 {
                let control_key = operation_key(connection.lease(), 41);
                let read = control_payload(
                    control_key,
                    another_target(field),
                    Command::Read {
                        source: ReadSource::Screen,
                        max_bytes: 256,
                        max_lines: 1,
                    },
                );
                assert_eq!(
                    rejection(&handler.handle_control(&connection, &read, 2).unwrap()),
                    RejectionCode::Unauthorized
                );
                assert_eq!(
                    handler.global.ledger.status(control_key, 2).unwrap(),
                    OperationStatus::NotAdmitted
                );

                // Even valid input ownership in another selected session
                // cannot replace the lease's immutable target authority.
                handler
                    .session
                    .input_owner
                    .claim(Owner::Client(connection.lease()), 1, 30, 2)
                    .unwrap();
                let input_key = operation_key(connection.lease(), 43);
                let input_owner_before = handler.session.input_owner.snapshot();
                let submit = control_payload(
                    input_key,
                    another_target(field),
                    Command::Submit {
                        owner_revision: input_owner_before.revision,
                        text: "scope-denied",
                    },
                );
                assert_eq!(
                    rejection(&handler.handle_control(&connection, &submit, 2).unwrap()),
                    RejectionCode::Unauthorized
                );
                assert_eq!(handler.session.input_owner.snapshot(), input_owner_before);
                assert_eq!(
                    handler.global.ledger.status(input_key, 2).unwrap(),
                    OperationStatus::NotAdmitted
                );
            }

            if field != 0 {
                let owner_key = operation_key(connection.lease(), 42);
                let claim = session_payload(SessionRequest::Claim {
                    key: owner_key,
                    target: another_target(field),
                    expected_owner_revision: 1,
                    ttl_seconds: 30,
                });
                let before = handler.session.input_owner.snapshot();
                let SessionReply::Payload(reply) =
                    handler.handle_session(&connection, &claim, 2).unwrap()
                else {
                    panic!("owner mutation must return an operation payload");
                };
                assert_eq!(rejection(&reply), RejectionCode::Unauthorized);
                assert_eq!(handler.session.input_owner.snapshot(), before);
                assert_eq!(
                    handler.global.ledger.status(owner_key, 2).unwrap(),
                    OperationStatus::NotAdmitted
                );
            }
            assert_eq!(handler.session.effects.validations.get(), 0);
            assert_eq!(handler.session.effects.reads, 0);
            assert_eq!(handler.session.effects.inputs, 0);
            assert_eq!(handler.session.effects.polls, 0);
            let final_grant = handler.global.grants[&connection.lease()];
            assert_eq!(final_grant.target, original_grant.target);
            assert_eq!(final_grant.expires_secs, original_grant.expires_secs);
        }
    }

    #[test]
    fn global_exact_results_and_conflicts_precede_selected_target_checks() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let key = operation_key(connection.lease(), 51);
        let payload = control_payload(
            key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        let first = owner.handle_control(&connection, &payload, 2).unwrap();
        assert!(matches!(response(&first).body, ResponseBody::Read { .. }));
        let mut other = target_state(another_target(4));
        let mut handler = SessionHandler::new(&mut owner.global, &mut other);

        let exact = handler.handle_control(&connection, &payload, 2).unwrap();
        assert_eq!(
            response(&exact).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(handler.session.effects.reads, 0);

        let conflict = control_payload(
            key,
            handler.session.target,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 257,
                max_lines: 1,
            },
        );
        assert!(matches!(
            handler.handle_control(&connection, &conflict, 2),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));
        assert_eq!(handler.session.effects.reads, 0);

        let stale_key = operation_key(connection.lease(), 52);
        let stale = control_payload(
            stale_key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert_eq!(
            rejection(&handler.handle_control(&connection, &stale, 2).unwrap()),
            RejectionCode::StaleTarget
        );
        assert_eq!(
            handler.global.ledger.status(stale_key, 2).unwrap(),
            OperationStatus::NotAdmitted
        );
    }

    #[test]
    fn global_preflight_is_read_only_and_renders_known_results_without_a_target() {
        let mut owner = state();
        let connection = connect(&mut owner);
        let control_key = operation_key(connection.lease(), 61);
        let control = control_payload(
            control_key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        let decoded_control = decode_request_payload(&control).unwrap();
        let usage_before = owner.global.ledger.usage();
        assert_eq!(
            owner
                .global
                .preflight_control(&connection, &decoded_control, &control, 2)
                .unwrap(),
            RequestPreflight::Continue
        );
        assert_eq!(owner.global.ledger.usage(), usage_before);
        assert_eq!(
            owner.global.ledger.status(control_key, 2).unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(
            preflight_reply(control_key, RequestPreflight::Continue,).unwrap(),
            None
        );

        assert_eq!(
            owner.global.ledger.admit(control_key, &control, 2),
            Ok(Admission::Execute)
        );
        owner
            .global
            .ledger
            .complete(control_key, Outcome::Applied)
            .unwrap();
        let usage_after_admission = owner.global.ledger.usage();
        let known = owner
            .global
            .preflight_control(&connection, &decoded_control, &control, 2)
            .unwrap();
        assert_eq!(
            known,
            RequestPreflight::Known(OperationStatus::Completed(Outcome::Applied))
        );
        let Some(reply) = preflight_reply(control_key, known).unwrap() else {
            panic!("known global operation must render a status reply");
        };
        assert_eq!(
            response(&reply).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.global.ledger.usage(), usage_after_admission);

        let conflict = control_payload(
            control_key,
            target(),
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 257,
                max_lines: 1,
            },
        );
        let decoded_conflict = decode_request_payload(&conflict).unwrap();
        assert!(matches!(
            owner
                .global
                .preflight_control(&connection, &decoded_conflict, &conflict, 2),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));

        let owner_key = operation_key(connection.lease(), 62);
        let owner_payload = session_payload(SessionRequest::Claim {
            key: owner_key,
            target: target(),
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        let usage_before_owner = owner.global.ledger.usage();
        assert_eq!(
            owner
                .global
                .preflight_owner_mutation(&connection, owner_key, &owner_payload, 2)
                .unwrap(),
            RequestPreflight::Continue
        );
        assert_eq!(owner.global.ledger.usage(), usage_before_owner);
        assert_eq!(
            owner.global.ledger.status(owner_key, 2).unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(
            owner.global.ledger.admit(owner_key, &owner_payload, 2),
            Ok(Admission::Execute)
        );
        owner
            .global
            .ledger
            .complete(owner_key, Outcome::Applied)
            .unwrap();
        let known_owner = owner
            .global
            .preflight_owner_mutation(&connection, owner_key, &owner_payload, 2)
            .unwrap();
        assert_eq!(
            known_owner,
            RequestPreflight::Known(OperationStatus::Completed(Outcome::Applied))
        );
        assert!(preflight_reply(owner_key, known_owner).unwrap().is_some());

        let missing_authority = Connection {
            generation: GENERATION,
            lease: connection.lease() + 100,
            selected_capabilities: connection.selected_capabilities,
        };
        let usage_before_missing = owner.global.ledger.usage();
        assert!(matches!(
            owner
                .global
                .preflight_control(&missing_authority, &decoded_control, &control, 2,),
            Err(ServerError::UnknownContext)
        ));
        assert!(matches!(
            owner
                .global
                .preflight_owner_mutation(&missing_authority, owner_key, &owner_payload, 2,),
            Err(ServerError::UnknownContext)
        ));
        assert_eq!(owner.global.ledger.usage(), usage_before_missing);
        assert_eq!(owner.effects().validations.get(), 0);
        assert_eq!(owner.effects().reads, 0);
        assert_eq!(owner.effects().inputs, 0);
    }

    #[test]
    fn input_handoff_rejects_a_live_recipient_pinned_to_another_target() {
        let mut owner = state();
        let caller = connect(&mut owner);
        let target_b = another_target(3);
        let recipient = owner
            .global
            .connect(
                &target_state(target_b),
                &hello(CAPABILITY_READ, CAPABILITY_INPUT),
                1,
            )
            .unwrap()
            .0;
        let revision = claim(&mut owner, &caller, 1);
        let key = operation_key(caller.lease(), 2);
        let request = session_payload(SessionRequest::Handoff {
            key,
            target: target(),
            expected_owner_revision: revision,
            recipient_lease: recipient.lease(),
            ttl_seconds: 30,
        });
        let before = owner.owner_snapshot();
        let SessionReply::Payload(reply) = owner.handle_session(&caller, &request, 3).unwrap()
        else {
            panic!("handoff must return an operation payload");
        };
        assert_eq!(rejection(&reply), RejectionCode::Unauthorized);
        assert_eq!(owner.owner_snapshot(), before);
        assert_eq!(
            owner.global.ledger.status(key, 3).unwrap(),
            OperationStatus::NotAdmitted
        );
    }

    #[test]
    fn describe_reports_only_client_or_vacant_owner_and_never_casts_human() {
        let mut owner = state();
        let connection = connect(&mut owner);
        owner.session.effects.child_exited = true;
        let reply = owner
            .handle_session(&connection, &session_payload(SessionRequest::Describe), 2)
            .unwrap();
        let SessionReply::Info(info) = reply else {
            panic!("Describe must return session info");
        };
        assert_eq!(info.target, target());
        assert_eq!(info.owner_revision, 1);
        assert_eq!(info.owner_lease, 0);
        assert_eq!(info.owner_expires_secs, 0);
        assert!(info.child_exited);

        owner
            .session
            .input_owner
            .claim(Owner::Client(connection.lease()), 1, 20, 2)
            .unwrap();
        owner
            .session
            .input_owner
            .handoff(
                Owner::Client(connection.lease()),
                2,
                Owner::Human(99),
                10,
                3,
            )
            .unwrap();
        assert!(matches!(
            owner.handle_session(&connection, &session_payload(SessionRequest::Describe), 4,),
            Err(ServerError::UnsupportedOwner)
        ));
    }

    #[test]
    fn polling_expires_grants_and_owner_but_keeps_a_pending_input_pinned() {
        let mut owner = state();
        let connection = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0;
        owner
            .session
            .input_owner
            .claim(Owner::Client(connection.lease()), 1, 1, 1)
            .unwrap();
        let pending_key = operation_key(connection.lease(), 100);
        owner
            .session
            .input_owner
            .begin_input(
                Owner::Client(connection.lease()),
                owner.owner_snapshot().revision,
                pending_key,
                1,
            )
            .unwrap();
        owner.poll(2).unwrap();
        assert_eq!(owner.owner_snapshot().pending, Some(pending_key));
        assert_eq!(
            owner.owner_snapshot().holder,
            Some(Owner::Client(connection.lease()))
        );

        owner.poll(LEASE_TTL_SECS + 1).unwrap();
        assert!(owner.global.grants.is_empty());
        assert_eq!(owner.global.ledger.usage().active_leases, 0);
        assert_eq!(owner.owner_snapshot().pending, Some(pending_key));
        assert!(matches!(
            owner.handle_session(
                &connection,
                &session_payload(SessionRequest::Detach),
                LEASE_TTL_SECS + 1
            ),
            Err(ServerError::UnknownContext)
        ));
        assert_eq!(owner.effects().polls, 2);
    }

    #[test]
    fn expiry_invalidates_old_connection_and_new_lease_uses_next_id() {
        let mut owner = state();
        let connection = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_INPUT), 1)
            .unwrap()
            .0;
        owner.poll(1 + LEASE_TTL_SECS).unwrap();
        assert!(matches!(
            owner.handle_session(
                &connection,
                &session_payload(SessionRequest::Detach),
                1 + LEASE_TTL_SECS
            ),
            Err(ServerError::UnknownContext)
        ));
        let next = owner
            .connect(
                &hello(CAPABILITY_READ, CAPABILITY_INPUT),
                1 + LEASE_TTL_SECS,
            )
            .unwrap()
            .0;
        assert_eq!(next.lease(), connection.lease() + 1);
    }

    #[test]
    fn handoff_requires_an_active_input_grant_and_never_accepts_unknown_recipients() {
        let mut owner = state();
        let caller = connect(&mut owner);
        let revision = claim(&mut owner, &caller, 1);
        let read_only_recipient = owner.connect(&hello(CAPABILITY_READ, 0), 2).unwrap().0;
        for (recipient, expected) in [
            (read_only_recipient.lease(), RejectionCode::Unauthorized),
            (
                read_only_recipient.lease() + 99,
                RejectionCode::UnknownContext,
            ),
        ] {
            let key = operation_key(
                caller.lease(),
                if recipient == read_only_recipient.lease() {
                    2
                } else {
                    3
                },
            );
            let payload = session_payload(SessionRequest::Handoff {
                key,
                target: target(),
                expected_owner_revision: revision,
                recipient_lease: recipient,
                ttl_seconds: 10,
            });
            let reply = owner.handle_session(&caller, &payload, 3).unwrap();
            let SessionReply::Payload(payload) = reply else {
                panic!("handoff must return a rejection payload");
            };
            assert_eq!(rejection(&payload), expected);
            assert_eq!(
                operation_status(&mut owner, &caller, key, 3),
                OperationStatus::NotAdmitted
            );
        }
        assert_eq!(
            owner.owner_snapshot().holder,
            Some(Owner::Client(caller.lease()))
        );
    }

    #[test]
    fn read_only_connection_can_observe_but_cannot_mutate_owner() {
        let mut owner = state();
        let read_only = owner.connect(&hello(CAPABILITY_READ, 0), 1).unwrap().0;
        let key = operation_key(read_only.lease(), 1);
        let payload = session_payload(SessionRequest::Claim {
            key,
            target: target(),
            expected_owner_revision: 1,
            ttl_seconds: 10,
        });
        let reply = owner.handle_session(&read_only, &payload, 2).unwrap();
        let SessionReply::Payload(payload) = reply else {
            panic!("claim must return a rejection payload");
        };
        assert_eq!(rejection(&payload), RejectionCode::Unauthorized);
        assert_eq!(owner.owner_snapshot().holder, None);
    }

    #[test]
    fn lease_deadline_overflow_does_not_consume_a_lease_id() {
        let mut owner = state();
        assert!(matches!(
            owner.connect(&hello(CAPABILITY_READ, 0), u64::MAX - 10),
            Err(ServerError::Protocol(ProtocolError::TimeOverflow))
        ));
        let connection = owner.connect(&hello(CAPABILITY_READ, 0), 0).unwrap().0;
        assert_eq!(connection.lease(), 1);
    }

    #[test]
    fn expiry_and_active_lease_limits_are_bounded_and_fail_closed() {
        let mut owner = state();
        let mut last = None;
        for _ in 0..MAX_ACTIVE_LEASES {
            last = Some(owner.connect(&hello(CAPABILITY_READ, 0), 1).unwrap().0);
        }
        assert_eq!(owner.global.grants.len(), MAX_ACTIVE_LEASES);
        assert!(matches!(
            owner.connect(&hello(CAPABILITY_READ, 0), 1),
            Err(ServerError::Protocol(
                ProtocolError::ActiveLeaseCapacityExceeded
            ))
        ));
        owner.poll(1 + LEASE_TTL_SECS).unwrap();
        assert!(owner.global.grants.is_empty());
        let next = owner
            .connect(&hello(CAPABILITY_READ, 0), 1 + LEASE_TTL_SECS)
            .unwrap()
            .0;
        assert_eq!(next.lease(), last.unwrap().lease() + 1);
    }

    #[test]
    fn events_require_healthy_metadata_and_resume_cannot_escalate_the_grant() {
        let mut unavailable = state();
        let no_events = unavailable
            .connect(&hello(CAPABILITY_READ, CAPABILITY_EVENTS), 1)
            .unwrap()
            .0;
        assert_eq!(no_events.selected_capabilities(), CAPABILITY_READ);

        let mut owner = watch_state();
        let connection = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_EVENTS), 1)
            .unwrap()
            .0;
        assert_eq!(
            connection.selected_capabilities(),
            CAPABILITY_READ | CAPABILITY_EVENTS
        );

        owner.effects_mut().screen_revision = None;
        owner.poll(2).unwrap();
        let mut resume = hello(CAPABILITY_READ, CAPABILITY_EVENTS);
        resume.resume = Some(LeaseRef {
            generation: GENERATION,
            lease: connection.lease(),
        });
        let (resumed, _) = owner.connect(&resume, 3).unwrap();
        assert_eq!(resumed.lease(), connection.lease());
        assert_eq!(resumed.selected_capabilities(), CAPABILITY_READ);

        let mut require_events = hello(CAPABILITY_READ | CAPABILITY_EVENTS, 0);
        require_events.resume = resume.resume;
        assert!(matches!(
            owner.connect(&require_events, 4),
            Err(ServerError::Handshake(_))
        ));
        let fresh = owner
            .connect(&hello(CAPABILITY_READ, CAPABILITY_EVENTS), 4)
            .unwrap()
            .0;
        assert_eq!(fresh.selected_capabilities(), CAPABILITY_READ);
    }

    #[test]
    fn legacy_single_session_owner_does_not_offer_multi_session_wrap_capability() {
        use crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS;

        let mut owner = state();
        let requested = hello(CAPABILITY_READ, CAPABILITY_SCREEN_ROW_WRAPS);
        let (connection, welcome) = owner.connect(&requested, 1).unwrap();
        assert_eq!(welcome.selected_capabilities, CAPABILITY_READ);
        assert_eq!(connection.selected_capabilities(), CAPABILITY_READ);

        let mut resume = requested;
        resume.resume = Some(LeaseRef {
            generation: GENERATION,
            lease: connection.lease(),
        });
        let (_, resumed) = owner.connect(&resume, 2).unwrap();
        assert_eq!(resumed.selected_capabilities, CAPABILITY_READ);
    }

    #[test]
    fn reconciliation_binds_screen_owner_exit_and_cursor_in_one_turn() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let initial = owner
            .read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 0,
                },
                64,
                1,
            )
            .unwrap();
        assert!(initial.events.is_empty());
        assert_eq!(initial.cursor.sequence, 0);

        let owner_revision = claim(&mut owner, &connection, 1);
        owner.effects_mut().child_exited = true;
        owner.effects_mut().read_result = Ok(ReadResult {
            text: String::from("sensitive-screen-sample"),
            revision: 1,
            truncated: false,
        });
        let state = owner
            .reconcile(&connection, target(), 4_096, 10, 3)
            .unwrap();
        assert_eq!(state.screen_revision, 1);
        assert_eq!(state.info.owner_revision, owner_revision);
        assert_eq!(state.info.owner_lease, connection.lease());
        assert!(state.info.child_exited);
        assert_eq!(state.cursor.sequence, 2);
        assert_eq!(state.text, "sensitive-screen-sample");
        let debug = format!("{state:?}");
        assert!(!debug.contains("sensitive-screen-sample"));
        assert!(!debug.contains(&connection.lease().to_string()));

        let page = owner
            .read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 0,
                },
                64,
                3,
            )
            .unwrap();
        assert_eq!(page.cursor, state.cursor);
        assert_eq!(page.events.len(), 2);
        assert_eq!(
            page.events[0].body,
            EventBody::InputOwnerChanged {
                revision: owner_revision,
                owner_lease: connection.lease(),
            }
        );
        assert_eq!(page.events[1].body, EventBody::ChildExited);
    }

    #[test]
    fn poll_records_screen_owner_expiry_and_exit_in_contract_order() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let owner_revision = claim(&mut owner, &connection, 1);
        assert_eq!(owner_revision, 2);

        owner.effects_mut().screen_revision = Some(2);
        owner.effects_mut().child_exited = true;
        owner.poll(32).unwrap();
        assert_eq!(owner.owner_snapshot().holder, None);

        let page = owner
            .read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 1,
                },
                64,
                32,
            )
            .unwrap();
        assert_eq!(page.events.len(), 3);
        assert_eq!(
            page.events[0].body,
            EventBody::ScreenChanged { revision: 2 }
        );
        assert_eq!(
            page.events[1].body,
            EventBody::InputOwnerChanged {
                revision: 3,
                owner_lease: 0,
            }
        );
        assert_eq!(page.events[2].body, EventBody::ChildExited);
    }

    #[test]
    fn duplicate_and_rejected_owner_requests_do_not_add_events() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let claim_payload = session_payload(SessionRequest::Claim {
            key: operation_key(connection.lease(), 1),
            target: target(),
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        owner
            .handle_session(&connection, &claim_payload, 2)
            .unwrap();
        owner
            .handle_session(&connection, &claim_payload, 3)
            .unwrap();

        let rejected_payload = session_payload(SessionRequest::Claim {
            key: operation_key(connection.lease(), 2),
            target: target(),
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        let reply = owner
            .handle_session(&connection, &rejected_payload, 3)
            .unwrap();
        let SessionReply::Payload(payload) = reply else {
            panic!("stale claim must return a rejection");
        };
        assert_eq!(rejection(&payload), RejectionCode::StaleOwner);

        let page = owner
            .read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 1,
                },
                64,
                3,
            )
            .unwrap();
        assert!(page.events.is_empty());
        assert_eq!(page.cursor.sequence, 1);
    }

    #[test]
    fn event_reads_reject_bad_identity_and_return_resync_for_expired_cursor() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let cursor = EventCursor {
            generation: GENERATION,
            sequence: 0,
        };
        assert_eq!(
            owner.read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION + 1,
                    sequence: 0,
                },
                64,
                1,
            ),
            Err(ServerError::WrongGeneration)
        );
        assert_eq!(
            owner.read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 1,
                },
                64,
                1,
            ),
            Err(ServerError::CursorAhead)
        );
        let mut stale_target = target();
        stale_target.occupant_revision += 1;
        assert_eq!(
            owner.read_events(&connection, stale_target, cursor, 64, 1),
            Err(ServerError::TargetStale)
        );

        owner.effects_mut().screen_revision = Some(2);
        let observed = owner
            .read_events(&connection, target(), cursor, 64, 2)
            .unwrap();
        assert_eq!(observed.events.len(), 1);
        assert_eq!(
            observed.events[0].body,
            EventBody::ScreenChanged { revision: 2 }
        );

        for revision in 3..=67 {
            owner.effects_mut().screen_revision = Some(revision);
            owner.poll(2).unwrap();
        }
        let expired = owner
            .read_events(&connection, target(), cursor, 64, 2)
            .unwrap();
        assert!(expired.events.is_empty());
        assert!(expired.resync_required);
        assert_eq!(expired.cursor.sequence, 66);
    }

    #[test]
    fn reconciliation_rejects_invalid_budgets_text_and_revision_races() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let reads_before = owner.effects().reads;
        assert_eq!(
            owner.reconcile(&connection, target(), 127, 1, 2),
            Err(ServerError::InvalidWatchRequest)
        );
        assert_eq!(
            owner.reconcile(&connection, target(), 1_024, 0, 2),
            Err(ServerError::InvalidWatchRequest)
        );
        assert_eq!(owner.effects().reads, reads_before);

        assert_eq!(
            owner.reconcile(&connection, target(), 128, 1, 2),
            Err(ServerError::InvalidSnapshot)
        );
        owner.effects_mut().read_result = Ok(ReadResult {
            text: String::from("bad\u{1b}text"),
            revision: 1,
            truncated: false,
        });
        assert_eq!(
            owner.reconcile(&connection, target(), 1_024, 10, 2),
            Err(ServerError::InvalidSnapshot)
        );
        owner.effects_mut().read_result = Ok(ReadResult {
            text: String::from("one\ntwo"),
            revision: 1,
            truncated: false,
        });
        assert_eq!(
            owner.reconcile(&connection, target(), 1_024, 1, 2),
            Err(ServerError::InvalidSnapshot)
        );

        owner.effects_mut().read_result = Ok(ReadResult {
            text: String::from("screen"),
            revision: 1,
            truncated: false,
        });
        owner.effects_mut().screen_revision_after_read = Some(2);
        assert_eq!(
            owner.reconcile(&connection, target(), 1_024, 10, 2),
            Err(ServerError::SnapshotChanged)
        );
        assert_eq!(owner.session.journal.current_cursor().sequence, 1);

        owner.effects_mut().read_result = Ok(ReadResult {
            text: String::from("screen"),
            revision: 2,
            truncated: false,
        });
        let state = owner
            .reconcile(&connection, target(), 1_024, 10, 3)
            .unwrap();
        assert_eq!(state.screen_revision, 2);
        assert_eq!(state.cursor.sequence, 1);
    }

    #[test]
    fn metadata_failures_disable_events_without_stopping_child_or_input() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let owner_revision = claim(&mut owner, &connection, 1);
        owner.effects_mut().screen_revision = None;
        assert_eq!(owner.poll(3), Ok(()));
        assert!(!owner.effects().child_exited());
        assert_eq!(
            owner.owner_snapshot().holder,
            Some(Owner::Client(connection.lease()))
        );
        assert_eq!(
            owner.read_events(
                &connection,
                target(),
                EventCursor {
                    generation: GENERATION,
                    sequence: 1,
                },
                64,
                3,
            ),
            Err(ServerError::NotificationsUnavailable)
        );

        let payload = control_payload(
            operation_key(connection.lease(), 2),
            target(),
            Command::Submit {
                owner_revision,
                text: "next",
            },
        );
        let response_payload = owner.handle_control(&connection, &payload, 4).unwrap();
        assert_eq!(
            response(&response_payload).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.effects().inputs, 1);
        assert!(!owner.effects().child_exited());
    }

    #[test]
    fn exhausted_journal_does_not_reclassify_an_applied_input() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        let owner_revision = claim(&mut owner, &connection, 1);
        owner.session.journal.exhaust_sequence_for_test();
        owner.effects_mut().screen_revision = Some(2);

        let payload = control_payload(
            operation_key(connection.lease(), 2),
            target(),
            Command::Submit {
                owner_revision,
                text: "next",
            },
        );
        let response_payload = owner.handle_control(&connection, &payload, 3).unwrap();
        assert_eq!(
            response(&response_payload).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.effects().inputs, 1);
        assert_eq!(
            owner.owner_snapshot().holder,
            Some(Owner::Client(connection.lease()))
        );
        assert!(!owner.effects().child_exited());
        assert!(!owner.session.notifications_available);
    }

    #[test]
    fn human_owner_disables_metadata_events_without_fabricating_vacancy() {
        let mut owner = watch_state();
        let connection = connect_with_events(&mut owner);
        assert_eq!(
            connection.selected_capabilities(),
            CAPABILITY_READ | CAPABILITY_INPUT | CAPABILITY_EVENTS
        );
        owner
            .session
            .input_owner
            .claim(Owner::Human(99), 1, 10, 1)
            .unwrap();
        owner.poll(2).unwrap();
        assert!(!owner.session.notifications_available);
        assert_eq!(owner.session.journal.current_cursor().sequence, 0);
    }
}
