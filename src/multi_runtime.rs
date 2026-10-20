//! Bounded multi-session runtime: authenticated connection workers forward
//! validated requests to the single owner thread, which alone touches PTYs.
//!
//! This is a parallel runtime beside the one-child [`crate::server_runtime`].
//! Workers never spawn, resize, read or write a PTY; every launch, split,
//! key delivery and screen capture runs inside [`MultiServerDriver::tick`] on
//! the thread that owns the driver. Connection loss never cancels admitted
//! work and never stops a child.
//!
//! Connection model: each lease grant is pinned to one full target, so a UI
//! uses one connection per pane. The Hello frame carries an existing
//! SessionHello optionally followed by a schema-8 AttachTarget trailer naming
//! the pane; without it a fresh Hello attaches to the first layout pane and a
//! resume Hello to its lease's pinned target.

use crate::close_protocol::{
    decode_close_request, decode_close_result, decode_close_status_query, encode_close_result,
    CloseResult, CLOSE_RESULT_BYTES, CLOSE_SCHEMA,
};
use crate::control_protocol::TargetRef;
use crate::event_protocol::{encode_event, Event};
use crate::handshake_protocol::{
    encode_welcome, validate_welcome, Welcome, CAPABILITY_EVENTS, CAPABILITY_READ,
};
use crate::history_store::HistoryStore;
use crate::managed_launch_prepare::prepare_launch;
use crate::managed_launch_protocol::{decode_launch_result, decode_status_query};
use crate::pane_layout::Rect;
use crate::protocol::OperationKey;
use crate::protocol_wire::{Frame, MessageKind};
use crate::response_protocol::decode_response_payload;
use crate::scoped_watch_protocol::{
    decode_reconciliation, decode_watch_request, encode_watch_end, WatchEnd, WatchRequest,
};
use crate::server::multi_session::{CloseError, MultiSessionOwner};
use crate::server::{Connection, ServerError};
use crate::server_io::{
    MAX_CONNECTIONS, OWNER_QUEUE_CAPACITY, OWNER_REPLY_DEADLINE, READ_DEADLINE, STOP_POLL_INTERVAL,
    WRITE_DEADLINE,
};
use crate::server_pty::PtyEffects;
use crate::server_storage::ServerStorage;
use crate::session_protocol::{
    decode_session_hello, decode_session_info, decode_session_request, SessionHello,
    SessionRequest, SESSION_HELLO_FRESH_BYTES, SESSION_HELLO_RESUME_BYTES,
};
use crate::session_registry::SessionState;
use crate::split_protocol::{decode_split_request, decode_split_result};
use crate::transport::{AuthenticatedStream, OwnedFrame};
use crate::ui_protocol::{
    decode_attach_target, decode_layout_reply, decode_resize_reply, decode_screen_reply,
    decode_ui_request, UiRequest, ATTACH_TARGET_BYTES,
};
use crate::watch_protocol::WatchStatus;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Maximum finite watches retained between owner ticks.
pub const MAX_PENDING_WATCHES: usize = 8;

/// Write deadline for one Snapshot frame (at most about 0.9 MB).
pub const SNAPSHOT_WRITE_DEADLINE: Duration = Duration::from_secs(2);

/// How long a worker waits for the owner's reply to a schema-9 close.
///
/// The owner runs the bounded stop (HUP, 1 s, KILL, 1 s) synchronously inside
/// [`MultiServerDriver::tick`], so a close reply can take about 2 s plus queue
/// latency. Other requests keep [`OWNER_REPLY_DEADLINE`]. This stays below
/// the attach client's 4 s close deadline.
pub const CLOSE_REPLY_DEADLINE: Duration = Duration::from_millis(3_500);

const MAX_ERROR_LABEL_BYTES: usize = 128;
const DETACHED_ACK: [u8; 4] = [0, 2, 0, 2];

/// One request for the owner thread.
pub enum MultiRequest {
    /// Initial handshake plus an optional explicit attach target.
    Hello {
        /// Existing schema-1/2 Hello.
        hello: SessionHello,
        /// Target named by the schema-8 AttachTarget trailer, if present.
        target: Option<TargetRef>,
    },
    /// One validated request payload from an established connection.
    Request {
        /// Connection created by the owner for this worker.
        connection: Connection,
        /// Exact request payload bytes; never logged.
        payload: Vec<u8>,
    },
}

impl fmt::Debug for MultiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello { target, .. } => f
                .debug_struct("Hello")
                .field("explicit_target", &target.is_some())
                .finish_non_exhaustive(),
            Self::Request { payload, .. } => f
                .debug_struct("Request")
                .field("payload_len", &payload.len())
                .finish_non_exhaustive(),
        }
    }
}

/// One bounded owner reply.
pub enum MultiReply {
    /// Successful attach.
    Welcome {
        /// Connection pinned to one target.
        connection: Connection,
        /// Negotiated Welcome.
        welcome: Welcome,
    },
    /// Encoded response payload for a Response frame.
    Response(Vec<u8>),
    /// Encoded schema-8 ScreenReply for one Snapshot frame.
    Snapshot(Vec<u8>),
    /// Metadata events followed by a scoped terminal watch result.
    Watch {
        /// Ordered complete metadata events.
        events: Vec<Event>,
        /// Terminal scoped watch status.
        end: WatchEnd,
    },
    /// Fixed lower-snake-case label; never caller content.
    Error(&'static str),
    /// Acknowledge Detach and close only this connection.
    Detached,
}

impl fmt::Debug for MultiReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Welcome { .. } => f.write_str("Welcome { .. }"),
            Self::Response(bytes) => write!(f, "Response({} bytes)", bytes.len()),
            Self::Snapshot(bytes) => write!(f, "Snapshot({} bytes)", bytes.len()),
            Self::Watch { events, .. } => write!(f, "Watch({} events)", events.len()),
            Self::Error(_) => f.write_str("Error(<label>)"),
            Self::Detached => f.write_str("Detached"),
        }
    }
}

/// One request with its one-slot reply channel.
pub struct MultiEnvelope {
    /// The forwarded request.
    pub request: MultiRequest,
    reply: SyncSender<MultiReply>,
}

impl MultiEnvelope {
    /// Send the single reply without blocking the owner thread.
    pub fn try_reply(&self, reply: MultiReply) -> Result<(), TrySendError<MultiReply>> {
        self.reply.try_send(reply)
    }
}

impl fmt::Debug for MultiEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiEnvelope")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

/// Why an authenticated stream was not admitted.
#[derive(Debug)]
pub enum AttachError {
    /// All connection slots are occupied.
    AtCapacity,
    /// Shutdown has started.
    Stopped,
    /// The worker thread could not be created.
    Spawn(io::Error),
}

impl fmt::Display for AttachError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AtCapacity => "connection worker capacity is full",
            Self::Stopped => "connection worker hub is stopping",
            Self::Spawn(_) => "connection worker could not be started",
        })
    }
}

impl std::error::Error for AttachError {}

struct Hub {
    sender: SyncSender<MultiEnvelope>,
    receiver: Receiver<MultiEnvelope>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    workers: Vec<JoinHandle<()>>,
}

impl Hub {
    fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(OWNER_QUEUE_CAPACITY);
        Self {
            sender,
            receiver,
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            workers: Vec::with_capacity(MAX_CONNECTIONS),
        }
    }

    fn start(&mut self, stream: AuthenticatedStream) -> Result<(), AttachError> {
        self.reap_finished();
        if self.stop.load(Ordering::Acquire) {
            return Err(AttachError::Stopped);
        }
        if self.workers.len() >= MAX_CONNECTIONS {
            return Err(AttachError::AtCapacity);
        }
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= MAX_CONNECTIONS {
                return Err(AttachError::AtCapacity);
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        let guard = ActiveGuard(Arc::clone(&self.active));
        let stop = Arc::clone(&self.stop);
        let sender = self.sender.clone();
        match thread::Builder::new()
            .name("panelink-multi-connection".to_owned())
            .spawn(move || {
                let _guard = guard;
                run_worker(stream, &sender, &stop);
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                Ok(())
            }
            Err(error) => Err(AttachError::Spawn(error)),
        }
    }

    fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let _ = self.workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Multi-session driver: one owner, its storage lock, and bounded workers.
///
/// Field order is the teardown order: connection workers stop first (in
/// `Drop`), then the owner reaps both children, then storage releases the
/// singleton lock for the next generation.
pub struct MultiServerDriver {
    hub: Hub,
    pending_watches: Vec<PendingWatch>,
    owner: MultiSessionOwner<PtyEffects>,
    storage: ServerStorage,
    terminfo: PathBuf,
    history: Option<Arc<HistoryStore>>,
    history_failure: Arc<AtomicBool>,
}

struct HistoryCapture<'a> {
    store: Option<&'a Arc<HistoryStore>>,
    failure: &'a Arc<AtomicBool>,
}

impl fmt::Debug for MultiServerDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiServerDriver")
            .field("owner", &self.owner)
            .field("pending_watches", &self.pending_watches.len())
            .finish_non_exhaustive()
    }
}

impl Drop for MultiServerDriver {
    fn drop(&mut self) {
        self.hub.shutdown();
        self.settle_pending_watches();
        self.capture_now();
    }
}

impl MultiServerDriver {
    /// Assemble a driver from an owner whose generation matches `storage`.
    ///
    /// `terminfo` is the explicit server-owned terminfo directory used by
    /// split launch preparation. No thread or listener is created here.
    pub fn new(
        owner: MultiSessionOwner<PtyEffects>,
        storage: ServerStorage,
        terminfo: PathBuf,
    ) -> Self {
        Self {
            hub: Hub::new(),
            pending_watches: Vec::with_capacity(MAX_PENDING_WATCHES),
            owner,
            storage,
            terminfo,
            history: None,
            history_failure: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Opt in to private checkpoint capture for this server generation.
    /// Existing and subsequently split sessions share one owner-thread writer.
    pub fn enable_history(&mut self, store: HistoryStore) {
        let store = Arc::new(store);
        self.owner.for_each_effects_mut(|target, effects| {
            effects.enable_capture(
                Arc::clone(&store),
                Arc::clone(&self.history_failure),
                target,
            );
        });
        self.history = Some(store);
    }

    /// Force a capture without interrupting any PTY if storage fails.
    pub fn capture_now(&mut self) {
        self.owner
            .for_each_effects_mut(|_, effects| effects.capture_now());
    }

    /// Sticky failure indicator, including panes retired after a failed write.
    pub fn history_capture_failed(&self) -> bool {
        self.history_failure.load(Ordering::Relaxed)
    }

    /// Sticky capture failure signal shared with retired panes.
    pub fn history_failure_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.history_failure)
    }

    /// Whether a private checkpoint writer was opened for this server.
    pub fn history_available(&self) -> bool {
        self.history.is_some()
    }

    /// Admit one kernel-authenticated connection within the worker cap.
    pub fn attach(&mut self, stream: AuthenticatedStream) -> Result<(), AttachError> {
        self.hub.start(stream)
    }

    /// Poll every retained PTY once, then process at most eight queued
    /// requests and the bounded pending watches, all on this thread.
    ///
    /// A per-session poll error is returned after the queue was still served.
    pub fn tick(&mut self, clock: impl Fn() -> u64) -> Result<(), ServerError> {
        let progress = self.owner.poll(clock());
        self.process_pending(&clock);
        self.process_pending_watches(&clock);
        self.hub.reap_finished();
        progress
    }

    /// Borrow the owner for observation.
    pub const fn owner(&self) -> &MultiSessionOwner<PtyEffects> {
        &self.owner
    }

    /// Server generation held by the storage lock.
    pub const fn generation(&self) -> u64 {
        self.storage.generation()
    }

    /// Fail closed if runtime paths no longer identify the held lock.
    pub fn verify_storage(&self) -> io::Result<()> {
        self.storage.verify_identity()
    }

    /// Live connection workers, never more than eight.
    pub fn active_connections(&self) -> usize {
        self.hub.active.load(Ordering::Acquire)
    }

    /// Finite watches waiting for a later tick.
    pub fn pending_watches(&self) -> usize {
        self.pending_watches.len()
    }

    /// Stop workers and settle queued requests; children stay alive until
    /// the driver drops.
    pub fn stop_connections(&mut self, clock: impl Fn() -> u64) {
        self.hub.shutdown();
        self.process_pending(&clock);
        self.settle_pending_watches();
    }

    fn process_pending(&mut self, clock: &impl Fn() -> u64) {
        for _ in 0..OWNER_QUEUE_CAPACITY {
            let Ok(envelope) = self.hub.receiver.try_recv() else {
                break;
            };
            let result = dispatch(
                &mut self.owner,
                &mut self.storage,
                &self.terminfo,
                HistoryCapture {
                    store: self.history.as_ref(),
                    failure: &self.history_failure,
                },
                &envelope.request,
                clock(),
                self.pending_watches.len() < MAX_PENDING_WATCHES,
            );
            // Detach is an observation boundary, not a process stop. Save
            // changed visible screens after the owner's request is handled.
            if matches!(&envelope.request, MultiRequest::Request { payload, .. } if crate::session_protocol::decode_session_request(payload).is_ok_and(|request| matches!(request, SessionRequest::Detach)))
            {
                self.capture_now();
            }
            match result {
                // A lost acknowledgment never undoes or resends an effect.
                DispatchResult::Reply(reply) => {
                    let _ = envelope.try_reply(reply);
                }
                DispatchResult::Pending { request, deadline } => {
                    self.pending_watches.push(PendingWatch {
                        envelope,
                        request,
                        deadline,
                    });
                }
            }
        }
    }

    fn process_pending_watches(&mut self, clock: &impl Fn() -> u64) {
        let mut index = 0;
        while index < self.pending_watches.len() {
            match resolve_pending_watch(&mut self.owner, &self.pending_watches[index], clock()) {
                Some(reply) => {
                    let pending = self.pending_watches.swap_remove(index);
                    let _ = pending.envelope.try_reply(reply);
                }
                None => index += 1,
            }
        }
    }

    fn settle_pending_watches(&mut self) {
        for pending in self.pending_watches.drain(..) {
            let _ = pending
                .envelope
                .try_reply(MultiReply::Error("server_stopping"));
        }
    }
}

/// Prepare a split child only from its embedded schema-6 request (the
/// client's captured cwd/environment) and the server-owned terminfo.
pub(crate) fn split_prepare(
    terminfo: &std::path::Path,
) -> impl FnOnce(
    crate::managed_launch_protocol::LaunchRequestView<'_>,
    TargetRef,
) -> Result<crate::managed_launch_prepare::PreparedLaunch, ()>
       + '_ {
    move |request, target| prepare_launch(request, target, terminfo).map_err(|_| ())
}

// ---------------------------------------------------------------------------
// Schema-9 pane status (kind 3 query, kind 0x8003 reply).
//
// Read-only companion of the schema-9 close codec in `close_protocol`: lists
// the current layout panes with lifecycle state so a caller can choose a
// complete target and the current layout revision for a close. It carries no
// pane content. All integers are big-endian.
//
// Query, exactly 8 bytes: schema 9, kind 3, four zero bytes.
//
// Reply, exactly 136 bytes:
// | Offset | Size | Field |
// | 0 | 2 | schema 9 |
// | 2 | 2 | kind 0x8003 |
// | 4 | 4 | reserved, zero |
// | 8 | 8 | layout revision; 0 only when no layout exists |
// | 16 | 2 | root rows |
// | 18 | 2 | root columns |
// | 20 | 4 | reserved, zero |
// | 24 | 56 | pane entry 0 |
// | 80 | 56 | pane entry 1 |
//
// Pane entry: target (5 x u64), rect x/y/rows/columns (4 x u16), state (u8:
// 1 Reserved, 2 Intent, 3 Spawned, 4 Applied, 5 Failed, 6 Unknown,
// 7 Exited), flags (u8: bit0 present, bit1 live, bit2 resize pending), six
// zero bytes. An absent entry is all zero.
// ---------------------------------------------------------------------------

/// Exact size of a schema-9 pane status query.
pub const PANE_QUERY_BYTES: usize = 8;
/// Exact size of a schema-9 pane status reply.
pub const PANE_REPLY_BYTES: usize = 136;

const PANE_QUERY_KIND: u16 = 3;
const PANE_REPLY_KIND: u16 = 0x8003;
const PANE_ENTRY_BYTES: usize = 56;
const PANE_FLAG_PRESENT: u8 = 1;
const PANE_FLAG_LIVE: u8 = 2;
const PANE_FLAG_RESIZE_PENDING: u8 = 4;

/// One current layout pane in a [`PaneReply`].
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneEntry {
    /// Complete identity of the pane occupant.
    pub target: TargetRef,
    /// Pane rectangle in the root content area.
    pub rect: Rect,
    /// Lifecycle state of the pane's session record.
    pub state: SessionState,
    /// Whether the child is still running as far as the owner knows.
    pub live: bool,
    /// Whether the pane's absolute PTY resize is still pending.
    pub resize_pending: bool,
}

impl fmt::Debug for PaneEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneEntry")
            .field("target", &"<redacted>")
            .field("rect", &self.rect)
            .field("state", &self.state)
            .field("live", &self.live)
            .field("resize_pending", &self.resize_pending)
            .finish()
    }
}

/// Current layout panes in spatial order; unused entries are `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneReply {
    /// Layout revision a close must name; 0 when no layout exists.
    pub layout_revision: u64,
    /// Root rows.
    pub rows: u16,
    /// Root columns.
    pub columns: u16,
    /// Spatially ordered panes (left/top first).
    pub panes: [Option<PaneEntry>; 2],
}

/// Fixed, redacted pane-status codec failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneCodecError;

impl fmt::Display for PaneCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("pane status payload is invalid")
    }
}

impl std::error::Error for PaneCodecError {}

/// Encode the fixed schema-9 pane status query.
pub fn encode_pane_query() -> [u8; PANE_QUERY_BYTES] {
    let mut bytes = [0_u8; PANE_QUERY_BYTES];
    bytes[..2].copy_from_slice(&CLOSE_SCHEMA.to_be_bytes());
    bytes[2..4].copy_from_slice(&PANE_QUERY_KIND.to_be_bytes());
    bytes
}

/// Decode exactly one schema-9 pane status query.
pub fn decode_pane_query(bytes: &[u8]) -> Result<(), PaneCodecError> {
    if bytes == encode_pane_query() {
        Ok(())
    } else {
        Err(PaneCodecError)
    }
}

const fn session_state_code(state: SessionState) -> u8 {
    match state {
        SessionState::Reserved => 1,
        SessionState::Intent => 2,
        SessionState::Spawned => 3,
        SessionState::Applied => 4,
        SessionState::Failed => 5,
        SessionState::Unknown => 6,
        SessionState::Exited => 7,
    }
}

const fn session_state_from_code(code: u8) -> Option<SessionState> {
    Some(match code {
        1 => SessionState::Reserved,
        2 => SessionState::Intent,
        3 => SessionState::Spawned,
        4 => SessionState::Applied,
        5 => SessionState::Failed,
        6 => SessionState::Unknown,
        7 => SessionState::Exited,
        _ => return None,
    })
}

fn target_valid(target: TargetRef) -> bool {
    target.generation != 0
        && target.workspace != 0
        && target.pane != 0
        && target.session != 0
        && target.occupant_revision != 0
}

/// Validate and encode one pane status reply.
pub fn encode_pane_reply(reply: &PaneReply) -> Result<[u8; PANE_REPLY_BYTES], PaneCodecError> {
    let present = reply.panes.iter().flatten().count();
    if (reply.layout_revision == 0) != (present == 0) {
        return Err(PaneCodecError);
    }
    let mut bytes = [0_u8; PANE_REPLY_BYTES];
    bytes[..2].copy_from_slice(&CLOSE_SCHEMA.to_be_bytes());
    bytes[2..4].copy_from_slice(&PANE_REPLY_KIND.to_be_bytes());
    bytes[8..16].copy_from_slice(&reply.layout_revision.to_be_bytes());
    bytes[16..18].copy_from_slice(&reply.rows.to_be_bytes());
    bytes[18..20].copy_from_slice(&reply.columns.to_be_bytes());
    for (index, entry) in reply.panes.iter().enumerate() {
        let Some(entry) = entry else {
            continue;
        };
        if !target_valid(entry.target) {
            return Err(PaneCodecError);
        }
        let base = 24 + index * PANE_ENTRY_BYTES;
        let target = entry.target;
        for (slot, value) in [
            target.generation,
            target.workspace,
            target.pane,
            target.session,
            target.occupant_revision,
        ]
        .into_iter()
        .enumerate()
        {
            bytes[base + slot * 8..base + slot * 8 + 8].copy_from_slice(&value.to_be_bytes());
        }
        let rect = entry.rect;
        for (slot, value) in [rect.x, rect.y, rect.rows, rect.columns]
            .into_iter()
            .enumerate()
        {
            bytes[base + 40 + slot * 2..base + 42 + slot * 2].copy_from_slice(&value.to_be_bytes());
        }
        bytes[base + 48] = session_state_code(entry.state);
        bytes[base + 49] = PANE_FLAG_PRESENT
            | if entry.live { PANE_FLAG_LIVE } else { 0 }
            | if entry.resize_pending {
                PANE_FLAG_RESIZE_PENDING
            } else {
                0
            };
    }
    Ok(bytes)
}

/// Decode and validate exactly one pane status reply.
pub fn decode_pane_reply(bytes: &[u8]) -> Result<PaneReply, PaneCodecError> {
    // The reply comes from the socket. Validate the entire fixed-size frame
    // before any indexed field reads (including the header).
    if bytes.len() != PANE_REPLY_BYTES {
        return Err(PaneCodecError);
    }
    let read_u16 = |offset: usize| u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    let read_u64 = |offset: usize| {
        let mut field = [0_u8; 8];
        field.copy_from_slice(&bytes[offset..offset + 8]);
        u64::from_be_bytes(field)
    };
    if read_u16(0) != CLOSE_SCHEMA
        || read_u16(2) != PANE_REPLY_KIND
        || bytes[4..8].iter().any(|byte| *byte != 0)
        || bytes[20..24].iter().any(|byte| *byte != 0)
    {
        return Err(PaneCodecError);
    }
    let mut panes = [None; 2];
    for (index, slot) in panes.iter_mut().enumerate() {
        let base = 24 + index * PANE_ENTRY_BYTES;
        let entry = &bytes[base..base + PANE_ENTRY_BYTES];
        let flags = entry[49];
        if flags & PANE_FLAG_PRESENT == 0 {
            if entry.iter().any(|byte| *byte != 0) {
                return Err(PaneCodecError);
            }
            continue;
        }
        if flags & !(PANE_FLAG_PRESENT | PANE_FLAG_LIVE | PANE_FLAG_RESIZE_PENDING) != 0
            || entry[50..].iter().any(|byte| *byte != 0)
        {
            return Err(PaneCodecError);
        }
        let target = TargetRef {
            generation: read_u64(base),
            workspace: read_u64(base + 8),
            pane: read_u64(base + 16),
            session: read_u64(base + 24),
            occupant_revision: read_u64(base + 32),
        };
        if !target_valid(target) {
            return Err(PaneCodecError);
        }
        *slot = Some(PaneEntry {
            target,
            rect: Rect {
                x: read_u16(base + 40),
                y: read_u16(base + 42),
                rows: read_u16(base + 44),
                columns: read_u16(base + 46),
            },
            state: session_state_from_code(entry[48]).ok_or(PaneCodecError)?,
            live: flags & PANE_FLAG_LIVE != 0,
            resize_pending: flags & PANE_FLAG_RESIZE_PENDING != 0,
        });
    }
    let reply = PaneReply {
        layout_revision: read_u64(8),
        rows: read_u16(16),
        columns: read_u16(18),
        panes,
    };
    if (reply.layout_revision == 0) != reply.panes.iter().flatten().next().is_none() {
        return Err(PaneCodecError);
    }
    Ok(reply)
}

// ---------------------------------------------------------------------------
// Owner-thread dispatch. Everything below `dispatch` runs on the owner thread.
// ---------------------------------------------------------------------------

struct PendingWatch {
    envelope: MultiEnvelope,
    request: WatchRequest,
    deadline: Instant,
}

enum DispatchResult {
    Reply(MultiReply),
    Pending {
        request: WatchRequest,
        deadline: Instant,
    },
}

fn reply(value: MultiReply) -> DispatchResult {
    DispatchResult::Reply(value)
}

fn error(error: &ServerError) -> DispatchResult {
    reply(MultiReply::Error(error.code()))
}

fn dispatch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    storage: &mut ServerStorage,
    terminfo: &std::path::Path,
    history: HistoryCapture<'_>,
    request: &MultiRequest,
    now: u64,
    pending_capacity: bool,
) -> DispatchResult {
    match request {
        MultiRequest::Hello { hello, target } => {
            let selected = match (target, hello.resume) {
                (Some(target), _) => Some(*target),
                (None, Some(resume)) => owner.lease_target(resume.lease),
                (None, None) => owner.first_pane_target(),
            };
            let Some(selected) = selected else {
                return reply(MultiReply::Error("unknown_context"));
            };
            match owner.connect(selected, hello, now) {
                Ok((connection, welcome)) => reply(MultiReply::Welcome {
                    connection,
                    welcome,
                }),
                Err(error) => self::error(&error),
            }
        }
        MultiRequest::Request {
            connection,
            payload,
        } => match payload.get(..2) {
            Some([0, 1]) => match owner.handle_control(connection, payload, now) {
                Ok(bytes) => reply(MultiReply::Response(bytes)),
                Err(error) => self::error(&error),
            },
            Some([0, 2]) => dispatch_session(owner, connection, payload, now),
            Some([0, 4]) => match decode_legacy_watch(payload) {
                Some(request) => {
                    dispatch_watch(owner, connection, request, now, pending_capacity, true)
                }
                None => reply(MultiReply::Error("invalid_watch_request")),
            },
            Some([0, 5]) => match decode_watch_request(payload) {
                Ok(request) => {
                    dispatch_watch(owner, connection, request, now, pending_capacity, false)
                }
                Err(_) => reply(MultiReply::Error("invalid_watch_request")),
            },
            Some([0, 6]) => dispatch_launch(owner, connection, payload, now),
            Some([0, 7]) => {
                dispatch_split(owner, storage, terminfo, history, connection, payload, now)
            }
            Some([0, 8]) => dispatch_ui(owner, connection, payload, now),
            Some([0, 9]) => dispatch_close(owner, connection, payload, now),
            _ => reply(MultiReply::Error("unsupported_request_schema")),
        },
    }
}

fn dispatch_session(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    match owner.handle_session(connection, payload, now) {
        Ok(crate::server::SessionReply::Info(info)) => {
            match crate::session_protocol::encode_session_info(&info) {
                Ok(bytes) => reply(MultiReply::Response(bytes.to_vec())),
                Err(_) => reply(MultiReply::Error("invalid_session_info")),
            }
        }
        Ok(crate::server::SessionReply::Payload(bytes)) => reply(MultiReply::Response(bytes)),
        Ok(crate::server::SessionReply::Detached) => reply(MultiReply::Detached),
        Err(error) => self::error(&error),
    }
}

fn dispatch_launch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    // Client-initiated launch (kind 1) is not offered in this phase; only a
    // split embeds a launch. Status queries observe the caller's own keys.
    let Ok(query) = decode_status_query(payload) else {
        return reply(MultiReply::Error("launch_unsupported"));
    };
    match owner.launch_status(connection, query.key, now) {
        Ok(result) => {
            let mut bytes = Vec::with_capacity(80);
            match crate::managed_launch_protocol::encode_launch_result(&result, &mut bytes) {
                Ok(_) => reply(MultiReply::Response(bytes)),
                Err(_) => reply(MultiReply::Error("invalid_launch_result")),
            }
        }
        Err(error) => self::error(&error),
    }
}

fn dispatch_split(
    owner: &mut MultiSessionOwner<PtyEffects>,
    storage: &mut ServerStorage,
    terminfo: &std::path::Path,
    history: HistoryCapture<'_>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    if crate::split_protocol::decode_split_status_query(payload).is_ok() {
        // A lost split acknowledgment is recovered by resending the exact
        // split bytes, which returns the retained result without effects.
        return reply(MultiReply::Error("split_status_unsupported"));
    }
    let request = match decode_split_request(payload) {
        Ok(request) => request,
        Err(_) => return reply(MultiReply::Error("invalid_split_request")),
    };
    if let Err(error) = owner.authorize_split(connection, &request, now) {
        return self::error(&error);
    }
    let mut journal = match storage.journal() {
        Ok(journal) => journal,
        Err(_) => return reply(MultiReply::Error("launch_journal_unavailable")),
    };
    let result = owner.execute_split(
        payload,
        now,
        &mut journal,
        |request, target| {
            split_prepare(terminfo)(request, target).map(|prepared| (prepared, target))
        },
        |prepared| {
            let (prepared, target) = prepared;
            prepared
                .spawn()
                .map(|session| {
                    let mut effects = PtyEffects::new(session);
                    if let Some(store) = history.store {
                        effects.enable_capture(
                            Arc::clone(store),
                            Arc::clone(history.failure),
                            target,
                        );
                    }
                    effects
                })
                .map_err(|_| ())
        },
        |effects: &mut PtyEffects, rows, columns| effects.resize(rows, columns),
    );
    let result = match result {
        Ok(result) => result,
        Err(error) => match error.failure() {
            Some((_, failed)) => failed,
            None => {
                return reply(MultiReply::Error(match error {
                    crate::server::multi_session::SplitError::Ledger(
                        crate::protocol::ProtocolError::OperationConflict,
                    ) => "operation_conflict",
                    crate::server::multi_session::SplitError::Ledger(_) => "split_ledger_rejected",
                    crate::server::multi_session::SplitError::InProgress => "split_in_progress",
                    crate::server::multi_session::SplitError::GenerationMismatch => {
                        "wrong_generation"
                    }
                    crate::server::multi_session::SplitError::InvalidPayload(_) => {
                        "invalid_split_request"
                    }
                    _ => "split_failed",
                }))
            }
        },
    };
    let mut bytes = Vec::with_capacity(crate::split_protocol::SPLIT_RESULT_BYTES);
    match crate::split_protocol::encode_split_result(&result, &mut bytes) {
        Ok(_) => reply(MultiReply::Response(bytes)),
        Err(_) => reply(MultiReply::Error("invalid_split_result")),
    }
}

/// Schema-9 route: explicit pane close, close status, and pane status.
///
/// A close runs [`MultiSessionOwner::execute_close`] synchronously on the
/// owner thread. Its bounded stop (HUP, 1 s, KILL, 1 s) can therefore block
/// this tick, and every other queued request, for about 2 s. This is accepted
/// for the two-pane prototype; the closing worker waits up to
/// [`CLOSE_REPLY_DEADLINE`] for the reply.
fn dispatch_close(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    if let Ok(query) = decode_close_status_query(payload) {
        return match owner.close_status(connection, query.key, now) {
            Ok(result) => close_reply(&result),
            Err(error) => self::error(&error),
        };
    }
    if decode_pane_query(payload).is_ok() {
        return match owner.pane_view(connection, now) {
            Ok(statuses) => {
                let layout = owner.layout_snapshot();
                let reply_value = PaneReply {
                    layout_revision: layout.map_or(0, |layout| layout.revision),
                    rows: layout.map_or(0, |layout| layout.rows),
                    columns: layout.map_or(0, |layout| layout.columns),
                    panes: statuses.map(|status| {
                        status.map(|status| PaneEntry {
                            target: status.geometry.target,
                            rect: status.geometry.rect,
                            state: status.state,
                            live: status.live,
                            resize_pending: status.resize_pending,
                        })
                    }),
                };
                match encode_pane_reply(&reply_value) {
                    Ok(bytes) => reply(MultiReply::Response(bytes.to_vec())),
                    Err(_) => reply(MultiReply::Error("invalid_pane_status")),
                }
            }
            Err(error) => self::error(&error),
        };
    }
    let request = match decode_close_request(payload) {
        Ok(request) => request,
        Err(_) => return reply(MultiReply::Error("invalid_close_request")),
    };
    if let Err(error) = owner.authorize_close(connection, &request, now) {
        return self::error(&error);
    }
    match owner.execute_close(payload, now, |effects: &mut PtyEffects, rows, columns| {
        effects.resize(rows, columns)
    }) {
        Ok(result) => close_reply(&result),
        Err(error) => reply(MultiReply::Error(close_error_label(&error))),
    }
}

fn close_reply(result: &CloseResult) -> DispatchResult {
    let mut bytes = Vec::with_capacity(CLOSE_RESULT_BYTES);
    match encode_close_result(result, &mut bytes) {
        Ok(_) => reply(MultiReply::Response(bytes)),
        Err(_) => reply(MultiReply::Error("invalid_close_result")),
    }
}

/// Fixed lower-snake-case label for a close that produced no result.
fn close_error_label(error: &CloseError) -> &'static str {
    match error {
        CloseError::InvalidPayload(_) => "invalid_close_request",
        CloseError::GenerationMismatch => "wrong_generation",
        CloseError::Ledger(crate::protocol::ProtocolError::OperationConflict) => {
            "operation_conflict"
        }
        CloseError::Ledger(_) => "close_ledger_rejected",
        CloseError::InProgress => "close_in_progress",
        CloseError::Capacity => "close_capacity",
        CloseError::InternalInvariant => "close_failed",
    }
}

fn dispatch_ui(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    let request = match decode_ui_request(payload) {
        Ok(request) => request,
        Err(_) => return reply(MultiReply::Error("invalid_ui_request")),
    };
    match request {
        UiRequest::LayoutQuery => match owner.layout_view(connection, now) {
            Ok(Some((layout, pending))) => match encode_layout(&layout, &pending) {
                Some(bytes) => reply(MultiReply::Response(bytes.to_vec())),
                None => reply(MultiReply::Error("invalid_layout")),
            },
            Ok(None) => reply(MultiReply::Error("no_layout")),
            Err(error) => self::error(&error),
        },
        UiRequest::ScreenQuery { target } => {
            let captured = owner.observe_target(connection, target, now, |effects| {
                let revision = effects.revision();
                let metadata = crate::screen_checkpoint::CheckpointMetadata::new(
                    target,
                    0,
                    unix_millis(),
                    revision,
                )
                .map_err(|_| "invalid_screen")?;
                let checkpoint = crate::screen_checkpoint::encode(effects.screen(), &metadata)
                    .map_err(|_| "invalid_screen")?;
                crate::ui_protocol::prefix_screen_reply(target, revision, checkpoint)
                    .map_err(|_| "invalid_screen")
            });
            match captured {
                Ok(Ok(bytes)) => reply(MultiReply::Snapshot(bytes)),
                Ok(Err(label)) => reply(MultiReply::Error(label)),
                Err(error) => self::error(&error),
            }
        }
        UiRequest::KeyInput(view) => {
            match owner.handle_key_input(
                connection,
                &view,
                payload,
                now,
                PtyEffects::validate_keys,
                |effects| effects.keys(view.keys()),
            ) {
                Ok(bytes) => reply(MultiReply::Response(bytes)),
                Err(error) => self::error(&error),
            }
        }
        UiRequest::RootResize(resize) => {
            use crate::server::multi_session::RootResizeReply;
            match owner.handle_root_resize(
                connection,
                resize,
                payload,
                now,
                |effects, rows, cols| effects.resize(rows, cols),
            ) {
                Ok(RootResizeReply::Response(bytes)) => reply(MultiReply::Response(bytes)),
                Ok(RootResizeReply::Resized { snapshot, pending }) => {
                    let result = crate::ui_protocol::ResizeReply {
                        key: resize.key,
                        status: if pending.contains(&true) {
                            crate::ui_protocol::ResizeStatus::ResizePending
                        } else {
                            crate::ui_protocol::ResizeStatus::Applied
                        },
                        pending,
                        revision: snapshot.revision,
                        rows: snapshot.rows,
                        columns: snapshot.columns,
                    };
                    match crate::ui_protocol::encode_resize_reply(&result) {
                        Ok(bytes) => reply(MultiReply::Response(bytes.to_vec())),
                        Err(_) => reply(MultiReply::Error("invalid_resize_result")),
                    }
                }
                Ok(RootResizeReply::LayoutRejected(error)) => {
                    reply(MultiReply::Error(match error {
                        crate::server::multi_session::LayoutResizeError::NoLayout => "no_layout",
                        crate::server::multi_session::LayoutResizeError::Layout(
                            crate::pane_layout::LayoutError::StaleRevision,
                        ) => "stale_layout",
                        _ => "layout_rejected",
                    }))
                }
                Err(error) => self::error(&error),
            }
        }
    }
}

fn encode_layout(
    layout: &crate::pane_layout::LayoutSnapshot,
    pending: &[Option<crate::server::multi_session::PendingResize>; 2],
) -> Option<[u8; crate::ui_protocol::LAYOUT_REPLY_BYTES]> {
    let mut panes = [None; 2];
    for (slot, pane) in panes.iter_mut().zip(layout.panes) {
        *slot = pane.map(|pane| crate::ui_protocol::UiPane {
            target: pane.target,
            rect: pane.rect,
            pending_confirmed: pending
                .iter()
                .flatten()
                .find(|entry| entry.target == pane.target)
                .map(|entry| entry.confirmed),
        });
    }
    crate::ui_protocol::encode_layout_reply(&crate::ui_protocol::LayoutReply {
        revision: layout.revision,
        rows: layout.rows,
        columns: layout.columns,
        panes,
        divider: layout.divider,
    })
    .ok()
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

/// Decode a schema-4 watch/reconcile request and map it onto the scoped
/// schema-5 form for its own target.
///
/// A schema-4 cursor has no session field. Every lease here is pinned to one
/// full target, and the owner rejects any other target before reading, so
/// the cursor's session is that target's session. Replies are encoded back in
/// schema 4 (`legacy`), keeping the existing `control reconcile/watch` CLI
/// usable against both servers.
fn decode_legacy_watch(payload: &[u8]) -> Option<WatchRequest> {
    use crate::watch_protocol::WatchRequest as Legacy;
    Some(
        match crate::watch_protocol::decode_watch_request(payload).ok()? {
            Legacy::Reconcile {
                target,
                max_bytes,
                max_lines,
            } => WatchRequest::Reconcile {
                target,
                max_bytes,
                max_lines,
            },
            Legacy::Watch {
                target,
                cursor,
                limit,
                wait_ms,
            } => WatchRequest::Watch {
                target,
                cursor: crate::scoped_watch_protocol::SessionCursor {
                    generation: cursor.generation,
                    session: target.session,
                    sequence: cursor.sequence,
                },
                limit,
                wait_ms,
            },
        },
    )
}

fn dispatch_watch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    request: WatchRequest,
    now: u64,
    pending_capacity: bool,
    legacy: bool,
) -> DispatchResult {
    match request {
        WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        } => match owner.reconcile(connection, target, max_bytes, max_lines, now) {
            Ok(state) => {
                let encoded = if legacy {
                    crate::watch_protocol::encode_reconciliation(
                        &crate::watch_protocol::Reconciliation {
                            info: state.info,
                            cursor: crate::event_journal::EventCursor {
                                generation: state.cursor.generation,
                                sequence: state.cursor.sequence,
                            },
                            screen_revision: state.screen_revision,
                            truncated: state.truncated,
                            text: &state.text,
                        },
                    )
                    .map_err(|_| ())
                } else {
                    crate::scoped_watch_protocol::encode_reconciliation(
                        &crate::scoped_watch_protocol::Reconciliation {
                            info: state.info,
                            cursor: state.cursor,
                            screen_revision: state.screen_revision,
                            truncated: state.truncated,
                            text: &state.text,
                        },
                    )
                    .map_err(|_| ())
                };
                match encoded {
                    Ok(bytes) => reply(MultiReply::Response(bytes)),
                    Err(_) => reply(MultiReply::Error("invalid_reconciliation")),
                }
            }
            Err(error) => self::error(&error),
        },
        WatchRequest::Watch {
            target,
            cursor,
            limit,
            wait_ms,
        } => {
            let deadline = Instant::now().checked_add(Duration::from_millis(u64::from(wait_ms)));
            match owner.read_events(connection, target, cursor, limit, now) {
                Ok(page) => match watch_page_reply(page, wait_ms == 0, false) {
                    Some(value) => reply(value),
                    None if !pending_capacity => reply(MultiReply::Error("watch_capacity")),
                    None => match deadline {
                        Some(deadline) => DispatchResult::Pending { request, deadline },
                        None => reply(MultiReply::Error("watch_deadline")),
                    },
                },
                Err(error) => self::error(&error),
            }
        }
    }
}

fn watch_page_reply(
    page: crate::server::multi_session::ScopedEventPage,
    immediate: bool,
    expired: bool,
) -> Option<MultiReply> {
    let status = if page.resync_required {
        WatchStatus::ResyncRequired
    } else if !page.events.is_empty() {
        WatchStatus::Changed
    } else if immediate || expired {
        WatchStatus::TimedOut
    } else {
        return None;
    };
    let event_count = if status == WatchStatus::Changed {
        u16::try_from(page.events.len()).unwrap_or(u16::MAX)
    } else {
        0
    };
    Some(MultiReply::Watch {
        events: page.events,
        end: WatchEnd {
            status,
            cursor: page.cursor,
            event_count,
        },
    })
}

fn resolve_pending_watch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    pending: &PendingWatch,
    now: u64,
) -> Option<MultiReply> {
    let WatchRequest::Watch {
        target,
        cursor,
        limit,
        ..
    } = pending.request
    else {
        return Some(MultiReply::Error("invalid_watch_request"));
    };
    let MultiRequest::Request { connection, .. } = &pending.envelope.request else {
        return Some(MultiReply::Error("invalid_watch_request"));
    };
    match owner.read_events(connection, target, cursor, limit, now) {
        Ok(page) => watch_page_reply(page, false, Instant::now() >= pending.deadline),
        Err(error) => Some(MultiReply::Error(error.code())),
    }
}

// ---------------------------------------------------------------------------
// Connection worker. Workers parse/validate frames and forward requests only.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum RequestSchema {
    Control,
    Session(SessionRequest),
    /// Scoped watch/reconcile; `legacy` means schema 4 on the wire.
    Watch(WatchRequest, bool),
    Launch,
    Split,
    UiLayout,
    UiScreen(TargetRef),
    UiKeys(OperationKey),
    UiResize(OperationKey),
    /// Schema-9 close request or close status query for this key.
    Close(OperationKey),
    /// Schema-9 pane status query.
    Panes,
}

fn classify(payload: &[u8]) -> Option<RequestSchema> {
    match payload.get(..2)? {
        [0, 1] => crate::control_protocol::decode_request_payload(payload)
            .ok()
            .map(|_| RequestSchema::Control),
        [0, 2] => decode_session_request(payload)
            .ok()
            .map(RequestSchema::Session),
        [0, 4] => decode_legacy_watch(payload).map(|request| RequestSchema::Watch(request, true)),
        [0, 5] => decode_watch_request(payload)
            .ok()
            .map(|request| RequestSchema::Watch(request, false)),
        // Launch kind 1 is rejected by the owner with a fixed label; the
        // worker forwards only well-formed schema-6 shapes.
        [0, 6] => (decode_status_query(payload).is_ok()
            || crate::managed_launch_protocol::decode_launch_request(payload).is_ok())
        .then_some(RequestSchema::Launch),
        [0, 7] => (decode_split_request(payload).is_ok()
            || crate::split_protocol::decode_split_status_query(payload).is_ok())
        .then_some(RequestSchema::Split),
        [0, 8] => match decode_ui_request(payload).ok()? {
            UiRequest::LayoutQuery => Some(RequestSchema::UiLayout),
            UiRequest::ScreenQuery { target } => Some(RequestSchema::UiScreen(target)),
            UiRequest::KeyInput(view) => Some(RequestSchema::UiKeys(view.key())),
            UiRequest::RootResize(resize) => Some(RequestSchema::UiResize(resize.key)),
        },
        [0, 9] => {
            if let Ok(request) = decode_close_request(payload) {
                Some(RequestSchema::Close(request.key))
            } else if let Ok(query) = decode_close_status_query(payload) {
                Some(RequestSchema::Close(query.key))
            } else {
                decode_pane_query(payload)
                    .ok()
                    .map(|()| RequestSchema::Panes)
            }
        }
        _ => None,
    }
}

fn response_matches(schema: RequestSchema, payload: &[u8]) -> bool {
    match schema {
        RequestSchema::Control => decode_response_payload(payload).is_ok(),
        RequestSchema::UiKeys(key) => {
            decode_response_payload(payload).is_ok_and(|response| response.key == key)
        }
        RequestSchema::Session(SessionRequest::Describe) => decode_session_info(payload).is_ok(),
        RequestSchema::Session(SessionRequest::Detach) => false,
        RequestSchema::Session(_) => decode_response_payload(payload).is_ok(),
        RequestSchema::Watch(
            WatchRequest::Reconcile {
                target, max_bytes, ..
            },
            legacy,
        ) => {
            payload.len() <= max_bytes as usize
                && if legacy {
                    crate::watch_protocol::decode_reconciliation(payload)
                        .is_ok_and(|state| state.info.target == target)
                } else {
                    decode_reconciliation(payload).is_ok_and(|state| state.info.target == target)
                }
        }
        RequestSchema::Watch(WatchRequest::Watch { .. }, _) => false,
        RequestSchema::Launch => decode_launch_result(payload).is_ok(),
        RequestSchema::Split => decode_split_result(payload).is_ok(),
        RequestSchema::UiLayout => decode_layout_reply(payload).is_ok(),
        RequestSchema::UiScreen(_) => false,
        RequestSchema::UiResize(key) => {
            decode_resize_reply(payload).is_ok_and(|result| result.key == key)
                || decode_response_payload(payload).is_ok_and(|response| response.key == key)
        }
        RequestSchema::Close(key) => {
            decode_close_result(payload).is_ok_and(|result| result.key == key)
        }
        RequestSchema::Panes => decode_pane_reply(payload).is_ok(),
    }
}

fn snapshot_matches(schema: RequestSchema, payload: &[u8]) -> bool {
    matches!(schema, RequestSchema::UiScreen(target)
        if decode_screen_reply(payload).is_ok_and(|view| view.target() == target))
}

fn split_hello(payload: &[u8]) -> Option<(SessionHello, Option<TargetRef>)> {
    let hello_len = match payload.len() {
        SESSION_HELLO_FRESH_BYTES | SESSION_HELLO_RESUME_BYTES => payload.len(),
        length if length == SESSION_HELLO_FRESH_BYTES + ATTACH_TARGET_BYTES => {
            SESSION_HELLO_FRESH_BYTES
        }
        length if length == SESSION_HELLO_RESUME_BYTES + ATTACH_TARGET_BYTES => {
            SESSION_HELLO_RESUME_BYTES
        }
        _ => return None,
    };
    let hello = decode_session_hello(&payload[..hello_len]).ok()?;
    let target = if hello_len == payload.len() {
        None
    } else {
        Some(decode_attach_target(&payload[hello_len..]).ok()?)
    };
    Some((hello, target))
}

fn run_worker(
    mut stream: AuthenticatedStream,
    sender: &SyncSender<MultiEnvelope>,
    stop: &AtomicBool,
) {
    let Some(frame) = read_frame(&mut stream, stop) else {
        return;
    };
    if frame.kind() != MessageKind::Hello || frame.request_id() != 0 {
        return;
    }
    let Some((hello, target)) = split_hello(frame.payload()) else {
        return;
    };
    drop(frame);
    let Some(answer) = submit(
        sender,
        stop,
        MultiRequest::Hello { hello, target },
        OWNER_REPLY_DEADLINE,
    ) else {
        return;
    };
    let (connection, welcome) = match answer {
        MultiReply::Welcome {
            connection,
            welcome,
        } => (connection, welcome),
        MultiReply::Error(label) => {
            let _ = write_error(&mut stream, 0, label, stop);
            return;
        }
        _ => return,
    };
    if connection.generation() != welcome.generation
        || connection.lease() != welcome.lease
        || connection.selected_capabilities() != welcome.selected_capabilities
        || validate_welcome(&hello.hello, &welcome).is_err()
    {
        return;
    }
    if welcome.selected_capabilities & CAPABILITY_EVENTS != 0
        && stream.enable_metadata_events().is_err()
    {
        return;
    }
    if welcome.selected_capabilities & CAPABILITY_READ != 0
        && stream.enable_snapshot_writes().is_err()
    {
        return;
    }
    let Ok(welcome_payload) = encode_welcome(&welcome) else {
        return;
    };
    if write_frame(&mut stream, MessageKind::Welcome, 0, &welcome_payload, stop).is_err() {
        return;
    }

    let mut last_request_id = 0;
    while !stop.load(Ordering::Acquire) {
        let Some(frame) = read_frame(&mut stream, stop) else {
            return;
        };
        if frame.kind() != MessageKind::Request
            || frame.request_id() == 0
            || frame.request_id() <= last_request_id
        {
            return;
        }
        let request_id = frame.request_id();
        last_request_id = request_id;
        let Some(schema) = classify(frame.payload()) else {
            if write_error(&mut stream, request_id, "invalid_request", stop).is_err() {
                return;
            }
            continue;
        };
        if matches!(schema, RequestSchema::Watch(..))
            && connection.selected_capabilities() & (CAPABILITY_READ | CAPABILITY_EVENTS)
                != (CAPABILITY_READ | CAPABILITY_EVENTS)
        {
            if write_error(&mut stream, request_id, "capability_denied", stop).is_err() {
                return;
            }
            continue;
        }
        // A close's bounded stop runs inside the owner tick; wait longer.
        let reply_deadline = if matches!(schema, RequestSchema::Close(_)) {
            CLOSE_REPLY_DEADLINE
        } else {
            OWNER_REPLY_DEADLINE
        };
        let Some(answer) = submit(
            sender,
            stop,
            MultiRequest::Request {
                connection,
                payload: frame.into_payload(),
            },
            reply_deadline,
        ) else {
            return;
        };
        let written = match answer {
            MultiReply::Response(payload) if response_matches(schema, &payload) => write_frame(
                &mut stream,
                MessageKind::Response,
                request_id,
                &payload,
                stop,
            ),
            MultiReply::Snapshot(payload) if snapshot_matches(schema, &payload) => {
                write_frame_until(
                    &mut stream,
                    MessageKind::Snapshot,
                    request_id,
                    &payload,
                    Instant::now() + SNAPSHOT_WRITE_DEADLINE,
                    stop,
                )
            }
            MultiReply::Watch { events, end } => match schema {
                RequestSchema::Watch(WatchRequest::Watch { target, .. }, legacy) => {
                    write_watch(&mut stream, request_id, target, &events, &end, legacy, stop)
                }
                _ => return,
            },
            MultiReply::Error(label) => write_error(&mut stream, request_id, label, stop),
            MultiReply::Detached
                if matches!(schema, RequestSchema::Session(SessionRequest::Detach)) =>
            {
                let _ = write_frame(
                    &mut stream,
                    MessageKind::Response,
                    request_id,
                    &DETACHED_ACK,
                    stop,
                );
                return;
            }
            _ => return,
        };
        if written.is_err() {
            return;
        }
    }
}

fn write_watch(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    target: TargetRef,
    events: &[Event],
    end: &WatchEnd,
    legacy: bool,
    stop: &AtomicBool,
) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_DEADLINE;
    if usize::from(end.event_count) != events.len()
        || events.iter().any(|event| event.target != target)
        || end.cursor.session != target.session
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "watch batch"));
    }
    let encoded_end = if legacy {
        crate::watch_protocol::encode_watch_end(&crate::watch_protocol::WatchEnd {
            status: end.status,
            cursor: crate::event_journal::EventCursor {
                generation: end.cursor.generation,
                sequence: end.cursor.sequence,
            },
            event_count: end.event_count,
        })
        .map_err(|_| ())
    } else {
        encode_watch_end(end, target).map_err(|_| ())
    }
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "watch end"))?;
    for event in events {
        let payload = encode_event(event)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "watch event"))?;
        write_frame_until(stream, MessageKind::Event, 0, &payload, deadline, stop)?;
    }
    write_frame_until(
        stream,
        MessageKind::Response,
        request_id,
        &encoded_end,
        deadline,
        stop,
    )
}

fn submit(
    sender: &SyncSender<MultiEnvelope>,
    stop: &AtomicBool,
    request: MultiRequest,
    reply_deadline: Duration,
) -> Option<MultiReply> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
    let envelope = MultiEnvelope {
        request,
        reply: reply_sender,
    };
    if sender.try_send(envelope).is_err() {
        return None;
    }
    let deadline = Instant::now().checked_add(reply_deadline)?;
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match reply_receiver.recv_timeout(remaining.min(STOP_POLL_INTERVAL)) {
            Ok(answer) => return Some(answer),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn read_frame(stream: &mut AuthenticatedStream, stop: &AtomicBool) -> Option<OwnedFrame> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    stream
        .read_frame(Instant::now().checked_add(READ_DEADLINE)?)
        .ok()
}

fn write_frame(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
    stop: &AtomicBool,
) -> io::Result<()> {
    write_frame_until(
        stream,
        kind,
        request_id,
        payload,
        Instant::now() + WRITE_DEADLINE,
        stop,
    )
}

fn write_frame_until(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "stopping"));
    }
    let frame = Frame::new(kind, request_id, payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    stream.write_frame(&frame, deadline)
}

fn write_error(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    label: &'static str,
    stop: &AtomicBool,
) -> io::Result<()> {
    if label.is_empty()
        || label.len() > MAX_ERROR_LABEL_BYTES
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "error label"));
    }
    write_frame(
        stream,
        MessageKind::Error,
        request_id,
        label.as_bytes(),
        stop,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_journal::EventCursor;

    const TARGET: TargetRef = TargetRef {
        generation: 3,
        workspace: 1,
        pane: 2,
        session: 2,
        occupant_revision: 1,
    };

    const KEY: OperationKey = OperationKey {
        generation: 3,
        lease: 4,
        sequence: 5,
    };

    fn close_request_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        crate::close_protocol::encode_close_request(
            &crate::close_protocol::CloseRequest {
                key: KEY,
                target: TARGET,
                expected_layout_revision: 2,
            },
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn sample_pane_reply() -> PaneReply {
        PaneReply {
            layout_revision: 7,
            rows: 24,
            columns: 81,
            panes: [
                Some(PaneEntry {
                    target: TARGET,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    state: SessionState::Applied,
                    live: true,
                    resize_pending: false,
                }),
                Some(PaneEntry {
                    target: TargetRef {
                        pane: 3,
                        session: 3,
                        ..TARGET
                    },
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    state: SessionState::Exited,
                    live: false,
                    resize_pending: true,
                }),
            ],
        }
    }

    #[test]
    fn schema_nine_close_status_and_pane_queries_are_classified_and_matched() {
        let request = close_request_bytes();
        assert!(matches!(
            classify(&request),
            Some(RequestSchema::Close(key)) if key == KEY
        ));
        let mut query = Vec::new();
        crate::close_protocol::encode_close_status_query(
            crate::close_protocol::CloseStatusQuery { key: KEY },
            &mut query,
        )
        .unwrap();
        assert!(matches!(
            classify(&query),
            Some(RequestSchema::Close(key)) if key == KEY
        ));
        assert!(matches!(
            classify(&encode_pane_query()),
            Some(RequestSchema::Panes)
        ));
        // Malformed schema-9 shapes never reach the owner.
        let mut truncated = request.clone();
        truncated.pop();
        assert!(classify(&truncated).is_none());
        let mut reserved = request;
        reserved[79] = 1;
        assert!(classify(&reserved).is_none());
        let mut pane = encode_pane_query();
        pane[7] = 1;
        assert!(classify(&pane).is_none());

        // Only a close result for exactly this key is forwarded.
        let result = CloseResult {
            key: KEY,
            outcome: crate::close_protocol::CloseOutcome::Stopped(
                crate::close_protocol::StopOutcome::Escalated,
            ),
            target: Some(TARGET),
            layout_revision: 3,
            survivor_resize_pending: false,
        };
        let mut bytes = Vec::new();
        encode_close_result(&result, &mut bytes).unwrap();
        assert!(response_matches(RequestSchema::Close(KEY), &bytes));
        let other = OperationKey { sequence: 6, ..KEY };
        assert!(!response_matches(RequestSchema::Close(other), &bytes));
        assert!(!response_matches(RequestSchema::Panes, &bytes));
        let panes = encode_pane_reply(&sample_pane_reply()).unwrap();
        assert!(response_matches(RequestSchema::Panes, &panes));
        assert!(!response_matches(RequestSchema::Close(KEY), &panes));
    }

    #[test]
    fn pane_reply_round_trips_and_rejects_malformed_bytes() {
        let reply = sample_pane_reply();
        let bytes = encode_pane_reply(&reply).unwrap();
        assert_eq!(bytes.len(), PANE_REPLY_BYTES);
        assert_eq!(decode_pane_reply(&bytes), Ok(reply));

        let single = PaneReply {
            panes: [reply.panes[0], None],
            ..reply
        };
        let bytes = encode_pane_reply(&single).unwrap();
        assert_eq!(decode_pane_reply(&bytes), Ok(single));
        assert!(bytes[80..].iter().all(|byte| *byte == 0));

        let empty = PaneReply {
            layout_revision: 0,
            rows: 0,
            columns: 0,
            panes: [None, None],
        };
        assert_eq!(
            decode_pane_reply(&encode_pane_reply(&empty).unwrap()),
            Ok(empty)
        );
        // A revision without panes, or panes without a revision, is invalid.
        assert!(encode_pane_reply(&PaneReply {
            layout_revision: 1,
            ..empty
        })
        .is_err());
        assert!(encode_pane_reply(&PaneReply {
            layout_revision: 0,
            ..reply
        })
        .is_err());

        let good = encode_pane_reply(&reply).unwrap();
        for len in 0..PANE_REPLY_BYTES {
            assert!(decode_pane_reply(&good[..len]).is_err(), "length {len}");
        }
        assert!(decode_pane_reply(&good[..PANE_REPLY_BYTES - 1]).is_err());
        for (offset, value) in [
            (1, 8),
            (3, 1),
            (5, 1),
            (21, 1),
            (24 + 48, 9),
            (24 + 49, 8),
            (24 + 50, 1),
            (24, 0),
        ] {
            let mut bad = good;
            if offset == 24 {
                bad[24..32].fill(0);
            } else {
                bad[offset] = value;
            }
            assert!(decode_pane_reply(&bad).is_err(), "offset {offset}");
        }
        // Absent entries must be all zero.
        let mut bad = encode_pane_reply(&single).unwrap();
        bad[100] = 1;
        assert!(decode_pane_reply(&bad).is_err());
        // Debug never prints the target identity.
        let debug = format!("{:?}", reply.panes[0].unwrap());
        assert!(debug.contains("<redacted>") && !debug.contains("generation"));
    }

    #[test]
    fn close_errors_map_to_fixed_lower_snake_case_labels() {
        for (error, label) in [
            (
                CloseError::InvalidPayload(crate::close_protocol::CloseCodecError::Truncated),
                "invalid_close_request",
            ),
            (CloseError::GenerationMismatch, "wrong_generation"),
            (
                CloseError::Ledger(crate::protocol::ProtocolError::OperationConflict),
                "operation_conflict",
            ),
            (
                CloseError::Ledger(crate::protocol::ProtocolError::LeaseExpired),
                "close_ledger_rejected",
            ),
            (CloseError::InProgress, "close_in_progress"),
            (CloseError::Capacity, "close_capacity"),
            (CloseError::InternalInvariant, "close_failed"),
        ] {
            assert_eq!(close_error_label(&error), label);
            assert!(label.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        }
    }

    #[test]
    fn schema_four_watch_maps_onto_the_target_session_and_is_classified_legacy() {
        let legacy = crate::watch_protocol::encode_watch_request(
            &crate::watch_protocol::WatchRequest::Watch {
                target: TARGET,
                cursor: EventCursor {
                    generation: 3,
                    sequence: 7,
                },
                limit: 4,
                wait_ms: 10,
            },
        )
        .unwrap();
        let Some(WatchRequest::Watch { target, cursor, .. }) = decode_legacy_watch(&legacy) else {
            panic!("legacy watch maps");
        };
        assert_eq!(target, TARGET);
        assert_eq!(
            (cursor.generation, cursor.session, cursor.sequence),
            (3, TARGET.session, 7)
        );
        assert!(matches!(
            classify(&legacy),
            Some(RequestSchema::Watch(WatchRequest::Watch { .. }, true))
        ));
        let scoped = crate::scoped_watch_protocol::encode_watch_request(&WatchRequest::Watch {
            target: TARGET,
            cursor,
            limit: 4,
            wait_ms: 10,
        })
        .unwrap();
        assert!(matches!(
            classify(&scoped),
            Some(RequestSchema::Watch(WatchRequest::Watch { .. }, false))
        ));
        let reconcile = crate::watch_protocol::encode_watch_request(
            &crate::watch_protocol::WatchRequest::Reconcile {
                target: TARGET,
                max_bytes: 4_096,
                max_lines: 24,
            },
        )
        .unwrap();
        assert!(matches!(
            classify(&reconcile),
            Some(RequestSchema::Watch(WatchRequest::Reconcile { .. }, true))
        ));
        let mut truncated = legacy.clone();
        truncated.pop();
        assert!(decode_legacy_watch(&truncated).is_none());
        assert!(classify(&truncated).is_none());
    }
}
