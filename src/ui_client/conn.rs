//! One authenticated, target-pinned connection plus its durable client
//! context file. Every keyed operation reserves its key durably before it is
//! sent and is acknowledged only after the exact response validates.
//!
//! Errors are fixed `&'static str` labels: no target, key, env or input data.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panelink::client_context::{ContextError, ContextFile};
use panelink::control_protocol::{encode_request_payload, Command, ControlRequest, TargetRef};
use panelink::event_protocol::{decode_event, EventBody};
use panelink::handshake_protocol::{
    decode_welcome, validate_welcome, Hello, Welcome, CAPABILITY_EVENTS, CAPABILITY_HISTORY_CURSOR,
    CAPABILITY_INPUT, CAPABILITY_PANE_OPERATIONS, CAPABILITY_READ, CAPABILITY_SCOPED_MOUSE_WHEEL,
    CAPABILITY_SCREEN_ROW_WRAPS,
};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::scoped_watch_protocol::{
    decode_watch_end, encode_watch_request, SessionCursor, WatchRequest,
};
use panelink::session_protocol::{
    decode_session_info, encode_session_hello, encode_session_request, LeaseRef, SessionHello,
    SessionInfo, SessionRequest,
};
use panelink::terminal::Key;
use panelink::transport::{AuthenticatedStream, OwnedFrame};
use panelink::ui_protocol::{
    encode_attach_target, encode_key_input, encode_mouse_wheel_input,
    encode_scoped_mouse_wheel_input, KeyInput, MouseWheelInput, ScopedMouseWheelInput,
};
use panelink::watch_protocol::WatchStatus;

/// Deadline for exchanges made before the host terminal is taken over.
pub(crate) const DEADLINE: Duration = Duration::from_secs(2);
/// Deadline for each exchange inside the interactive loop, so a stalled
/// server cannot hold the loop (and Ctrl-X/signal handling) for long.
pub(crate) const OP_DEADLINE: Duration = Duration::from_millis(400);
/// Longer deadline for the split request (the server spawns a child).
pub(crate) const SPLIT_DEADLINE: Duration = Duration::from_secs(2);
/// Deadline for an explicit close: the server's bounded stop may take a 1 s
/// HUP window plus a 1 s KILL window before it replies.
pub(crate) const CLOSE_DEADLINE: Duration = Duration::from_secs(4);
/// Bound for each detach-time exchange (release, settle, receipt).
pub(crate) const DETACH_DEADLINE: Duration = Duration::from_millis(500);
/// Server lease lifetime (`protocol::LEASE_TTL_SECS`); only a resume Hello
/// renews it, ordinary requests do not.
pub(crate) const LEASE_TTL: Duration = Duration::from_secs(panelink::protocol::LEASE_TTL_SECS);
/// Renew this long before the conservative local expiry estimate.
pub(crate) const RENEW_MARGIN: Duration = Duration::from_secs(300);
/// Retry interval after a failed (non-fatal) renewal attempt.
pub(crate) const RENEW_RETRY: Duration = Duration::from_secs(15);
const OWNER_TTL_SECONDS: u32 = 30;
const WATCH_LIMIT: u16 = 16;
const MAX_CONTEXT_CANDIDATES: usize = 8;
const RECEIPT_EVERY: u64 = 32;
const RESUME_STATUS_PROBES: usize = 4;

static NONCE: AtomicU64 = AtomicU64::new(1);

/// When to proactively resume (renew) one connection's lease.
///
/// `renewed_at` is taken *before* the Hello is sent, so it is never later
/// than the server's own expiry base: the local estimate expires no later
/// than the server's lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LeaseSchedule {
    renewed_at: Instant,
    next_attempt: Instant,
}

/// What the lease schedule asks for at `now`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeaseAction {
    Wait,
    Renew,
}

impl LeaseSchedule {
    pub(crate) fn new(renewed_at: Instant) -> Self {
        Self {
            renewed_at,
            next_attempt: renewed_at + (LEASE_TTL - RENEW_MARGIN),
        }
    }

    pub(crate) fn action(&self, now: Instant) -> LeaseAction {
        if now >= self.next_attempt {
            LeaseAction::Renew
        } else {
            LeaseAction::Wait
        }
    }

    /// A resume Hello started at `started` succeeded.
    pub(crate) fn renewed(&mut self, started: Instant) {
        *self = Self::new(started);
    }

    /// A renewal attempt failed without proving the lease dead.
    pub(crate) fn failed(&mut self, now: Instant) {
        self.next_attempt = now + RENEW_RETRY;
    }

    /// Whether the conservative local estimate says the lease has expired.
    pub(crate) fn past_ttl(&self, now: Instant) -> bool {
        now >= self.renewed_at + LEASE_TTL
    }
}

/// Server error label for a resume Hello naming an unknown/expired lease.
pub(crate) const LEASE_UNKNOWN: &str = "lease unknown or expired";

/// One reply frame to a request.
pub(crate) enum Reply {
    Response(Vec<u8>),
    Snapshot(Vec<u8>),
    /// Server error label mapped to a fixed local label.
    Error(&'static str),
}

/// Outcome of a typed-input or paste operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputResult {
    Applied,
    NotOwner,
    Failed(&'static str),
}

/// Result of one non-replayable mouse-wheel input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MouseInputResult {
    Applied,
    NotOwner,
    /// The server confirmed before admission that the child disabled mouse input.
    Unsupported,
    Failed(&'static str),
}

/// A pane connection: pinned stream, context lock and ownership view.
pub(crate) struct PaneConn {
    pub(crate) target: TargetRef,
    context: ContextFile,
    stream: Option<AuthenticatedStream>,
    next_request: u64,
    pub(crate) events: bool,
    /// Schema-13 pane operations selected on the current lease connection.
    pub(crate) pane_operations: bool,
    /// Negotiated only by multi-session leases that were originally granted it.
    pub(crate) screen_row_wraps: bool,
    /// Negotiated only by multi-session leases that were originally granted it.
    pub(crate) history_cursor: bool,
    /// Optional vacancy-scoped wheel capability selected by this lease.
    pub(crate) scoped_mouse_wheel: bool,
    input_capability: bool,
    pub(crate) cursor: u64,
    /// Our input-owner revision when we hold the claim.
    pub(crate) owner_revision: Option<u64>,
    pub(crate) lease_schedule: LeaseSchedule,
    pub(crate) next_retry: Instant,
    retry_backoff: Duration,
    pub(crate) needs_screen: bool,
    pub(crate) next_watch: Instant,
    deadline: Duration,
    /// Highest sequence whose terminal *admitted* result this process
    /// validated (a ledger status, resize or split result).
    last_admitted: u64,
    /// Highest sequence whose receipt the server confirmed.
    receipted: u64,
    /// The server reported this lease unknown/expired: the context is dead
    /// and the pane must be re-attached (which replaces a settled context).
    pub(crate) lease_dead: bool,
}

impl PaneConn {
    pub(crate) fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub(crate) fn lease(&self) -> u64 {
        self.context.context().lease()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.context.context().generation()
    }

    pub(crate) fn disconnect(&mut self, now: Instant) {
        self.stream = None;
        self.owner_revision = None;
        self.next_retry = now + self.retry_backoff;
        self.retry_backoff = (self.retry_backoff * 2).min(Duration::from_secs(8));
    }

    /// Bound every later exchange on this connection.
    pub(crate) fn set_deadline(&mut self, deadline: Duration) {
        self.deadline = deadline;
    }

    /// Attach to `target` with a context file under `dir`.
    ///
    /// `bootstrap` is an already-handshaken fresh stream pinned to `target`
    /// (from a trailer-less Hello) plus the last request ID it used and when
    /// its Hello started; its lease becomes this pane's context when a new
    /// context file is created, so discovery never leaks a lease. Existing
    /// context files are resumed and never overwritten; a locked one is
    /// skipped for the next candidate name, and a provably dead, settled one
    /// is replaced.
    pub(crate) fn attach(
        directory: &Path,
        dir: &Path,
        target: TargetRef,
        mut bootstrap: Option<Bootstrap>,
        now: Instant,
        deadline: Duration,
    ) -> Result<Self, &'static str> {
        let mut last = "context_unavailable";
        for index in 0..MAX_CONTEXT_CANDIDATES {
            let path = context_path(dir, target, index);
            let exists = match fs::symlink_metadata(&path) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(_) => return Err("context_unavailable"),
            };
            if exists {
                match Self::resume(directory, &path, Some(target), now, deadline) {
                    Ok(pane) => return Ok(pane),
                    // A provably dead, settled context was removed: reuse
                    // its name for a fresh lease below.
                    Err(ResumeError::Removed) => {}
                    Err(ResumeError::Other(error)) => {
                        last = error;
                        continue;
                    }
                }
            }
            let (stream, welcome, first_request_id, started) = match bootstrap.take() {
                Some(boot) => (boot.stream, boot.welcome, boot.used + 1, boot.started),
                None => {
                    let started = Instant::now();
                    let mut stream = connect(directory, deadline)?;
                    let welcome = handshake(&mut stream, None, Some(target), deadline)?;
                    (stream, welcome, 1, started)
                }
            };
            let context = match ContextFile::create(
                &path,
                welcome.generation,
                welcome.lease,
                Instant::now() + DEADLINE,
            ) {
                Ok(context) => context,
                // Only a proven pre-creation collision (a concurrent client
                // created this exact name first, O_EXCL -> EEXIST) is safe to
                // retry: nothing was written for this lease. Any later failure
                // may have left a durable record for this lease, so reusing
                // the lease under another name could duplicate its allocator.
                Err(ContextError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                    drop(error);
                    last = "context in use by another client";
                    bootstrap = Some(Bootstrap {
                        stream,
                        welcome,
                        used: first_request_id - 1,
                        started,
                    });
                    continue;
                }
                Err(error) => return Err(context_label(error)),
            };
            let mut pane = Self::new(target, context, stream, welcome, started.min(now));
            pane.deadline = deadline;
            // The bootstrap stream may already have used request IDs;
            // the server requires strictly increasing IDs per stream.
            pane.next_request = first_request_id;
            return Ok(pane);
        }
        Err(last)
    }

    /// Resume an existing context file for `target`.
    ///
    /// A context whose lease the server reports unknown (expired, or from an
    /// older generation) is provably dead. When it holds no unresolved
    /// operation it is removed (while still locked) so its name can be
    /// reused; a dead context with an unresolved operation is kept.
    pub(crate) fn resume(
        directory: &Path,
        path: &Path,
        expected: Option<TargetRef>,
        now: Instant,
        deadline: Duration,
    ) -> Result<Self, ResumeError> {
        let context = ContextFile::open_existing(path, Instant::now() + Duration::from_millis(100))
            .map_err(|error| ResumeError::Other(context_label(error)))?;
        let started = Instant::now();
        let mut stream = connect(directory, deadline).map_err(ResumeError::Other)?;
        let state = context.context();
        let resume = Some((state.generation(), state.lease()));
        let welcome = match handshake(&mut stream, resume, None, deadline) {
            Ok(welcome) => welcome,
            Err(LEASE_UNKNOWN) if state.uncertain().is_none() => {
                return match fs::remove_file(path) {
                    Ok(()) => Err(ResumeError::Removed),
                    Err(_) => Err(ResumeError::Other(LEASE_UNKNOWN)),
                };
            }
            Err(error) => return Err(ResumeError::Other(error)),
        };
        // The pinned target is learned from the resumed grant itself.
        let placeholder = expected.unwrap_or(TargetRef {
            generation: state.generation(),
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        });
        let mut pane = Self::new(placeholder, context, stream, welcome, started.min(now));
        pane.deadline = deadline;
        // Resolve an earlier uncertain operation by status; never resend it.
        pane.settle_uncertain().map_err(ResumeError::Other)?;
        let info = pane.describe().map_err(ResumeError::Other)?;
        match expected {
            // The resumed grant must still be pinned to this exact target.
            Some(target) if info.target != target => {
                return Err(ResumeError::Other("context_target_mismatch"))
            }
            _ => pane.target = info.target,
        }
        // Retire results left by earlier attachments of this context.
        pane.receipt_all_settled();
        Ok(pane)
    }

    /// Reconnect (or proactively renew) by resuming this context's lease on a
    /// new stream. The old stream is replaced only after the new handshake
    /// succeeded. Nothing is resent; an uncertain key is settled by status.
    pub(crate) fn reconnect(&mut self, directory: &Path, now: Instant) -> Result<(), &'static str> {
        let started = Instant::now().min(now);
        let deadline = self.deadline;
        let mut stream = connect(directory, deadline)?;
        let state = self.context.context();
        let resume = Some((state.generation(), state.lease()));
        let welcome = match handshake(&mut stream, resume, None, deadline) {
            Ok(welcome) => welcome,
            Err(LEASE_UNKNOWN) => {
                self.lease_dead = true;
                return Err(LEASE_UNKNOWN);
            }
            Err(error) => return Err(error),
        };
        self.events = welcome.selected_capabilities & CAPABILITY_EVENTS != 0;
        self.screen_row_wraps = welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS != 0;
        self.history_cursor = welcome.selected_capabilities & CAPABILITY_HISTORY_CURSOR != 0;
        self.pane_operations = welcome.selected_capabilities & CAPABILITY_PANE_OPERATIONS != 0;
        self.scoped_mouse_wheel =
            welcome.selected_capabilities & CAPABILITY_SCOPED_MOUSE_WHEEL != 0;
        self.input_capability = welcome.selected_capabilities & CAPABILITY_INPUT != 0;
        self.stream = Some(stream);
        self.next_request = 1;
        self.lease_schedule.renewed(started);
        self.retry_backoff = Duration::from_secs(1);
        self.needs_screen = true;
        self.settle_uncertain()
    }

    fn new(
        target: TargetRef,
        context: ContextFile,
        stream: AuthenticatedStream,
        welcome: Welcome,
        renewed_at: Instant,
    ) -> Self {
        Self {
            target,
            context,
            stream: Some(stream),
            next_request: 1,
            events: welcome.selected_capabilities & CAPABILITY_EVENTS != 0,
            pane_operations: welcome.selected_capabilities & CAPABILITY_PANE_OPERATIONS != 0,
            screen_row_wraps: welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS != 0,
            history_cursor: welcome.selected_capabilities & CAPABILITY_HISTORY_CURSOR != 0,
            scoped_mouse_wheel: welcome.selected_capabilities & CAPABILITY_SCOPED_MOUSE_WHEEL != 0,
            input_capability: welcome.selected_capabilities & CAPABILITY_INPUT != 0,
            cursor: 0,
            owner_revision: None,
            lease_schedule: LeaseSchedule::new(renewed_at),
            next_retry: renewed_at,
            retry_backoff: Duration::from_secs(1),
            needs_screen: true,
            next_watch: renewed_at,
            deadline: DEADLINE,
            last_admitted: 0,
            receipted: 0,
            lease_dead: false,
        }
    }

    /// Send one request and read its single reply frame.
    pub(crate) fn request(&mut self, payload: &[u8]) -> Result<Reply, &'static str> {
        let id = self.next_request;
        let deadline = self.deadline;
        let Some(stream) = self.stream.as_mut() else {
            return Err("pane disconnected");
        };
        self.next_request += 1;
        let result = write(stream, MessageKind::Request, id, payload, deadline)
            .and_then(|()| read(stream, deadline));
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                // A timed-out or broken stream is poisoned; drop it.
                self.stream = None;
                return Err(error);
            }
        };
        if frame.request_id() != id {
            return Err("response_invalid");
        }
        match frame.kind() {
            MessageKind::Response => Ok(Reply::Response(frame.payload().to_vec())),
            MessageKind::Snapshot => Ok(Reply::Snapshot(frame.payload().to_vec())),
            MessageKind::Error => {
                if frame.payload() == b"unknown_context" {
                    self.lease_dead = true;
                }
                Ok(Reply::Error(server_label(frame.payload())))
            }
            _ => Err("response_invalid"),
        }
    }

    pub(crate) fn describe(&mut self) -> Result<SessionInfo, &'static str> {
        let payload =
            encode_session_request(&SessionRequest::Describe).map_err(|_| "request_invalid")?;
        match self.request(&payload)? {
            Reply::Response(bytes) => decode_session_info(&bytes).map_err(|_| "response_invalid"),
            Reply::Error(label) => Err(label),
            Reply::Snapshot(_) => Err("response_invalid"),
        }
    }

    fn candidate(&self) -> OperationKey {
        let state = self.context.context();
        OperationKey {
            generation: state.generation(),
            lease: state.lease(),
            sequence: state.next_sequence(),
        }
    }

    /// Reserve, send and settle one keyed operation. Returns the decoded
    /// response body bytes for the caller to interpret, already acknowledged
    /// when terminal; an Error frame is settled through OperationStatus.
    pub(crate) fn keyed(
        &mut self,
        build: impl FnOnce(OperationKey) -> Result<Vec<u8>, &'static str>,
    ) -> Result<(OperationKey, Reply), &'static str> {
        self.settle_uncertain()?;
        let key = self.candidate();
        let payload = build(key)?;
        let reserved = self.context.reserve().map_err(context_label)?;
        if reserved != key {
            return Err("context_key_mismatch");
        }
        let reply = self.request(&payload);
        // Payloads may hold paste text or launch environment values.
        drop(payload);
        let reply = reply?;
        match &reply {
            Reply::Response(_) | Reply::Snapshot(_) => {}
            Reply::Error(_) => self.settle_uncertain()?,
        }
        Ok((key, reply))
    }

    /// Acknowledge a validated terminal response for `key`.
    pub(crate) fn ack(&mut self, key: OperationKey) -> Result<(), &'static str> {
        self.context.ack(key).map_err(context_label)
    }

    /// Acknowledge `key` whose validated reply proves ledger admission and a
    /// terminal result (a resize or split result, or a completed status).
    pub(crate) fn ack_admitted(&mut self, key: OperationKey) -> Result<(), &'static str> {
        self.ack(key)?;
        self.last_admitted = self.last_admitted.max(key.sequence);
        self.maybe_receipt();
        Ok(())
    }

    /// Query an uncertain key's status and clear it when terminal.
    pub(crate) fn settle_uncertain(&mut self) -> Result<(), &'static str> {
        let Some(key) = self.context.context().uncertain() else {
            return Ok(());
        };
        let payload = encode_session_request(&SessionRequest::OperationStatus { key })
            .map_err(|_| "request_invalid")?;
        let Reply::Response(bytes) = self.request(&payload)? else {
            return Err("operation_uncertain");
        };
        let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
        if response.key != key {
            return Err("response_invalid");
        }
        let ResponseBody::Status(status) = response.body else {
            return Err("operation_uncertain");
        };
        if status == OperationStatus::InProgress {
            return Err("operation_uncertain");
        }
        self.context.reconcile(key, status).map_err(context_label)?;
        if matches!(status, OperationStatus::Completed(_)) {
            self.last_admitted = self.last_admitted.max(key.sequence);
        }
        Ok(())
    }

    /// Decode a schema-1 response for `key`, acknowledging terminal ones.
    fn settle_response(
        &mut self,
        key: OperationKey,
        reply: Reply,
    ) -> Result<ResponseOutcome, &'static str> {
        let bytes = match reply {
            Reply::Response(bytes) => bytes,
            Reply::Error(label) => return Ok(ResponseOutcome::ServerError(label)),
            Reply::Snapshot(_) => return Err("response_invalid"),
        };
        let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
        if response.key != key {
            return Err("response_invalid");
        }
        match response.body {
            ResponseBody::Status(OperationStatus::InProgress) => {
                Ok(ResponseOutcome::Status(OperationStatus::InProgress))
            }
            ResponseBody::Status(status) => {
                if matches!(status, OperationStatus::Completed(_)) {
                    self.ack_admitted(key)?;
                } else {
                    self.ack(key)?;
                }
                Ok(ResponseOutcome::Status(status))
            }
            ResponseBody::Rejected(code) => {
                // Rejections are decided before admission.
                self.ack(key)?;
                Ok(ResponseOutcome::Rejected(code))
            }
            ResponseBody::Read { .. } => Err("response_invalid"),
        }
    }

    /// Acquire input ownership for one input batch. Callers must not retain it
    /// across idle polling or reads.
    pub(crate) fn claim(&mut self, _now: Instant) -> Result<bool, &'static str> {
        self.claim_inner(false)
    }

    /// Wheel input may claim only vacancy, never renew a retained claim.
    pub(crate) fn claim_vacant(&mut self) -> Result<bool, &'static str> {
        if !self.input_capability {
            return Ok(false);
        }
        self.claim_inner(true)
    }

    fn claim_inner(&mut self, vacant_only: bool) -> Result<bool, &'static str> {
        let info = self.describe()?;
        if vacant_only {
            if info.target != self.target {
                return Err("target changed; wheel not sent");
            }
            if info.owner_lease != 0 {
                return Ok(false);
            }
        }
        if info.owner_lease != 0 && info.owner_lease != self.lease() {
            self.owner_revision = None;
            return Ok(false);
        }
        let target = self.target;
        let revision = info.owner_revision;
        let (key, reply) = self.keyed(|key| {
            encode_session_request(&SessionRequest::Claim {
                key,
                target,
                expected_owner_revision: revision,
                ttl_seconds: OWNER_TTL_SECONDS,
            })
            .map_err(|_| "request_invalid")
        })?;
        let outcome = self.settle_response(key, reply)?;
        if outcome != ResponseOutcome::Status(OperationStatus::Completed(Outcome::Applied)) {
            self.owner_revision = None;
            return Ok(false);
        }
        let info = self.describe()?;
        if info.owner_lease == self.lease() {
            self.owner_revision = Some(info.owner_revision);
            Ok(true)
        } else {
            self.owner_revision = None;
            Ok(false)
        }
    }

    /// Release this connection's claim and verify the exact target is vacant.
    /// The locally held revision is retained on any uncertainty or refusal.
    pub(crate) fn release_confirmed(&mut self) -> Result<(), &'static str> {
        self.release_confirmed_inner()
            .map_err(|_| "input delivery confirmed; ownership release unconfirmed")
    }

    fn release_confirmed_inner(&mut self) -> Result<(), &'static str> {
        let revision = self.owner_revision.ok_or("input owner not held")?;
        let target = self.target;
        let (key, reply) = self.keyed(|key| {
            encode_session_request(&SessionRequest::Release {
                key,
                target,
                expected_owner_revision: revision,
            })
            .map_err(|_| "request_invalid")
        })?;
        let outcome = self.settle_response(key, reply)?;
        if outcome != ResponseOutcome::Status(OperationStatus::Completed(Outcome::Applied)) {
            return Err("release refused");
        }
        let info = self.describe()?;
        if info.target != target || info.owner_lease != 0 {
            return Err("release not confirmed");
        }
        self.owner_revision = None;
        Ok(())
    }

    /// Best-effort detach cleanup. Its result is intentionally not used to
    /// infer that the claim was released.
    pub(crate) fn release(&mut self) {
        let _ = self.release_confirmed();
    }

    /// Deliver typed keys once; never resent.
    pub(crate) fn send_keys(&mut self, keys: &[Key]) -> Result<InputResult, &'static str> {
        let Some(revision) = self.owner_revision else {
            return Ok(InputResult::NotOwner);
        };
        let target = self.target;
        let (key, reply) = self.keyed(|key| {
            encode_key_input(&KeyInput {
                key,
                target,
                expected_owner_revision: revision,
                keys,
            })
            .map_err(|_| "request_invalid")
        })?;
        let outcome = self.settle_response(key, reply)?;
        Ok(self.input_result(outcome))
    }

    /// Deliver one typed wheel event. This event is never resent.
    pub(crate) fn send_mouse_wheel(
        &mut self,
        column: u16,
        row: u16,
        up: bool,
    ) -> Result<MouseInputResult, &'static str> {
        let Some(revision) = self.owner_revision else {
            return Ok(MouseInputResult::NotOwner);
        };
        let target = self.target;
        let (key, reply) = self.keyed(|key| {
            encode_mouse_wheel_input(&MouseWheelInput {
                key,
                target,
                expected_owner_revision: revision,
                column,
                row,
                up,
            })
            .map(|bytes| bytes.to_vec())
            .map_err(|_| "request_invalid")
        })?;
        let outcome = self.settle_response(key, reply)?;
        Ok(match outcome {
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Applied)) => {
                MouseInputResult::Applied
            }
            ResponseOutcome::Rejected(RejectionCode::Unsupported) => MouseInputResult::Unsupported,
            ResponseOutcome::Rejected(
                RejectionCode::InputBusy | RejectionCode::StaleOwner | RejectionCode::ExpiredOwner,
            ) => {
                self.owner_revision = None;
                MouseInputResult::NotOwner
            }
            ResponseOutcome::ServerError(label) => MouseInputResult::Failed(label),
            _ => MouseInputResult::Failed("mouse input not delivered"),
        })
    }

    /// Deliver a vacancy-scoped wheel event as one durable keyed RPC.
    pub(crate) fn send_scoped_mouse_wheel(
        &mut self,
        column: u16,
        row: u16,
        up: bool,
    ) -> Result<MouseInputResult, &'static str> {
        if !self.scoped_mouse_wheel || !self.input_capability {
            return Ok(MouseInputResult::Unsupported);
        }
        let target = self.target;
        let (key, reply) = self.keyed(|key| {
            encode_scoped_mouse_wheel_input(&ScopedMouseWheelInput {
                key,
                target,
                column,
                row,
                up,
            })
            .map(|bytes| bytes.to_vec())
            .map_err(|_| "request_invalid")
        })?;
        Ok(match self.settle_response(key, reply)? {
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Applied)) => {
                MouseInputResult::Applied
            }
            ResponseOutcome::Rejected(RejectionCode::Unsupported) => MouseInputResult::Unsupported,
            ResponseOutcome::Rejected(
                RejectionCode::InputBusy | RejectionCode::StaleOwner | RejectionCode::ExpiredOwner,
            ) => MouseInputResult::NotOwner,
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Failed)) => {
                MouseInputResult::Failed("wheel delivery failed; ownership release not confirmed")
            }
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Partial)) => {
                MouseInputResult::Failed("wheel delivery partial; ownership release not confirmed")
            }
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Unknown)) => {
                MouseInputResult::Failed("wheel delivery unknown; ownership release not confirmed")
            }
            ResponseOutcome::ServerError(label) => MouseInputResult::Failed(label),
            _ => MouseInputResult::Failed(
                "mouse input not delivered; ownership release not confirmed",
            ),
        })
    }

    /// Deliver one paste through control PASTE; never resent.
    pub(crate) fn send_paste(&mut self, text: &str) -> Result<InputResult, &'static str> {
        let Some(owner_revision) = self.owner_revision else {
            return Ok(InputResult::NotOwner);
        };
        let target = self.target;
        let (key, reply) = self.keyed(|key| {
            let request = ControlRequest {
                key,
                target,
                command: Command::Paste {
                    owner_revision,
                    text,
                },
            };
            let length = request
                .encoded_len()
                .map_err(|_| "paste too large; not sent")?;
            let mut bytes = Vec::with_capacity(length);
            encode_request_payload(&request, &mut bytes).map_err(|_| "request_invalid")?;
            Ok(bytes)
        })?;
        let outcome = self.settle_response(key, reply)?;
        Ok(self.input_result(outcome))
    }

    fn input_result(&mut self, outcome: ResponseOutcome) -> InputResult {
        match outcome {
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Applied)) => {
                InputResult::Applied
            }
            ResponseOutcome::Status(OperationStatus::Completed(Outcome::Partial)) => {
                InputResult::Failed("input partially delivered; not resent")
            }
            ResponseOutcome::Rejected(
                RejectionCode::InputBusy | RejectionCode::StaleOwner | RejectionCode::ExpiredOwner,
            ) => {
                self.owner_revision = None;
                InputResult::NotOwner
            }
            ResponseOutcome::ServerError(label) => InputResult::Failed(label),
            _ => InputResult::Failed("input not delivered"),
        }
    }

    /// Retire completed results periodically so the server ledger stays bounded.
    fn maybe_receipt(&mut self) {
        if self.last_admitted >= self.receipted + RECEIPT_EVERY {
            let _ = self.receipt_through(self.last_admitted);
        }
    }

    /// Ask the server to retire results through `through`, recording it only
    /// after the exact success response validates. Refused while an
    /// operation is uncertain.
    pub(crate) fn receipt_through(&mut self, through: u64) -> Result<(), &'static str> {
        if self.context.context().uncertain().is_some() {
            return Err("operation_uncertain");
        }
        if through == 0 || through <= self.receipted {
            return Ok(());
        }
        let state = self.context.context();
        let key = OperationKey {
            generation: state.generation(),
            lease: state.lease(),
            sequence: through,
        };
        let payload = encode_session_request(&SessionRequest::Receipt {
            generation: key.generation,
            lease: key.lease,
            through_sequence: through,
        })
        .map_err(|_| "request_invalid")?;
        let Reply::Response(bytes) = self.request(&payload)? else {
            return Err("receipt refused");
        };
        let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
        if response.key != key
            || response.body != ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        {
            return Err("response_invalid");
        }
        self.receipted = through;
        self.last_admitted = self.last_admitted.max(through);
        Ok(())
    }

    /// After resume, retire results left by earlier attachments.
    ///
    /// Everything below `next_sequence` is settled (uncertainty was resolved
    /// first). The highest *admitted, completed* sequence is found by exact
    /// OperationStatus queries, walking down at most `RESUME_STATUS_PROBES`
    /// keys; only a validated `Completed` status is used as the receipt
    /// bound. A retired key (`result_expired`) ends the walk: everything at
    /// or below it was already receipted.
    fn receipt_all_settled(&mut self) {
        let mut sequence = self.context.context().next_sequence().saturating_sub(1);
        for _ in 0..RESUME_STATUS_PROBES {
            if sequence == 0 {
                return;
            }
            match self.status_of(sequence) {
                Ok(Some(OperationStatus::Completed(_))) => {
                    let _ = self.receipt_through(sequence);
                    return;
                }
                Ok(_) => sequence -= 1,
                Err(_) => return,
            }
        }
    }

    /// Exact status of one of this lease's keys; `Ok(None)` when the server
    /// reports the key unknown-but-older (never admitted).
    fn status_of(&mut self, sequence: u64) -> Result<Option<OperationStatus>, &'static str> {
        let state = self.context.context();
        let key = OperationKey {
            generation: state.generation(),
            lease: state.lease(),
            sequence,
        };
        let payload = encode_session_request(&SessionRequest::OperationStatus { key })
            .map_err(|_| "request_invalid")?;
        match self.request(&payload)? {
            Reply::Response(bytes) => {
                let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
                match (response.key == key, response.body) {
                    (true, ResponseBody::Status(status)) => Ok(Some(status)),
                    _ => Err("response_invalid"),
                }
            }
            Reply::Error("server: sequence_not_increasing") => Ok(None),
            Reply::Error(label) => Err(label),
            Reply::Snapshot(_) => Err("response_invalid"),
        }
    }

    /// Bounded detach work: settle, release the claim, retire results.
    pub(crate) fn detach(&mut self) {
        self.set_deadline(DETACH_DEADLINE);
        if !self.connected() || self.settle_uncertain().is_err() {
            return;
        }
        self.release();
        if self.connected() && self.last_admitted > self.receipted {
            let _ = self.receipt_through(self.last_admitted);
        }
    }

    /// One immediate metadata watch. Returns whether the screen changed.
    pub(crate) fn watch(&mut self) -> Result<bool, &'static str> {
        let target = self.target;
        let generation = self.generation();
        let payload = encode_watch_request(&WatchRequest::Watch {
            target,
            cursor: SessionCursor {
                generation,
                session: target.session,
                sequence: self.cursor,
            },
            limit: WATCH_LIMIT,
            wait_ms: 0,
        })
        .map_err(|_| "request_invalid")?;
        let result = self.watch_exchange(&payload);
        if result.is_err() {
            self.stream = None;
        }
        result
    }

    fn watch_exchange(&mut self, payload: &[u8]) -> Result<bool, &'static str> {
        let target = self.target;
        let id = self.next_request;
        let deadline = self.deadline;
        let stream = self.stream.as_mut().ok_or("pane disconnected")?;
        self.next_request += 1;
        write(stream, MessageKind::Request, id, payload, deadline)?;
        let mut changed = false;
        let mut events = 0_u16;
        for _ in 0..=WATCH_LIMIT {
            let frame = read(stream, deadline)?;
            match (frame.kind(), frame.request_id()) {
                (MessageKind::Event, 0) => {
                    let event = decode_event(frame.payload()).map_err(|_| "response_invalid")?;
                    if event.target != target {
                        return Err("response_invalid");
                    }
                    events += 1;
                    if matches!(
                        event.body,
                        EventBody::ScreenChanged { .. } | EventBody::ChildExited
                    ) {
                        changed = true;
                    }
                }
                (MessageKind::Response, request) if request == id => {
                    let end = decode_watch_end(frame.payload(), target)
                        .map_err(|_| "response_invalid")?;
                    if end.event_count != events {
                        return Err("response_invalid");
                    }
                    self.cursor = end.cursor.sequence;
                    return Ok(changed || end.status == WatchStatus::ResyncRequired);
                }
                (MessageKind::Error, request) if request == id => {
                    // Notifications unavailable: fall back to screen polling.
                    self.events = false;
                    return Ok(true);
                }
                _ => return Err("response_invalid"),
            }
        }
        Err("response_invalid")
    }
}

/// A fresh, already-handshaken discovery stream whose lease is adopted by
/// the first pane's new context.
pub(crate) struct Bootstrap {
    pub(crate) stream: AuthenticatedStream,
    pub(crate) welcome: Welcome,
    /// Highest request ID already used on `stream`.
    pub(crate) used: u64,
    /// When its Hello was started (lease expiry base, conservative).
    pub(crate) started: Instant,
}

/// Why an existing context could not be resumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResumeError {
    /// The lease was dead and the settled context file was removed.
    Removed,
    /// Any other failure; the file is left untouched.
    Other(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseOutcome {
    Status(OperationStatus),
    Rejected(RejectionCode),
    ServerError(&'static str),
}

/// Context file name for one pane; `index` selects a bounded fallback name.
pub(crate) fn context_path(dir: &Path, target: TargetRef, index: usize) -> PathBuf {
    let suffix = if index == 0 {
        String::new()
    } else {
        format!("-{index}")
    };
    dir.join(format!(
        "pane-{}-{}-{}-{}-{}{suffix}.ctx",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    ))
}

/// Parse a `pane-G-W-P-S-O[-N].ctx` file name back into its target.
pub(crate) fn parse_context_name(name: &str) -> Option<TargetRef> {
    let body = name.strip_prefix("pane-")?.strip_suffix(".ctx")?;
    let mut fields = body.split('-').map(|field| {
        if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
            None
        } else {
            field.parse::<u64>().ok().filter(|value| *value != 0)
        }
    });
    let target = TargetRef {
        generation: fields.next()??,
        workspace: fields.next()??,
        pane: fields.next()??,
        session: fields.next()??,
        occupant_revision: fields.next()??,
    };
    match (fields.next(), fields.next()) {
        (None, _) | (Some(Some(_)), None) => Some(target),
        _ => None,
    }
}

/// Create the private context directory if missing (mode 0700, parent must exist).
pub(crate) fn ensure_context_dir(dir: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match fs::symlink_metadata(dir) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.permissions().mode() & 0o777 != 0o700
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                return Err("context directory must be a private (0700) directory");
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|_| "could not create the context directory"),
        Err(_) => Err("context directory is unavailable"),
    }
}

pub(crate) fn connect(
    directory: &Path,
    deadline: Duration,
) -> Result<AuthenticatedStream, &'static str> {
    AuthenticatedStream::connect(directory, Instant::now() + deadline)
        .map_err(|_| "server unavailable")
}

/// Fresh (optionally with an AttachTarget trailer) or resumed Hello.
pub(crate) fn handshake(
    stream: &mut AuthenticatedStream,
    resume: Option<(u64, u64)>,
    target: Option<TargetRef>,
    deadline: Duration,
) -> Result<Welcome, &'static str> {
    let hello = Hello {
        nonce: nonce(),
        required_capabilities: CAPABILITY_READ,
        optional_capabilities: CAPABILITY_INPUT
            | CAPABILITY_EVENTS
            | CAPABILITY_SCREEN_ROW_WRAPS
            | CAPABILITY_HISTORY_CURSOR
            | CAPABILITY_PANE_OPERATIONS
            | CAPABILITY_SCOPED_MOUSE_WHEEL,
    };
    let mut payload = encode_session_hello(&SessionHello {
        hello,
        resume: resume.map(|(generation, lease)| LeaseRef { generation, lease }),
    })
    .map_err(|_| "handshake_invalid")?;
    if let Some(target) = target {
        payload.extend_from_slice(&encode_attach_target(target).map_err(|_| "request_invalid")?);
    }
    write(stream, MessageKind::Hello, 0, &payload, deadline)?;
    let frame = read(stream, deadline)?;
    if frame.kind() == MessageKind::Error && frame.request_id() == 0 {
        return Err(if frame.payload() == b"unknown_context" {
            LEASE_UNKNOWN
        } else {
            "server rejected the attachment"
        });
    }
    if frame.kind() != MessageKind::Welcome || frame.request_id() != 0 {
        return Err("handshake_invalid");
    }
    let welcome = decode_welcome(frame.payload()).map_err(|_| "handshake_invalid")?;
    validate_welcome(&hello, &welcome).map_err(|_| "handshake_invalid")?;
    if let Some((generation, lease)) = resume {
        if welcome.generation != generation || welcome.lease != lease {
            return Err("context_mismatch");
        }
    }
    if welcome.selected_capabilities & CAPABILITY_EVENTS != 0 {
        stream
            .enable_metadata_events()
            .map_err(|_| "transport_failed")?;
    }
    stream
        .enable_snapshot_reads()
        .map_err(|_| "transport_failed")?;
    Ok(welcome)
}

pub(crate) fn write(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    id: u64,
    payload: &[u8],
    deadline: Duration,
) -> Result<(), &'static str> {
    let frame = Frame::new(kind, id, payload).map_err(|_| "request_invalid")?;
    stream
        .write_frame(&frame, Instant::now() + deadline)
        .map_err(|_| "connection lost")
}

pub(crate) fn read(
    stream: &mut AuthenticatedStream,
    deadline: Duration,
) -> Result<OwnedFrame, &'static str> {
    stream
        .read_frame(Instant::now() + deadline)
        .map_err(|_| "connection lost")
}

/// Map a server error label to a fixed local label (never echoes data).
pub(crate) fn server_label(payload: &[u8]) -> &'static str {
    match payload {
        b"unauthorized" => "server: unauthorized",
        b"unknown_context" => "server: unknown_context",
        b"stale_layout" => "server: stale_layout",
        b"layout_rejected" => "server: layout_rejected",
        b"no_layout" => "server: no_layout",
        b"operation_conflict" => "server: operation_conflict",
        b"split_in_progress" => "server: split_in_progress",
        b"capacity_exceeded" => "server: capacity_exceeded",
        b"notifications_unavailable" => "server: notifications_unavailable",
        b"target_stale" => "server: target_stale",
        b"lease_expired" => "server: lease_expired",
        b"result_expired" => "server: result_expired",
        b"invalid_request" | b"unsupported_request_schema" | b"invalid_ui_request" => {
            "scrolling requires a newer Panelink server; restart it"
        }
        b"sequence_not_increasing" => "server: sequence_not_increasing",
        _ => "server error",
    }
}

fn context_label(error: ContextError) -> &'static str {
    match error {
        ContextError::LockTimeout => "context in use by another client",
        ContextError::UnsafeParent | ContextError::UnsafeFile => "context file is unsafe",
        ContextError::UncertainOperation => "operation_uncertain",
        _ => "context file error",
    }
}

fn nonce() -> u64 {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ (u64::from(std::process::id()) << 32);
    let value = seed.wrapping_add(NONCE.fetch_add(1, Ordering::Relaxed));
    if value == 0 {
        1
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScopedPeerRoot(PathBuf);
    impl Drop for ScopedPeerRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scoped_peer_fixture() -> (ScopedPeerRoot, PaneConn, AuthenticatedStream) {
        use std::os::unix::{fs::DirBuilderExt, net::UnixStream};
        let root = ScopedPeerRoot(PathBuf::from(format!(
            "/private/tmp/pl-scoped-peer-{}-{}",
            std::process::id(),
            nonce()
        )));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .unwrap();
        let target = TargetRef {
            generation: 7,
            workspace: 3,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let now = Instant::now();
        let (client, server) = UnixStream::pair().unwrap();
        let pane = PaneConn::attach(
            &root.0,
            &root.0,
            target,
            Some(Bootstrap {
                stream: AuthenticatedStream::new(client).unwrap(),
                welcome: Welcome {
                    nonce: 1,
                    generation: 7,
                    lease: 10,
                    selected_capabilities: CAPABILITY_READ
                        | CAPABILITY_INPUT
                        | CAPABILITY_SCOPED_MOUSE_WHEEL,
                    ttl_seconds: 1800,
                },
                used: 0,
                started: now,
            }),
            now,
            Duration::from_secs(2),
        )
        .unwrap();
        (root, pane, AuthenticatedStream::new(server).unwrap())
    }

    fn scoped_peer_status(
        peer: &mut AuthenticatedStream,
        request_id: u64,
        key: OperationKey,
        outcome: Outcome,
    ) {
        use panelink::response_protocol::{encode_response_payload, Response};
        let mut bytes = Vec::new();
        encode_response_payload(
            &Response {
                key,
                body: ResponseBody::Status(OperationStatus::Completed(outcome)),
            },
            &mut bytes,
        )
        .unwrap();
        peer.write_frame(
            &Frame::new(MessageKind::Response, request_id, &bytes).unwrap(),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    }

    #[test]
    fn scoped_wheel_one_keyed_request_preserves_local_owner_and_settles_once() {
        for outcome in [
            Outcome::Applied,
            Outcome::Failed,
            Outcome::Partial,
            Outcome::Unknown,
        ] {
            let (_root, mut pane, mut peer) = scoped_peer_fixture();
            pane.owner_revision = Some(77);
            std::thread::scope(|scope| {
                let peer_task = scope.spawn(move || {
                    let frame = peer
                        .read_frame(Instant::now() + Duration::from_secs(2))
                        .unwrap();
                    assert_eq!(
                        frame.payload().len(),
                        panelink::ui_protocol::SCOPED_MOUSE_WHEEL_INPUT_BYTES
                    );
                    let panelink::ui_protocol::UiRequest::ScopedMouseWheelInput(request) =
                        panelink::ui_protocol::decode_ui_request(frame.payload()).unwrap()
                    else {
                        panic!("not scoped wheel");
                    };
                    assert_eq!((request.column, request.row, request.up), (4, 5, false));
                    assert_eq!(request.key.sequence, 1);
                    scoped_peer_status(&mut peer, frame.request_id(), request.key, outcome);
                    peer
                });
                let result = pane.send_scoped_mouse_wheel(4, 5, false).unwrap();
                if outcome == Outcome::Applied {
                    assert_eq!(result, MouseInputResult::Applied);
                } else {
                    assert!(
                        matches!(result, MouseInputResult::Failed(label) if label.contains("ownership release not confirmed"))
                    );
                }
                assert_eq!(pane.owner_revision, Some(77));
                assert_eq!(pane.context.context().next_sequence(), 2);
                assert_eq!(pane.context.context().uncertain(), None);
                let mut peer = peer_task.join().unwrap();
                assert!(
                    peer.read_frame(Instant::now() + Duration::from_millis(30))
                        .is_err(),
                    "extra Claim/Release/Describe RPC"
                );
            });
        }
    }

    #[test]
    fn scoped_wheel_wrong_reply_key_reconciles_by_status_without_replay() {
        let (_root, mut pane, mut peer) = scoped_peer_fixture();
        std::thread::scope(|scope| {
            let peer_task = scope.spawn(move || {
                let frame = peer
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                let panelink::ui_protocol::UiRequest::ScopedMouseWheelInput(request) =
                    panelink::ui_protocol::decode_ui_request(frame.payload()).unwrap()
                else {
                    panic!("not scoped wheel");
                };
                scoped_peer_status(
                    &mut peer,
                    frame.request_id(),
                    OperationKey {
                        sequence: request.key.sequence + 1,
                        ..request.key
                    },
                    Outcome::Applied,
                );
                let status = peer
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(
                    panelink::session_protocol::decode_session_request(status.payload()).unwrap(),
                    SessionRequest::OperationStatus { key: request.key }
                );
                scoped_peer_status(
                    &mut peer,
                    status.request_id(),
                    request.key,
                    Outcome::Applied,
                );
            });
            assert_eq!(
                pane.send_scoped_mouse_wheel(4, 5, true),
                Err("response_invalid")
            );
            assert!(pane.context.context().uncertain().is_some());
            pane.settle_uncertain().unwrap();
            assert_eq!(pane.context.context().uncertain(), None);
            assert_eq!(pane.context.context().next_sequence(), 2);
            peer_task.join().unwrap();
        });
    }

    #[test]
    fn scoped_wheel_legacy_vacancy_claim_refuses_retained_same_caller() {
        let (_root, mut pane, mut peer) = scoped_peer_fixture();
        pane.owner_revision = Some(9);
        let target = pane.target;
        std::thread::scope(|scope| {
            let peer_task = scope.spawn(move || {
                let frame = peer
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(
                    panelink::session_protocol::decode_session_request(frame.payload()).unwrap(),
                    SessionRequest::Describe
                );
                let bytes = panelink::session_protocol::encode_session_info(
                    &panelink::session_protocol::SessionInfo {
                        target,
                        owner_lease: 10,
                        owner_revision: 9,
                        owner_expires_secs: 30,
                        child_exited: false,
                    },
                )
                .unwrap();
                peer.write_frame(
                    &Frame::new(MessageKind::Response, frame.request_id(), &bytes).unwrap(),
                    Instant::now() + Duration::from_secs(2),
                )
                .unwrap();
                peer
            });
            assert_eq!(pane.claim_vacant(), Ok(false));
            assert_eq!(pane.owner_revision, Some(9));
            assert_eq!(pane.context.context().next_sequence(), 1);
            let mut peer = peer_task.join().unwrap();
            assert!(
                peer.read_frame(Instant::now() + Duration::from_millis(30))
                    .is_err(),
                "retained claim was renewed"
            );
        });
    }

    #[test]
    fn old_server_reject_label_explains_scroll_compatibility() {
        for label in [
            b"invalid_request".as_slice(),
            b"unsupported_request_schema".as_slice(),
            b"invalid_ui_request".as_slice(),
        ] {
            assert_eq!(
                server_label(label),
                "scrolling requires a newer Panelink server; restart it"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fresh_handshake_offers_row_wraps_optionally_and_accepts_selection() {
        use panelink::handshake_protocol::{
            encode_welcome, Welcome, CAPABILITY_HISTORY_CURSOR, CAPABILITY_SCREEN_ROW_WRAPS,
        };
        use panelink::protocol_wire::{Frame, MessageKind};
        use panelink::session_protocol::decode_session_hello;
        use std::os::unix::net::UnixStream;

        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut server = AuthenticatedStream::new(server_socket).unwrap();
            let frame = server
                .read_frame(Instant::now() + Duration::from_secs(1))
                .unwrap();
            assert_eq!(frame.kind(), MessageKind::Hello);
            let hello = decode_session_hello(frame.payload()).unwrap();
            assert_ne!(
                hello.hello.optional_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
                0
            );
            assert_eq!(
                hello.hello.required_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
                0
            );
            assert_ne!(
                hello.hello.optional_capabilities & CAPABILITY_HISTORY_CURSOR,
                0
            );
            assert_eq!(
                hello.hello.required_capabilities & CAPABILITY_HISTORY_CURSOR,
                0
            );
            let welcome = Welcome {
                nonce: hello.hello.nonce,
                generation: 7,
                lease: 9,
                selected_capabilities: CAPABILITY_READ
                    | CAPABILITY_SCREEN_ROW_WRAPS
                    | CAPABILITY_HISTORY_CURSOR,
                ttl_seconds: panelink::handshake_protocol::HANDSHAKE_LEASE_TTL_SECONDS,
            };
            let payload = encode_welcome(&welcome).unwrap();
            server
                .write_frame(
                    &Frame::new(MessageKind::Welcome, 0, &payload).unwrap(),
                    Instant::now() + Duration::from_secs(1),
                )
                .unwrap();
        });
        let welcome = handshake(&mut client, None, None, Duration::from_secs(1)).unwrap();
        assert_ne!(
            welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
            0
        );
        assert_ne!(welcome.selected_capabilities & CAPABILITY_HISTORY_CURSOR, 0);
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scoped_wheel_requires_both_selected_capabilities_without_reserving() {
        use panelink::handshake_protocol::{Welcome, CAPABILITY_READ};
        use std::os::unix::{fs::DirBuilderExt, net::UnixStream};
        use std::time::SystemTime;

        for selected in [CAPABILITY_READ, CAPABILITY_READ | CAPABILITY_INPUT] {
            let serial = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = PathBuf::from(format!(
                "/private/tmp/panelink-scoped-cap-{}-{serial}",
                std::process::id()
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let target = TargetRef {
                generation: 7,
                workspace: 3,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            };
            let now = Instant::now();
            let (client_socket, peer_socket) = UnixStream::pair().unwrap();
            let mut pane = PaneConn::attach(
                &directory,
                &directory,
                target,
                Some(Bootstrap {
                    stream: AuthenticatedStream::new(client_socket).unwrap(),
                    welcome: Welcome {
                        nonce: 1,
                        generation: 7,
                        lease: 10,
                        selected_capabilities: selected,
                        ttl_seconds: 1800,
                    },
                    used: 0,
                    started: now,
                }),
                now,
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(
                pane.send_scoped_mouse_wheel(4, 5, true).unwrap(),
                MouseInputResult::Unsupported
            );
            assert_eq!(pane.context.context().next_sequence(), 1);
            assert_eq!(pane.context.context().uncertain(), None);
            peer_socket
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let mut byte = [0];
            use std::io::Read;
            assert!(
                matches!(peer_socket.try_clone().unwrap().read(&mut byte), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut)
            );
            drop(pane);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_release_requires_applied_reply_and_vacant_matching_describe() {
        use panelink::handshake_protocol::{Welcome, CAPABILITY_READ};
        use panelink::response_protocol::{encode_response_payload, Response, ResponseBody};
        use panelink::session_protocol::{
            decode_session_request, encode_session_info, SessionInfo, SessionRequest,
        };
        use panelink::transport::AuthenticatedStream;
        use std::os::unix::{fs::DirBuilderExt, net::UnixStream};
        use std::time::SystemTime;

        #[derive(Clone, Copy)]
        enum ReplyCase {
            AppliedAndVacant,
            Refused,
            Lost,
        }

        for case in [
            ReplyCase::AppliedAndVacant,
            ReplyCase::Refused,
            ReplyCase::Lost,
        ] {
            let serial = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = PathBuf::from(format!(
                "/private/tmp/panelink-release-{}-{serial}",
                std::process::id()
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let target = TargetRef {
                generation: 7,
                workspace: 3,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            };
            let now = Instant::now();
            let (client_socket, peer_socket) = UnixStream::pair().unwrap();
            let mut pane = PaneConn::attach(
                &directory,
                &directory,
                target,
                Some(Bootstrap {
                    stream: AuthenticatedStream::new(client_socket).unwrap(),
                    welcome: Welcome {
                        nonce: 1,
                        generation: target.generation,
                        lease: 10,
                        selected_capabilities: CAPABILITY_READ,
                        ttl_seconds: 1800,
                    },
                    used: 0,
                    started: now,
                }),
                now,
                Duration::from_secs(2),
            )
            .unwrap();
            pane.owner_revision = Some(5);
            let peer = std::thread::spawn(move || {
                let mut peer = AuthenticatedStream::new(peer_socket).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                let request = peer.read_frame(deadline).unwrap();
                let SessionRequest::Release {
                    key,
                    target: request_target,
                    expected_owner_revision,
                } = decode_session_request(request.payload()).unwrap()
                else {
                    panic!("expected keyed release");
                };
                assert_eq!(request_target, target);
                assert_eq!(expected_owner_revision, 5);
                if matches!(case, ReplyCase::Lost) {
                    return;
                }
                let mut bytes = Vec::new();
                let body = match case {
                    ReplyCase::AppliedAndVacant => {
                        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                    }
                    ReplyCase::Refused => ResponseBody::Rejected(RejectionCode::StaleOwner),
                    ReplyCase::Lost => unreachable!(),
                };
                encode_response_payload(&Response { key, body }, &mut bytes).unwrap();
                peer.write_frame(
                    &Frame::new(MessageKind::Response, request.request_id(), &bytes).unwrap(),
                    deadline,
                )
                .unwrap();
                if matches!(case, ReplyCase::AppliedAndVacant) {
                    let describe = peer.read_frame(deadline).unwrap();
                    assert_eq!(
                        decode_session_request(describe.payload()).unwrap(),
                        SessionRequest::Describe
                    );
                    let bytes = encode_session_info(&SessionInfo {
                        target,
                        owner_revision: 6,
                        owner_lease: 0,
                        owner_expires_secs: 0,
                        child_exited: false,
                    })
                    .unwrap();
                    peer.write_frame(
                        &Frame::new(MessageKind::Response, describe.request_id(), &bytes).unwrap(),
                        deadline,
                    )
                    .unwrap();
                }
            });

            let result = pane.release_confirmed();
            match case {
                ReplyCase::AppliedAndVacant => {
                    assert_eq!(result, Ok(()));
                    assert_eq!(pane.owner_revision, None);
                }
                ReplyCase::Refused | ReplyCase::Lost => {
                    assert_eq!(
                        result,
                        Err("input delivery confirmed; ownership release unconfirmed")
                    );
                    assert_eq!(pane.owner_revision, Some(5));
                }
            }
            peer.join().unwrap();
            drop(pane);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn lease_renewal_is_scheduled_before_expiry_and_retried_on_failure() {
        let t0 = Instant::now();
        let mut schedule = LeaseSchedule::new(t0);
        let renew_at = t0 + LEASE_TTL - RENEW_MARGIN;
        assert_eq!(
            schedule.action(renew_at - Duration::from_secs(1)),
            LeaseAction::Wait
        );
        assert_eq!(schedule.action(renew_at), LeaseAction::Renew);
        // The margin leaves room for many retries before the server's expiry.
        assert!(RENEW_MARGIN >= RENEW_RETRY * 10);
        assert!(LEASE_TTL > RENEW_MARGIN);

        // A failed attempt retries after RENEW_RETRY, without moving the
        // conservative expiry estimate.
        schedule.failed(renew_at);
        assert_eq!(
            schedule.action(renew_at + RENEW_RETRY - Duration::from_millis(1)),
            LeaseAction::Wait
        );
        assert_eq!(schedule.action(renew_at + RENEW_RETRY), LeaseAction::Renew);
        assert!(!schedule.past_ttl(t0 + LEASE_TTL - Duration::from_millis(1)));
        assert!(schedule.past_ttl(t0 + LEASE_TTL));

        // A successful resume restarts from the time the Hello *started*,
        // which is never later than the server's renewal time.
        let started = renew_at + Duration::from_secs(2);
        schedule.renewed(started);
        assert_eq!(schedule, LeaseSchedule::new(started));
        assert!(!schedule.past_ttl(t0 + LEASE_TTL));
        assert_eq!(
            schedule.action(started + LEASE_TTL - RENEW_MARGIN),
            LeaseAction::Renew
        );

        // Simulated healthy run over several lease lifetimes with an injected
        // clock: renewals always happen strictly before expiry.
        let mut schedule = LeaseSchedule::new(t0);
        let mut now = t0;
        let step = Duration::from_secs(1);
        let mut renewals = 0;
        while now < t0 + LEASE_TTL * 5 {
            assert!(!schedule.past_ttl(now), "lease estimate expired");
            if schedule.action(now) == LeaseAction::Renew {
                schedule.renewed(now);
                renewals += 1;
            }
            now += step;
        }
        assert!(renewals >= 5);
    }

    #[test]
    fn context_names_round_trip_and_reject_foreign_files() {
        let target = TargetRef {
            generation: 17,
            workspace: 1,
            pane: 2,
            session: 3,
            occupant_revision: 4,
        };
        let dir = Path::new("/private/tmp/ctx");
        for index in [0, 1, 7] {
            let path = context_path(dir, target, index);
            let name = path.file_name().unwrap().to_str().unwrap();
            assert_eq!(parse_context_name(name), Some(target), "{name}");
        }
        for name in [
            "pane-17-1-2-3.ctx",
            "pane-17-1-2-3-4-5-6.ctx",
            "pane-17-1-2-3-0.ctx",
            "pane-17-1-2-3-4-.ctx",
            "pane-a-1-2-3-4.ctx",
            "other.ctx",
            "pane-17-1-2-3-4.ctx.tmp",
        ] {
            assert_eq!(parse_context_name(name), None, "{name}");
        }
    }
}
