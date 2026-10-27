//! Bounded multi-session runtime: authenticated connection workers forward
//! validated requests to the single owner thread, which alone touches PTYs.
//!
//! This is a parallel runtime beside the one-child [`crate::server_runtime`].
//! Connection workers never touch PTYs. One separate serialized launch worker
//! prepares/spawns new children and performs launch-journal IO; the driver
//! adopts every child before final journal acknowledgment. Layout, existing
//! PTYs, key delivery and screen capture remain owner-thread work.
//! Connection loss never cancels admitted
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
    encode_welcome, validate_welcome, Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT,
    CAPABILITY_READ, CAPABILITY_WORKSPACE_EVENTS,
};
use crate::history_capture::{lock_capture, HistoryCaptureCoordinator, SharedHistoryCapture};
use crate::history_diagnostics::{HistoryDiagnostics, HistoryReason};
use crate::history_store::HistoryStore;
use crate::history_writer::{HistoryWriter, MAX_SHUTDOWN_WAIT};
use crate::launch_worker::{LaunchWorkResult, LaunchWorker};
use crate::layout_store::LayoutStore;
use crate::layout_writer::{LayoutShutdownOutcome, LayoutStatusHandle, LayoutWriter};
use crate::managed_launch_protocol::{decode_launch_result, decode_status_query};
use crate::pane_layout::LayoutSnapshot;
use crate::pane_layout::Rect;
use crate::pane_metadata_protocol::{self as pane_metadata, Observation, PathStatus};
use crate::process_metadata::{MetadataJob, MetadataWorker, ProcessIdentity};
use crate::protocol::OperationKey;
use crate::protocol_wire::{Frame, MessageKind};
use crate::response_protocol::decode_response_payload;
use crate::saved_layout::{LayoutMetadata, SavedLayout};
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
    decode_attach_target, decode_history_reply_with_wraps, decode_layout_reply,
    decode_resize_reply, decode_screen_reply_with_wraps, decode_ui_request, UiRequest,
    ATTACH_TARGET_BYTES,
};
use crate::watch_protocol::WatchStatus;
use crate::workspace_watch_protocol as workspace_watch;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Confirmed child termination reported by the operating system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneExit {
    /// The child returned an exit code.
    Code(i32),
    /// The child was terminated by this signal.
    Signal(i32),
}

#[cfg(test)]
mod launch_tests;

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod metadata_tests;

#[cfg(test)]
mod workspace_adapter_tests {
    use super::*;

    #[test]
    fn workspace_watch_uses_the_shared_pending_budget() {
        assert!(watch_capacity(4, 3));
        assert!(!watch_capacity(7, 1));
        assert!(!watch_capacity(0, MAX_PENDING_WATCHES));
    }

    #[test]
    fn response_preflight_rejects_page_that_skips_requested_cursor() {
        let requested = workspace_watch::WorkspaceCursor {
            generation: 2,
            workspace: 3,
            sequence: 7,
        };
        let request = workspace_watch::Request::Watch {
            cursor: requested,
            limit: 8,
            wait_ms: 0,
        };
        let skipped = workspace_watch::WatchReply {
            status: WatchStatus::Changed,
            cursor: workspace_watch::WorkspaceCursor {
                sequence: 9,
                ..requested
            },
            events: vec![workspace_watch::LayoutEvent {
                sequence: 9,
                revision: 4,
                empty: false,
            }],
        };
        let bytes = workspace_watch::encode_watch_reply(&skipped).unwrap();
        assert!(!workspace_response_matches(request, &bytes));

        let consecutive = workspace_watch::WatchReply {
            status: WatchStatus::Changed,
            cursor: workspace_watch::WorkspaceCursor {
                sequence: 8,
                ..requested
            },
            events: vec![workspace_watch::LayoutEvent {
                sequence: 8,
                revision: 4,
                empty: false,
            }],
        };
        let bytes = workspace_watch::encode_watch_reply(&consecutive).unwrap();
        assert!(workspace_response_matches(request, &bytes));
    }
}

/// Maximum finite watches retained between owner ticks.
pub const MAX_PENDING_WATCHES: usize = 8;
/// Maximum close replies retained while owner-driven stop phases progress.
pub const MAX_PENDING_CLOSES: usize = MAX_CONNECTIONS;

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
    /// Encoded schema-11 workspace reconciliation or watch response.
    WorkspaceResponse(Vec<u8>),
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
            Self::WorkspaceResponse(bytes) => {
                write!(f, "WorkspaceResponse({} bytes)", bytes.len())
            }
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
    pending_workspace_watches: Vec<PendingWorkspaceWatch>,
    pending_closes: Vec<PendingClose>,
    pending_splits: Vec<PendingSplitReply>,
    metadata_worker: Option<MetadataWorker>,
    metadata_origin: Instant,
    metadata_busy: bool,
    metadata_pending: Option<PendingMetadata>,
    next_metadata_id: u64,
    launch_worker: Option<LaunchWorker>,
    active_launch_target: Option<TargetRef>,
    stopping: bool,
    owner: MultiSessionOwner<PtyEffects>,
    storage: ServerStorage,
    history_capture: Option<SharedHistoryCapture>,
    history_failure: Arc<AtomicBool>,
    history_diagnostics: HistoryDiagnostics,
    layout: Option<LayoutObserver>,
}

/// The prototype's single default workspace identity.
const DEFAULT_WORKSPACE: u64 = 1;

/// Canonical saved-topology key: `LayoutSnapshot` value equality (revision,
/// root size, spatial targets/rects, divider) DEFINES a canonical change.
/// Desired geometry and targets only. Focus,
/// pending PTY resize and repeated identical results never form a change.
#[derive(Clone, Copy, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
enum LayoutCandidateKey {
    Live(LayoutSnapshot),
    Empty,
}

/// Owner-thread saved-layout observation (P5-SAVED-LAYOUT.md). Emits only on
/// a changed canonical topology/geometry or the terminal-empty transition,
/// with a checked per-generation snapshot sequence. Failures disable saving
/// only; shells and history are unaffected.
struct LayoutObserver {
    writer: Option<LayoutWriter>,
    status: LayoutStatusHandle,
    generation: u64,
    last: Option<LayoutCandidateKey>,
    sequence: u64,
    disabled: bool,
    /// Set before any driver-shutdown stop effect; later exits are not empty.
    frozen: bool,
}

impl LayoutObserver {
    fn observe(&mut self, owner: &MultiSessionOwner<PtyEffects>, now_unix_ms: u64) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if self.disabled || self.frozen {
            let _ = writer.flush();
            return;
        }
        let Some(snapshot) = owner.layout_snapshot() else {
            let _ = writer.flush();
            return;
        };
        let key = if owner.layout_terminal_empty() {
            LayoutCandidateKey::Empty
        } else {
            LayoutCandidateKey::Live(snapshot)
        };
        if self.last == Some(key) || self.last == Some(LayoutCandidateKey::Empty) {
            // Unchanged, or terminal empty already published for this generation.
            let _ = writer.flush();
            return;
        }
        // Checked sequence exhaustion is a terminal disable for the rest of
        // this generation (never wraps or reuses a key); shells continue.
        let Some(sequence) = self.sequence.checked_add(1) else {
            self.disabled = true;
            return;
        };
        let candidate = LayoutMetadata::new(
            self.generation,
            DEFAULT_WORKSPACE,
            sequence,
            snapshot.revision,
            now_unix_ms,
        )
        .and_then(|metadata| match key {
            LayoutCandidateKey::Empty => Ok(SavedLayout::empty(metadata)),
            LayoutCandidateKey::Live(snapshot) => SavedLayout::from_snapshot(metadata, &snapshot),
        });
        let Ok(candidate) = candidate else {
            // Missing clock or a snapshot outside the saved format: keep the
            // last good record, stop publishing for this generation.
            self.disabled = true;
            return;
        };
        self.sequence = sequence;
        self.last = Some(key);
        let _ = writer.observe(candidate);
    }
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
        self.stopping = true;
        // Request cancellation now; the bounded wait happens at the common
        // metadata-drain boundary after owned-child shutdown. The worker keeps
        // exclusion through any late IO and unclaimed-child cleanup.
        let launch_worker = self.launch_worker.take();
        if let Some(worker) = &launch_worker {
            worker.request_shutdown();
        }
        self.hub.shutdown();
        self.settle_pending_watches();
        self.settle_pending_workspace_watches();
        self.settle_pending_closes();
        self.settle_pending_splits();
        self.settle_pending_metadata();
        // Capture and freeze the pre-stop topology so shutdown-caused exits
        // cannot replace it with an empty tombstone.
        self.observe_layout();
        if let Some(observer) = self.layout.as_mut() {
            observer.frozen = true;
        }
        self.owner
            .for_each_effects_mut(|_, effects| effects.prepare_driver_shutdown());
        let shutdown_started = Instant::now();
        let hup_deadline = shutdown_started + Duration::from_secs(1);
        let kill_deadline = hup_deadline + Duration::from_secs(1);
        loop {
            let now = Instant::now();
            let kill = now >= hup_deadline;
            self.owner.for_each_effects_mut(|_, effects| {
                effects.advance_driver_shutdown(kill);
            });
            // Close transactions retain their result and continue progressing
            // alongside the shared shutdown of all child processes.
            let _ = self.owner.poll(0);
            let _ = self.owner.advance_closes_at(now);
            let mut complete = true;
            self.owner.for_each_effects_mut(|_, effects| {
                complete &= effects.driver_shutdown_complete();
            });
            if complete || now >= kill_deadline {
                break;
            }
            thread::sleep(
                Duration::from_millis(10).min(kill_deadline.saturating_duration_since(now)),
            );
        }
        // History, layout and launch-worker shutdown share one bounded wait
        // budget after the child shutdown window.
        let deadline = Instant::now() + MAX_SHUTDOWN_WAIT;
        let layout_writer = self
            .layout
            .as_mut()
            .and_then(|observer| observer.writer.take());
        let layout_outcome = layout_writer.map(|writer| writer.begin_shutdown());
        if let Some(coordinator) = self.history_capture.take() {
            lock_capture(&coordinator).begin_shutdown();
            let reached_deadline = loop {
                let settled = {
                    let mut capture = lock_capture(&coordinator);
                    capture.drive();
                    capture.all_targets_settled()
                };
                let now = Instant::now();
                if settled || now >= deadline {
                    break !settled && now >= deadline;
                }
                thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            };
            let writer = {
                let mut capture = lock_capture(&coordinator);
                if reached_deadline {
                    capture.note_drain_deadline();
                }
                capture.take_writer_for_shutdown()
            };
            if let Some(writer) = writer {
                let outcome = writer
                    .shutdown(deadline.saturating_duration_since(Instant::now()))
                    .outcome;
                lock_capture(&coordinator).note_shutdown_outcome(outcome);
            }
        }
        if let Some(pending) = layout_outcome {
            let _: LayoutShutdownOutcome = pending.wait_until(deadline);
        }
        if let Some(worker) = launch_worker {
            if !worker.shutdown_until(deadline) {
                eprintln!("panelink: launch shutdown incomplete");
            }
        }
    }
}

impl MultiServerDriver {
    /// Assemble a driver from an owner whose generation matches `storage`.
    ///
    /// `terminfo` is the explicit server-owned terminfo directory used by
    /// split launch preparation. The serialized launch worker is started here,
    /// before the live owner loop; no listener is created.
    pub fn new(
        owner: MultiSessionOwner<PtyEffects>,
        storage: ServerStorage,
        terminfo: PathBuf,
    ) -> Self {
        let launch_worker = LaunchWorker::start(&storage, terminfo.clone()).ok();
        let metadata_origin = Instant::now();
        Self {
            hub: Hub::new(),
            pending_watches: Vec::with_capacity(MAX_PENDING_WATCHES),
            pending_workspace_watches: Vec::with_capacity(MAX_PENDING_WATCHES),
            pending_closes: Vec::with_capacity(MAX_PENDING_CLOSES),
            pending_splits: Vec::with_capacity(MAX_CONNECTIONS),
            metadata_worker: MetadataWorker::start(metadata_origin).ok(),
            metadata_origin,
            metadata_busy: false,
            metadata_pending: None,
            next_metadata_id: 1,
            launch_worker,
            active_launch_target: None,
            stopping: false,
            owner,
            storage,
            history_capture: None,
            history_failure: Arc::new(AtomicBool::new(false)),
            history_diagnostics: HistoryDiagnostics::new(),
            layout: None,
        }
    }

    /// Opt in to bounded saved-layout snapshots for this server generation.
    ///
    /// `store` must be this storage domain's `layout/` directory. Failure
    /// leaves shells and history running. Read-only inspection never needs
    /// this; it never restores or relaunches anything.
    pub fn enable_layout(&mut self, store: LayoutStore) -> io::Result<()> {
        if self.layout.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "layout capture already enabled",
            ));
        }
        let writer = LayoutWriter::start(store, &self.storage, DEFAULT_WORKSPACE)?;
        self.layout = Some(LayoutObserver {
            status: writer.status_handle(),
            writer: Some(writer),
            generation: self.storage.generation(),
            last: None,
            sequence: 0,
            disabled: false,
            frozen: false,
        });
        self.observe_layout();
        Ok(())
    }

    /// Status handle readable after the driver drops; `None` if not enabled.
    pub fn layout_status(&self) -> Option<LayoutStatusHandle> {
        self.layout.as_ref().map(|observer| observer.status.clone())
    }

    fn observe_layout(&mut self) {
        if let Some(observer) = self.layout.as_mut() {
            observer.observe(&self.owner, unix_millis());
        }
    }

    /// Opt in to private checkpoint capture for this server generation.
    /// Existing and subsequently split sessions share one owner-thread writer.
    pub fn enable_history(&mut self, store: HistoryStore) {
        if self.history_capture.is_some() {
            self.history_diagnostics
                .record(HistoryReason::CaptureInvalid);
            self.history_failure.store(true, Ordering::Release);
            return;
        }
        let writer = match HistoryWriter::start_with_diagnostics(
            store,
            &self.storage,
            self.history_diagnostics.clone(),
        ) {
            Ok(writer) => writer,
            Err(_) => {
                self.history_failure.store(true, Ordering::Release);
                return;
            }
        };
        self.install_history_writer(writer);
    }

    fn install_history_writer(&mut self, writer: HistoryWriter) {
        if self.history_capture.is_some() {
            self.history_diagnostics
                .record(HistoryReason::CaptureInvalid);
            self.history_failure.store(true, Ordering::Release);
            return;
        }
        let coordinator = Arc::new(Mutex::new(HistoryCaptureCoordinator::new(
            writer,
            Arc::clone(&self.history_failure),
        )));
        self.owner.for_each_effects_mut(|target, effects| {
            effects.enable_async_capture(
                Arc::clone(&coordinator),
                Arc::clone(&self.history_failure),
                target,
            );
        });
        self.history_capture = Some(coordinator);
    }

    #[cfg(test)]
    pub(crate) fn enable_history_backend_for_test(
        &mut self,
        backend: Box<dyn crate::history_writer::HistoryBackend>,
    ) {
        if self.history_capture.is_some() {
            self.history_diagnostics
                .record(HistoryReason::CaptureInvalid);
            self.history_failure.store(true, Ordering::Release);
            return;
        }
        match HistoryWriter::start_with_backend_and_diagnostics(
            backend,
            &self.storage,
            self.history_diagnostics.clone(),
        ) {
            Ok(writer) => self.install_history_writer(writer),
            Err(_) => self.history_failure.store(true, Ordering::Release),
        }
    }

    /// Force a capture without interrupting any PTY if storage fails.
    pub fn capture_now(&mut self) {
        self.owner
            .for_each_effects_mut(|_, effects| effects.capture_now());
        self.drive_history();
    }

    /// Sticky failure indicator, including panes retired after a failed write.
    pub fn history_capture_failed(&self) -> bool {
        self.history_failure.load(Ordering::Relaxed)
    }

    /// Sticky capture failure signal shared with retired panes.
    pub fn history_failure_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.history_failure)
    }

    /// Return a stable diagnostic handle, including before history is enabled.
    pub fn history_diagnostics(&self) -> HistoryDiagnostics {
        self.history_diagnostics.clone()
    }

    /// Whether a private checkpoint writer was opened for this server.
    pub fn history_available(&self) -> bool {
        self.history_capture.is_some()
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
        self.drive_history();
        let progress = self.owner.poll(clock());
        self.advance_metadata(&clock);
        // Close ownership belongs to the driver, never to the bounded worker
        // reply lifetime. Advance even when the original worker timed out.
        let _ = self.owner.advance_closes_at(Instant::now());
        self.advance_launch();
        self.process_pending(&clock);
        self.process_pending_splits();
        self.process_pending_closes(&clock);
        self.process_pending_watches(&clock);
        self.process_pending_workspace_watches(&clock);
        self.hub.reap_finished();
        self.drive_history();
        self.observe_layout();
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
        self.pending_watches.len() + self.pending_workspace_watches.len()
    }

    /// Stop workers and settle queued requests; children stay alive until
    /// the driver drops.
    pub fn stop_connections(&mut self, clock: impl Fn() -> u64) {
        self.stopping = true;
        self.hub.shutdown();
        self.process_pending(&clock);
        self.settle_pending_watches();
        self.settle_pending_workspace_watches();
        self.settle_pending_closes();
        self.settle_pending_splits();
        self.settle_pending_metadata();
    }

    fn process_pending(&mut self, clock: &impl Fn() -> u64) {
        for _ in 0..OWNER_QUEUE_CAPACITY {
            let Ok(envelope) = self.hub.receiver.try_recv() else {
                break;
            };
            let metadata = match &envelope.request {
                MultiRequest::Request {
                    connection,
                    payload,
                } if payload.get(..2) == Some(&[0, 12]) => Some((*connection, payload.clone())),
                _ => None,
            };
            if let Some((connection, payload)) = metadata {
                self.begin_metadata(envelope, connection, &payload, clock());
                continue;
            }
            let result = if matches!(&envelope.request, MultiRequest::Request { payload, .. } if payload.get(..2) == Some(&[0, 7]))
            {
                self.dispatch_async_split(&envelope.request, clock())
            } else {
                dispatch(
                    &mut self.owner,
                    &envelope.request,
                    clock(),
                    PendingCapacity {
                        watches: watch_capacity(
                            self.pending_watches.len(),
                            self.pending_workspace_watches.len(),
                        ),
                        closes: self.pending_closes.len() < MAX_PENDING_CLOSES,
                    },
                )
            };
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
                DispatchResult::PendingWorkspaceWatch { request, deadline } => {
                    self.pending_workspace_watches.push(PendingWorkspaceWatch {
                        envelope,
                        request,
                        deadline,
                    });
                }
                DispatchResult::PendingClose { key, deadline } => {
                    self.pending_closes.push(PendingClose {
                        envelope,
                        key,
                        deadline,
                    });
                }
                DispatchResult::PendingSplit { key, deadline } => {
                    self.pending_splits.push(PendingSplitReply {
                        envelope,
                        key,
                        deadline,
                    });
                }
            }
        }
    }

    fn begin_metadata(
        &mut self,
        envelope: MultiEnvelope,
        connection: Connection,
        payload: &[u8],
        now: u64,
    ) {
        if pane_metadata::decode_query(payload).is_err() {
            let _ = envelope.try_reply(MultiReply::Error("invalid_metadata_request"));
            return;
        }
        let (target, identity, unavailable_status) =
            match self
                .owner
                .authorize_metadata_query(&connection, payload, now)
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = envelope.try_reply(MultiReply::Error(error.code()));
                    return;
                }
            };
        let Some(identity) = identity else {
            let observation =
                Observation::unavailable(target, self.metadata_now_ms(), unavailable_status);
            self.reply_metadata(envelope, observation);
            return;
        };
        let Some(worker) = self.metadata_worker.as_ref() else {
            self.reply_metadata(
                envelope,
                Observation::unavailable(target, self.metadata_now_ms(), PathStatus::Unsupported),
            );
            return;
        };
        if self.metadata_busy {
            self.reply_metadata(
                envelope,
                Observation::unavailable(target, self.metadata_now_ms(), PathStatus::Busy),
            );
            return;
        }
        let id = self.next_metadata_id;
        let Some(next_id) = id.checked_add(1) else {
            self.reply_metadata(
                envelope,
                Observation::unavailable(target, self.metadata_now_ms(), PathStatus::Busy),
            );
            return;
        };
        self.next_metadata_id = next_id;
        if worker
            .submit(MetadataJob {
                id,
                target,
                identity,
            })
            .is_err()
        {
            self.reply_metadata(
                envelope,
                Observation::unavailable(target, self.metadata_now_ms(), PathStatus::Busy),
            );
            return;
        }
        self.metadata_busy = true;
        self.metadata_pending = Some(PendingMetadata {
            envelope,
            connection,
            target,
            identity,
            id,
            deadline: Instant::now() + Duration::from_millis(500),
        });
    }

    fn advance_metadata(&mut self, clock: &impl Fn() -> u64) {
        if self
            .metadata_pending
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.deadline)
        {
            let pending = self.metadata_pending.take().expect("pending checked");
            self.reply_metadata(
                pending.envelope,
                Observation::unavailable(
                    pending.target,
                    self.metadata_now_ms(),
                    PathStatus::TimedOut,
                ),
            );
            // Keep metadata_busy set until the worker's late completion arrives.
        }
        let completion = match self.metadata_worker.as_ref().map(MetadataWorker::poll) {
            Some(Ok(completion)) => completion,
            Some(Err(())) => {
                self.metadata_worker = None;
                self.metadata_busy = false;
                if let Some(pending) = self.metadata_pending.take() {
                    self.reply_metadata(
                        pending.envelope,
                        Observation::unavailable(
                            pending.target,
                            self.metadata_now_ms(),
                            PathStatus::QueryFailed,
                        ),
                    );
                }
                None
            }
            None => None,
        };
        if let Some(completion) = completion {
            self.metadata_busy = false;
            if let Some(pending) = self.metadata_pending.take() {
                if completion.id == pending.id {
                    let observation = if self.owner.metadata_publish_allowed(
                        &pending.connection,
                        pending.target,
                        pending.identity,
                        clock(),
                    ) && completion.observation.target == pending.target
                    {
                        completion.observation
                    } else {
                        Observation::unavailable(
                            pending.target,
                            completion.observation.sampled_at_mono_ms,
                            PathStatus::IdentityMismatch,
                        )
                    };
                    self.reply_metadata(pending.envelope, observation);
                } else {
                    self.reply_metadata(
                        pending.envelope,
                        Observation::unavailable(
                            pending.target,
                            self.metadata_now_ms(),
                            PathStatus::IdentityMismatch,
                        ),
                    );
                }
            }
        }
    }

    fn settle_pending_metadata(&mut self) {
        if let Some(pending) = self.metadata_pending.take() {
            self.reply_metadata(
                pending.envelope,
                Observation::unavailable(
                    pending.target,
                    self.metadata_now_ms(),
                    PathStatus::TimedOut,
                ),
            );
        }
    }

    fn reply_metadata(&self, envelope: MultiEnvelope, observation: Observation) {
        let response = pane_metadata::encode_observation(&observation)
            .map(MultiReply::Response)
            .unwrap_or(MultiReply::Error("metadata_response_invalid"));
        let _ = envelope.try_reply(response);
    }

    fn metadata_now_ms(&self) -> u64 {
        self.metadata_origin
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    fn process_pending_closes(&mut self, clock: &impl Fn() -> u64) {
        let mut index = 0;
        while index < self.pending_closes.len() {
            let pending = &self.pending_closes[index];
            let MultiRequest::Request { connection, .. } = &pending.envelope.request else {
                let pending = self.pending_closes.swap_remove(index);
                let _ = pending
                    .envelope
                    .try_reply(MultiReply::Error("invalid_close_request"));
                continue;
            };
            let status = self
                .owner
                .close_status_state(connection, pending.key, clock());
            let reply_value = match status {
                Ok(crate::server::multi_session::CloseStatus::Terminal(result)) => {
                    Some(close_reply(&result))
                }
                Ok(crate::server::multi_session::CloseStatus::NotAdmitted) => Some(
                    match self.owner.close_status(connection, pending.key, clock()) {
                        Ok(result) => close_reply(&result),
                        Err(error) => self::error(&error),
                    },
                ),
                Ok(crate::server::multi_session::CloseStatus::InProgress)
                    if Instant::now() >= pending.deadline =>
                {
                    Some(reply(MultiReply::Error("close_reply_timeout")))
                }
                Ok(crate::server::multi_session::CloseStatus::InProgress) => None,
                Err(error) => Some(self::error(&error)),
            };
            if let Some(DispatchResult::Reply(reply)) = reply_value {
                let pending = self.pending_closes.swap_remove(index);
                let _ = pending.envelope.try_reply(reply);
            } else {
                index += 1;
            }
        }
    }

    fn dispatch_async_split(&mut self, request: &MultiRequest, now: u64) -> DispatchResult {
        use crate::server::multi_session::AsyncSplitAdmission;
        let MultiRequest::Request {
            connection,
            payload,
        } = request
        else {
            return reply(MultiReply::Error("invalid_split_request"));
        };
        if crate::split_protocol::decode_split_status_query(payload).is_ok() {
            return reply(MultiReply::Error("split_status_unsupported"));
        }
        let decoded = match decode_split_request(payload) {
            Ok(value) => value,
            Err(_) => return reply(MultiReply::Error("invalid_split_request")),
        };
        if let Err(error) = self.owner.authorize_split(connection, &decoded, now) {
            return self::error(&error);
        }
        if self.stopping {
            return reply(MultiReply::Error("server_stopping"));
        }
        let key = decoded.key();
        let waiter_available = self.pending_splits.len() < MAX_CONNECTIONS;
        let admission = self.owner.begin_async_split(
            payload,
            now,
            self.launch_worker.is_some() && waiter_available,
        );
        match admission {
            Ok(AsyncSplitAdmission::Terminal(result)) => reply(split_reply(&result)),
            Ok(AsyncSplitAdmission::Uncertain) => {
                reply(MultiReply::Error("launch_outcome_unknown"))
            }
            Ok(AsyncSplitAdmission::Pending) => {
                if !waiter_available {
                    return reply(MultiReply::Error("split_waiter_capacity"));
                }
                DispatchResult::PendingSplit {
                    key,
                    deadline: Instant::now() + OWNER_REPLY_DEADLINE,
                }
            }
            Ok(AsyncSplitAdmission::Start { key, target }) => {
                self.active_launch_target = Some(target);
                if self
                    .launch_worker
                    .as_ref()
                    .is_none_or(|worker| worker.submit_start(payload.clone(), target).is_err())
                {
                    self.fail_launch(crate::protocol::Outcome::Failed);
                    return reply(
                        self.owner
                            .async_split_result(key)
                            .as_ref()
                            .map(split_reply)
                            .unwrap_or(MultiReply::Error("launch_unavailable")),
                    );
                }
                DispatchResult::PendingSplit {
                    key,
                    deadline: Instant::now() + OWNER_REPLY_DEADLINE,
                }
            }
            Err(crate::server::multi_session::SplitError::InProgress)
                if self.launch_worker.is_none() =>
            {
                reply(MultiReply::Error("launch_worker_unavailable"))
            }
            Err(error) => reply(split_error_reply(error)),
        }
    }

    fn fail_launch(&mut self, outcome: crate::protocol::Outcome) {
        self.launch_worker.take();
        if let (Some(key), Some(target)) = (
            self.owner.async_split_key(),
            self.active_launch_target.take(),
        ) {
            let _ = self.owner.finish_async_split(
                key,
                target,
                outcome,
                true,
                |effects: &mut PtyEffects, rows, columns| effects.resize(rows, columns),
            );
        }
    }

    fn advance_launch(&mut self) {
        if !self.owner.async_split_active() {
            return;
        }
        let Some(key) = self.owner.async_split_key() else {
            return;
        };
        let Some(target) = self.active_launch_target else {
            return;
        };
        let result = match self.launch_worker.as_mut() {
            Some(worker) => worker.poll(),
            None => {
                self.fail_launch(crate::protocol::Outcome::Unknown);
                return;
            }
        };
        match result {
            Ok(None) => {}
            Ok(Some(LaunchWorkResult::Spawned(session))) => {
                let mut effects = PtyEffects::new(*session);
                if let Some(coordinator) = &self.history_capture {
                    effects.enable_async_capture(
                        Arc::clone(coordinator),
                        Arc::clone(&self.history_failure),
                        target,
                    );
                }
                if self.owner.attach_async_split(key, target, effects).is_err() {
                    self.fail_launch(crate::protocol::Outcome::Unknown);
                    return;
                }
                if self
                    .launch_worker
                    .as_ref()
                    .is_none_or(|worker| worker.submit_finalize(key, target).is_err())
                {
                    self.fail_launch(crate::protocol::Outcome::Unknown);
                }
            }
            Ok(Some(LaunchWorkResult::Finished { outcome, unhealthy })) => {
                let _ = self.owner.finish_async_split(
                    key,
                    target,
                    outcome,
                    unhealthy,
                    |effects: &mut PtyEffects, rows, columns| effects.resize(rows, columns),
                );
                self.active_launch_target = None;
                if unhealthy {
                    self.launch_worker.take();
                }
            }
            Err(_) => self.fail_launch(crate::protocol::Outcome::Unknown),
        }
    }

    fn process_pending_splits(&mut self) {
        let mut index = 0;
        while index < self.pending_splits.len() {
            let pending = &self.pending_splits[index];
            let response = if let Some(result) = self.owner.async_split_result(pending.key) {
                Some(split_reply(&result))
            } else if self.owner.async_split_key() != Some(pending.key) {
                Some(MultiReply::Error("launch_outcome_unknown"))
            } else if Instant::now() >= pending.deadline {
                Some(MultiReply::Error("split_reply_timeout"))
            } else {
                None
            };
            if let Some(response) = response {
                let pending = self.pending_splits.swap_remove(index);
                let _ = pending.envelope.try_reply(response);
            } else {
                index += 1;
            }
        }
    }

    fn settle_pending_splits(&mut self) {
        for pending in self.pending_splits.drain(..) {
            let _ = pending
                .envelope
                .try_reply(MultiReply::Error("server_stopping"));
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

    fn process_pending_workspace_watches(&mut self, clock: &impl Fn() -> u64) {
        let mut index = 0;
        while index < self.pending_workspace_watches.len() {
            match resolve_pending_workspace_watch(
                &mut self.owner,
                &self.pending_workspace_watches[index],
                clock(),
            ) {
                Some(reply) => {
                    let pending = self.pending_workspace_watches.swap_remove(index);
                    let _ = pending.envelope.try_reply(reply);
                }
                None => index += 1,
            }
        }
    }

    fn settle_pending_workspace_watches(&mut self) {
        for pending in self.pending_workspace_watches.drain(..) {
            let _ = pending
                .envelope
                .try_reply(MultiReply::Error("server_stopping"));
        }
    }

    fn settle_pending_closes(&mut self) {
        for pending in self.pending_closes.drain(..) {
            let _ = pending
                .envelope
                .try_reply(MultiReply::Error("server_stopping"));
        }
    }

    fn drive_history(&mut self) {
        if let Some(coordinator) = &self.history_capture {
            lock_capture(coordinator).drive();
        }
    }
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
/// Exact size of a schema-10 pane status reply.
pub const PANE_REPLY_BYTES: usize = 24 + crate::pane_layout::MAX_PANES * 56;

const PANE_QUERY_KIND: u16 = 3;
const PANE_REPLY_KIND: u16 = 0x8003;
const PANE_REPLY_SCHEMA: u16 = 10;
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
    /// Confirmed child exit code or terminating signal.
    pub exit: Option<PaneExit>,
}

impl fmt::Debug for PaneEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneEntry")
            .field("target", &"<redacted>")
            .field("rect", &self.rect)
            .field("state", &self.state)
            .field("live", &self.live)
            .field("resize_pending", &self.resize_pending)
            .field("exit", &self.exit)
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
    pub panes: [Option<PaneEntry>; crate::pane_layout::MAX_PANES],
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
    if reply.panes[..present].iter().any(Option::is_none)
        || reply.panes[present..].iter().any(Option::is_some)
    {
        return Err(PaneCodecError);
    }
    let mut bytes = [0_u8; PANE_REPLY_BYTES];
    bytes[..2].copy_from_slice(&PANE_REPLY_SCHEMA.to_be_bytes());
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
        match entry.exit {
            None => {}
            Some(PaneExit::Code(code)) => {
                bytes[base + 50] = 1;
                bytes[base + 52..base + 56].copy_from_slice(&code.to_le_bytes());
            }
            Some(PaneExit::Signal(signal)) => {
                bytes[base + 50] = 2;
                bytes[base + 52..base + 56].copy_from_slice(&signal.to_le_bytes());
            }
        }
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
    if read_u16(0) != PANE_REPLY_SCHEMA
        || read_u16(2) != PANE_REPLY_KIND
        || bytes[4..8].iter().any(|byte| *byte != 0)
        || bytes[20..24].iter().any(|byte| *byte != 0)
    {
        return Err(PaneCodecError);
    }
    let mut panes = [None; crate::pane_layout::MAX_PANES];
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
            || entry[51] != 0
        {
            return Err(PaneCodecError);
        }
        let exit_value = i32::from_le_bytes([entry[52], entry[53], entry[54], entry[55]]);
        let exit = match entry[50] {
            0 if exit_value == 0 => None,
            1 => Some(PaneExit::Code(exit_value)),
            2 => Some(PaneExit::Signal(exit_value)),
            _ => return Err(PaneCodecError),
        };
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
            exit,
        });
    }
    let reply = PaneReply {
        layout_revision: read_u64(8),
        rows: read_u16(16),
        columns: read_u16(18),
        panes,
    };
    if (reply.layout_revision == 0) != reply.panes.iter().flatten().next().is_none()
        || reply
            .panes
            .iter()
            .position(Option::is_none)
            .is_some_and(|empty| reply.panes[empty..].iter().any(Option::is_some))
    {
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

struct PendingWorkspaceWatch {
    envelope: MultiEnvelope,
    request: workspace_watch::Request,
    deadline: Instant,
}

struct PendingClose {
    envelope: MultiEnvelope,
    key: OperationKey,
    deadline: Instant,
}

struct PendingSplitReply {
    envelope: MultiEnvelope,
    key: OperationKey,
    deadline: Instant,
}

struct PendingMetadata {
    envelope: MultiEnvelope,
    connection: Connection,
    target: TargetRef,
    identity: ProcessIdentity,
    id: u64,
    deadline: Instant,
}

enum DispatchResult {
    Reply(MultiReply),
    Pending {
        request: WatchRequest,
        deadline: Instant,
    },
    PendingWorkspaceWatch {
        request: workspace_watch::Request,
        deadline: Instant,
    },
    PendingClose {
        key: OperationKey,
        deadline: Instant,
    },
    PendingSplit {
        key: OperationKey,
        deadline: Instant,
    },
}

fn reply(value: MultiReply) -> DispatchResult {
    DispatchResult::Reply(value)
}

fn error(error: &ServerError) -> DispatchResult {
    reply(MultiReply::Error(error.code()))
}

struct PendingCapacity {
    watches: bool,
    closes: bool,
}

fn watch_capacity(pane_watches: usize, workspace_watches: usize) -> bool {
    pane_watches.saturating_add(workspace_watches) < MAX_PENDING_WATCHES
}

fn dispatch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    request: &MultiRequest,
    now: u64,
    pending: PendingCapacity,
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
                    dispatch_watch(owner, connection, request, now, pending.watches, true)
                }
                None => reply(MultiReply::Error("invalid_watch_request")),
            },
            Some([0, 5]) => match decode_watch_request(payload) {
                Ok(request) => {
                    dispatch_watch(owner, connection, request, now, pending.watches, false)
                }
                Err(_) => reply(MultiReply::Error("invalid_watch_request")),
            },
            Some([0, 6]) => dispatch_launch(owner, connection, payload, now),
            Some([0, 7]) => reply(MultiReply::Error("split_dispatch_unavailable")),
            Some([0, 8]) => dispatch_ui(owner, connection, payload, now),
            Some([0, 9]) => dispatch_close(owner, connection, payload, now, pending.closes),
            Some([0, 11]) => match workspace_watch::decode_request(payload) {
                Ok(request) => {
                    dispatch_workspace_watch(owner, connection, request, now, pending.watches)
                }
                Err(_) => reply(MultiReply::Error("invalid_workspace_watch_request")),
            },
            Some([0, 13]) => dispatch_pane_operations(owner, connection, payload, now),
            Some([0, 14]) => dispatch_agent(owner, connection, payload, now),
            Some([0, 15]) if payload.as_slice() == crate::build_info::QUERY => {
                match owner.build_identity(connection, now) {
                    Ok(identity) => reply(MultiReply::Response(
                        crate::build_info::encode_server_identity(identity),
                    )),
                    Err(error) => self::error(&error),
                }
            }
            _ => reply(MultiReply::Error("unsupported_request_schema")),
        },
    }
}

fn dispatch_agent(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    use crate::agent_protocol::{AgentKind, AgentState, Request};
    if connection.selected_capabilities() & crate::handshake_protocol::CAPABILITY_AGENT_OBSERVATION
        == 0
    {
        return reply(MultiReply::Error("agent_observation_unsupported"));
    }
    match crate::agent_protocol::decode_request(payload) {
        Ok(Request::Query(target)) => {
            match owner.observe_target(connection, target, now, |effects| {
                effects.observe_agent(target, now.saturating_mul(1000))
            }) {
                Ok(observation) => match crate::agent_protocol::encode_observation(&observation) {
                    Ok(bytes) => reply(MultiReply::Response(bytes)),
                    Err(_) => reply(MultiReply::Error("agent_observation_invalid")),
                },
                Err(error) => self::error(&error),
            }
        }
        Ok(Request::Submit(request)) => {
            match owner.handle_guarded_agent_submit(connection, &request, payload, now, |effects| {
                let current = effects.observe_agent(request.target, now.saturating_mul(1000));
                if request.identity.kind == AgentKind::Unknown
                    || current.identity != request.identity
                    || current.screen_revision != request.screen_revision
                {
                    return Err(crate::response_protocol::RejectionCode::StaleTarget);
                }
                if current.state != AgentState::Idle {
                    return Err(crate::response_protocol::RejectionCode::Unsupported);
                }
                Ok(())
            }) {
                Ok(bytes) => reply(MultiReply::Response(bytes)),
                Err(error) => self::error(&error),
            }
        }
        Err(_) => reply(MultiReply::Error("invalid_agent_request")),
    }
}

fn dispatch_pane_operations(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
) -> DispatchResult {
    use crate::pane_operations_protocol::{self as pane_ops, Request};

    let request = match pane_ops::decode_request(payload) {
        Ok(request) => request,
        Err(_) => return reply(MultiReply::Error("invalid_pane_operations_request")),
    };
    if !pane_operations_capability_allowed(request, connection.selected_capabilities()) {
        return reply(MultiReply::Error("capability_denied"));
    }
    match request {
        Request::ViewQuery => match owner.pane_operations_view(connection, now) {
            Ok(Some(view)) => match pane_ops::encode_view(&view) {
                Ok(bytes) => reply(MultiReply::Response(bytes.to_vec())),
                Err(_) => reply(MultiReply::Error("invalid_pane_operations_view")),
            },
            Ok(None) => reply(MultiReply::Error("no_layout")),
            Err(error) => self::error(&error),
        },
        mutation => match owner.handle_pane_operation(
            connection,
            mutation,
            payload,
            now,
            |effects, rows, columns| effects.resize(rows, columns),
        ) {
            Ok(bytes) => reply(MultiReply::Response(bytes)),
            Err(error) => self::error(&error),
        },
    }
}

fn pane_operations_capability_allowed(
    request: crate::pane_operations_protocol::Request,
    selected: u64,
) -> bool {
    let required = crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS
        | match request {
            crate::pane_operations_protocol::Request::ViewQuery => CAPABILITY_READ,
            crate::pane_operations_protocol::Request::Swap(_)
            | crate::pane_operations_protocol::Request::DividerSet(_)
            | crate::pane_operations_protocol::Request::ZoomSet(_) => CAPABILITY_INPUT,
        };
    selected & required == required
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

fn split_reply(result: &crate::split_protocol::SplitResult) -> MultiReply {
    let mut bytes = Vec::with_capacity(crate::split_protocol::SPLIT_RESULT_BYTES);
    match crate::split_protocol::encode_split_result(result, &mut bytes) {
        Ok(_) => MultiReply::Response(bytes),
        Err(_) => MultiReply::Error("invalid_split_result"),
    }
}

fn split_error_reply(error: crate::server::multi_session::SplitError) -> MultiReply {
    if let Some((_, failed)) = error.failure() {
        return split_reply(&failed);
    }
    MultiReply::Error(match error {
        crate::server::multi_session::SplitError::Ledger(
            crate::protocol::ProtocolError::OperationConflict,
        ) => "operation_conflict",
        crate::server::multi_session::SplitError::Ledger(_) => "split_ledger_rejected",
        crate::server::multi_session::SplitError::InProgress => "split_in_progress",
        crate::server::multi_session::SplitError::GenerationMismatch => "wrong_generation",
        crate::server::multi_session::SplitError::InvalidPayload(_) => "invalid_split_request",
        _ => "split_failed",
    })
}

/// Schema-9 route: explicit pane close, close status, and pane status.
///
/// A close runs [`MultiSessionOwner::execute_close`] synchronously on the
/// owner thread. Its bounded stop (HUP, 1 s, KILL, 1 s) can therefore block
/// this tick, and every other queued request, for about 2 s. This is accepted
/// for the four-pane workspace; the closing worker waits up to
/// [`CLOSE_REPLY_DEADLINE`] for the reply.
fn dispatch_close(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    payload: &[u8],
    now: u64,
    pending_capacity: bool,
) -> DispatchResult {
    if let Ok(query) = decode_close_status_query(payload) {
        return match owner.close_status_state(connection, query.key, now) {
            Ok(crate::server::multi_session::CloseStatus::Terminal(result)) => close_reply(&result),
            Ok(crate::server::multi_session::CloseStatus::InProgress) if pending_capacity => {
                DispatchResult::PendingClose {
                    key: query.key,
                    deadline: Instant::now() + CLOSE_REPLY_DEADLINE,
                }
            }
            Ok(crate::server::multi_session::CloseStatus::InProgress) => {
                reply(MultiReply::Error("close_wait_capacity"))
            }
            Ok(crate::server::multi_session::CloseStatus::NotAdmitted) => {
                match owner.close_status(connection, query.key, now) {
                    Ok(result) => close_reply(&result),
                    Err(error) => self::error(&error),
                }
            }
            Err(error) => self::error(&error),
        };
    }
    if decode_pane_query(payload).is_ok() {
        return match owner.pane_view_with_exit(connection, now) {
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
                            exit: status.exit,
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
    if !pending_capacity {
        return reply(MultiReply::Error("close_wait_capacity"));
    }
    match owner.begin_close(payload, now, Instant::now()) {
        Ok(crate::server::multi_session::CloseStatus::Terminal(result)) => close_reply(&result),
        Ok(crate::server::multi_session::CloseStatus::InProgress) => DispatchResult::PendingClose {
            key: request.key,
            deadline: Instant::now() + CLOSE_REPLY_DEADLINE,
        },
        Ok(crate::server::multi_session::CloseStatus::NotAdmitted) => {
            reply(MultiReply::Error("close_failed"))
        }
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
                let wraps = (connection.selected_capabilities()
                    & crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS
                    != 0)
                    .then(|| effects.screen().row_wraps())
                    .flatten();
                crate::ui_protocol::prefix_screen_reply_with_wraps(
                    target, revision, checkpoint, wraps,
                )
                .map_err(|_| "invalid_screen")
            });
            match captured {
                Ok(Ok(bytes)) => reply(MultiReply::Snapshot(bytes)),
                Ok(Err(label)) => reply(MultiReply::Error(label)),
                Err(error) => self::error(&error),
            }
        }
        UiRequest::HistoryQuery(query) => {
            let target = query.target;
            let captured = owner.observe_target(connection, target, now, |effects| {
                let (snapshot, offset_rows) = effects
                    .history_snapshot(query.offset_rows)
                    .map_err(|_| "history unavailable")?;
                let revision = effects.revision();
                let metadata = crate::screen_checkpoint::CheckpointMetadata::new(
                    target,
                    0,
                    unix_millis(),
                    revision,
                )
                .map_err(|_| "invalid_history")?;
                let checkpoint = crate::screen_checkpoint::encode(&snapshot, &metadata)
                    .map_err(|_| "invalid_history")?;
                let wraps = (connection.selected_capabilities()
                    & crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS
                    != 0)
                    .then(|| snapshot.row_wraps())
                    .flatten();
                crate::ui_protocol::prefix_history_reply_with_wraps(
                    target,
                    revision,
                    offset_rows,
                    effects.alternate_screen_active(),
                    effects.mouse_reporting_enabled(),
                    checkpoint,
                    wraps,
                )
                .map_err(|_| "invalid_history")
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
        UiRequest::MouseWheelInput(input) => {
            match owner.handle_mouse_wheel(
                connection,
                &input,
                payload,
                now,
                |effects| effects.validate_mouse_wheel(input.column, input.row, input.up),
                |effects| effects.mouse_wheel(input.column, input.row, input.up),
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
                        crate::server::multi_session::LayoutResizeError::InProgress => {
                            "launch_in_progress"
                        }
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
    pending: &[Option<crate::server::multi_session::PendingResize>; crate::pane_layout::MAX_PANES],
) -> Option<[u8; crate::ui_protocol::LAYOUT_REPLY_BYTES]> {
    let mut panes = [None; crate::pane_layout::MAX_PANES];
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
        dividers: layout.dividers,
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

fn dispatch_workspace_watch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    request: workspace_watch::Request,
    now: u64,
    pending_capacity: bool,
) -> DispatchResult {
    match request {
        workspace_watch::Request::Reconcile {
            generation,
            workspace,
        } => match owner.reconcile_workspace(connection, generation, workspace, now) {
            Ok(state) => match workspace_watch::encode_reconciliation(&state) {
                Ok(bytes) => reply(MultiReply::WorkspaceResponse(bytes)),
                Err(_) => reply(MultiReply::Error("invalid_workspace_reconciliation")),
            },
            Err(error) => self::error(&error),
        },
        workspace_watch::Request::Watch {
            cursor,
            limit,
            wait_ms,
        } => {
            let deadline = Instant::now().checked_add(Duration::from_millis(u64::from(wait_ms)));
            match owner.read_workspace_events(connection, cursor, limit, now) {
                Ok(page) if page.status != WatchStatus::TimedOut || wait_ms == 0 => {
                    encode_workspace_watch_reply(page)
                }
                Ok(_) if !pending_capacity => reply(MultiReply::Error("watch_capacity")),
                Ok(_) => match deadline {
                    Some(deadline) => DispatchResult::PendingWorkspaceWatch { request, deadline },
                    None => reply(MultiReply::Error("watch_deadline")),
                },
                Err(error) => self::error(&error),
            }
        }
    }
}

fn encode_workspace_watch_reply(page: workspace_watch::WatchReply) -> DispatchResult {
    match workspace_watch::encode_watch_reply(&page) {
        Ok(bytes) => reply(MultiReply::WorkspaceResponse(bytes)),
        Err(_) => reply(MultiReply::Error("invalid_workspace_watch_reply")),
    }
}

fn resolve_pending_workspace_watch(
    owner: &mut MultiSessionOwner<PtyEffects>,
    pending: &PendingWorkspaceWatch,
    now: u64,
) -> Option<MultiReply> {
    let workspace_watch::Request::Watch { cursor, limit, .. } = pending.request else {
        return Some(MultiReply::Error("invalid_workspace_watch_request"));
    };
    let MultiRequest::Request { connection, .. } = &pending.envelope.request else {
        return Some(MultiReply::Error("invalid_workspace_watch_request"));
    };
    match owner.read_workspace_events(connection, cursor, limit, now) {
        Ok(page) if page.status != WatchStatus::TimedOut => {
            Some(match workspace_watch::encode_watch_reply(&page) {
                Ok(bytes) => MultiReply::WorkspaceResponse(bytes),
                Err(_) => MultiReply::Error("invalid_workspace_watch_reply"),
            })
        }
        Ok(page) if Instant::now() >= pending.deadline => {
            Some(match workspace_watch::encode_watch_reply(&page) {
                Ok(bytes) => MultiReply::WorkspaceResponse(bytes),
                Err(_) => MultiReply::Error("invalid_workspace_watch_reply"),
            })
        }
        Ok(_) => None,
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
    UiHistory(TargetRef),
    UiKeys(OperationKey),
    UiMouseWheel(OperationKey),
    UiResize(OperationKey),
    /// Schema-13 authoritative pane view query or keyed mutation.
    PaneOperations(crate::pane_operations_protocol::Request),
    /// Schema-9 close request or close status query for this key.
    Close(OperationKey),
    /// Schema-9 pane status query.
    Panes,
    /// Schema-11 workspace reconciliation or finite watch.
    Workspace(workspace_watch::Request),
    Metadata(TargetRef),
    AgentQuery(TargetRef),
    AgentSubmit(OperationKey),
    BuildIdentity,
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
            UiRequest::HistoryQuery(query) => Some(RequestSchema::UiHistory(query.target)),
            UiRequest::KeyInput(view) => Some(RequestSchema::UiKeys(view.key())),
            UiRequest::MouseWheelInput(input) => Some(RequestSchema::UiMouseWheel(input.key)),
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
        [0, 11] => workspace_watch::decode_request(payload)
            .ok()
            .map(RequestSchema::Workspace),
        [0, 12] => pane_metadata::decode_query(payload)
            .ok()
            .map(RequestSchema::Metadata),
        [0, 13] => crate::pane_operations_protocol::decode_request(payload)
            .ok()
            .map(RequestSchema::PaneOperations),
        [0, 15] => (payload == crate::build_info::QUERY).then_some(RequestSchema::BuildIdentity),
        [0, 14] => match crate::agent_protocol::decode_request(payload).ok()? {
            crate::agent_protocol::Request::Query(target) => {
                Some(RequestSchema::AgentQuery(target))
            }
            crate::agent_protocol::Request::Submit(request) => {
                Some(RequestSchema::AgentSubmit(request.key))
            }
        },
        _ => None,
    }
}

fn response_matches(schema: RequestSchema, payload: &[u8]) -> bool {
    match schema {
        RequestSchema::BuildIdentity => {
            crate::build_info::decode_server_identity(payload).is_some()
        }
        RequestSchema::AgentQuery(target) => crate::agent_protocol::decode_observation(payload)
            .is_ok_and(|observation| observation.target == target),
        RequestSchema::AgentSubmit(key) => {
            decode_response_payload(payload).is_ok_and(|response| response.key == key)
        }
        RequestSchema::Control => decode_response_payload(payload).is_ok(),
        RequestSchema::UiKeys(key) => {
            decode_response_payload(payload).is_ok_and(|response| response.key == key)
        }
        RequestSchema::UiMouseWheel(key) => {
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
        RequestSchema::UiHistory(_) => false,
        RequestSchema::UiResize(key) => {
            decode_resize_reply(payload).is_ok_and(|result| result.key == key)
                || decode_response_payload(payload).is_ok_and(|response| response.key == key)
        }
        RequestSchema::Close(key) => {
            decode_close_result(payload).is_ok_and(|result| result.key == key)
        }
        RequestSchema::Panes => decode_pane_reply(payload).is_ok(),
        RequestSchema::Workspace(request) => workspace_response_matches(request, payload),
        RequestSchema::Metadata(target) => pane_metadata::decode_observation(payload)
            .is_ok_and(|observation| observation.target == target),
        RequestSchema::PaneOperations(request) => match request {
            crate::pane_operations_protocol::Request::ViewQuery => {
                crate::pane_operations_protocol::decode_view(payload).is_ok()
            }
            crate::pane_operations_protocol::Request::Swap(value) => {
                decode_response_payload(payload).is_ok_and(|response| response.key == value.key)
            }
            crate::pane_operations_protocol::Request::DividerSet(value) => {
                decode_response_payload(payload).is_ok_and(|response| response.key == value.key)
            }
            crate::pane_operations_protocol::Request::ZoomSet(value) => {
                decode_response_payload(payload).is_ok_and(|response| response.key == value.key)
            }
        },
    }
}

fn snapshot_matches(schema: RequestSchema, payload: &[u8], wraps_negotiated: bool) -> bool {
    matches!(schema, RequestSchema::UiScreen(target)
        if decode_screen_reply_with_wraps(payload, wraps_negotiated).is_ok_and(|view| view.target() == target))
        || matches!(schema, RequestSchema::UiHistory(target)
            if decode_history_reply_with_wraps(payload, wraps_negotiated).is_ok_and(|view| view.target() == target))
}

fn workspace_response_matches(request: workspace_watch::Request, payload: &[u8]) -> bool {
    match request {
        workspace_watch::Request::Reconcile {
            generation,
            workspace,
        } => workspace_watch::decode_reconciliation(payload).is_ok_and(|state| {
            state.cursor.generation == generation
                && state.cursor.workspace == workspace
                && state.revision != 0
                && state.layout.as_ref().is_none_or(|layout| {
                    layout.revision == state.revision
                        && layout.panes.iter().flatten().all(|pane| {
                            pane.target.generation == generation
                                && pane.target.workspace == workspace
                        })
                })
        }),
        workspace_watch::Request::Watch { cursor, limit, .. } => {
            workspace_watch::decode_watch_reply(payload).is_ok_and(|reply| {
                if reply.cursor.generation != cursor.generation
                    || reply.cursor.workspace != cursor.workspace
                {
                    return false;
                }
                match reply.status {
                    WatchStatus::Changed => {
                        let Some(first) = cursor.sequence.checked_add(1) else {
                            return false;
                        };
                        reply.events.len() <= usize::from(limit)
                            && reply
                                .events
                                .first()
                                .is_some_and(|event| event.sequence == first)
                            && reply.events.windows(2).all(|pair| {
                                pair[0].sequence.checked_add(1) == Some(pair[1].sequence)
                            })
                            && reply
                                .events
                                .last()
                                .is_some_and(|event| event.sequence == reply.cursor.sequence)
                    }
                    WatchStatus::TimedOut => {
                        reply.events.is_empty() && reply.cursor.sequence == cursor.sequence
                    }
                    WatchStatus::ResyncRequired => {
                        reply.events.is_empty() && reply.cursor.sequence >= cursor.sequence
                    }
                }
            })
        }
    }
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
        if matches!(schema, RequestSchema::Workspace(_))
            && connection.selected_capabilities() & (CAPABILITY_READ | CAPABILITY_WORKSPACE_EVENTS)
                != (CAPABILITY_READ | CAPABILITY_WORKSPACE_EVENTS)
        {
            if write_error(&mut stream, request_id, "capability_denied", stop).is_err() {
                return;
            }
            continue;
        }
        if let RequestSchema::PaneOperations(request) = schema {
            if !pane_operations_capability_allowed(request, connection.selected_capabilities()) {
                if write_error(&mut stream, request_id, "capability_denied", stop).is_err() {
                    return;
                }
                continue;
            }
        }
        // A close's bounded stop runs inside the owner tick; wait longer.
        let reply_deadline = if matches!(schema, RequestSchema::Close(_)) {
            CLOSE_REPLY_DEADLINE
        } else if matches!(
            schema,
            RequestSchema::Workspace(workspace_watch::Request::Watch { .. })
        ) {
            OWNER_REPLY_DEADLINE + Duration::from_millis(1_100)
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
            MultiReply::WorkspaceResponse(payload) if response_matches(schema, &payload) => {
                write_frame(
                    &mut stream,
                    MessageKind::Response,
                    request_id,
                    &payload,
                    stop,
                )
            }
            MultiReply::Snapshot(payload)
                if snapshot_matches(
                    schema,
                    &payload,
                    connection.selected_capabilities()
                        & crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS
                        != 0,
                ) =>
            {
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    static NEXT_BLOCKED_CAPTURE_FIXTURE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct RemoveFixture(PathBuf);

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl Drop for RemoveFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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

    #[test]
    fn pane_operations_classifier_and_capability_classes_are_strict() {
        use crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS;
        use crate::pane_operations_protocol::{self as pane_ops, PaneSwap, Request};

        let query = pane_ops::encode_request(&Request::ViewQuery).unwrap();
        assert!(matches!(
            classify(&query),
            Some(RequestSchema::PaneOperations(Request::ViewQuery))
        ));
        assert!(!pane_operations_capability_allowed(
            Request::ViewQuery,
            CAPABILITY_PANE_OPERATIONS
        ));
        assert!(pane_operations_capability_allowed(
            Request::ViewQuery,
            CAPABILITY_PANE_OPERATIONS | CAPABILITY_READ
        ));

        let swap = Request::Swap(PaneSwap {
            key: KEY,
            expected_layout_revision: 2,
            source: TARGET,
            destination: TargetRef {
                pane: 3,
                session: 3,
                ..TARGET
            },
        });
        let mutation = pane_ops::encode_request(&swap).unwrap();
        assert!(
            matches!(classify(&mutation), Some(RequestSchema::PaneOperations(value)) if value == swap)
        );
        assert!(!pane_operations_capability_allowed(
            swap,
            CAPABILITY_PANE_OPERATIONS | CAPABILITY_READ
        ));
        assert!(pane_operations_capability_allowed(
            swap,
            CAPABILITY_PANE_OPERATIONS | CAPABILITY_INPUT
        ));

        let mut trailing = query;
        trailing.push(0);
        assert!(classify(&trailing).is_none());
    }

    #[test]
    fn pane_operation_mutation_response_must_match_original_key() {
        let mut bytes = Vec::new();
        crate::response_protocol::encode_response_payload(
            &crate::response_protocol::Response {
                key: KEY,
                body: crate::response_protocol::ResponseBody::Status(
                    crate::protocol::OperationStatus::InProgress,
                ),
            },
            &mut bytes,
        )
        .unwrap();
        assert!(response_matches(
            RequestSchema::PaneOperations(crate::pane_operations_protocol::Request::ZoomSet(
                crate::pane_operations_protocol::PaneZoomSet {
                    key: KEY,
                    expected_layout_revision: 2,
                    target: TARGET,
                    zoomed: true,
                }
            )),
            &bytes
        ));
        let wrong_key = OperationKey { sequence: 6, ..KEY };
        assert!(!response_matches(
            RequestSchema::PaneOperations(crate::pane_operations_protocol::Request::ZoomSet(
                crate::pane_operations_protocol::PaneZoomSet {
                    key: wrong_key,
                    expected_layout_revision: 2,
                    target: TargetRef {
                        generation: 3,
                        ..TARGET
                    },
                    zoomed: true,
                }
            )),
            &bytes
        ));
    }

    #[test]
    fn pane_operations_view_response_rejects_noncanonical_shape() {
        let malformed = [0; crate::pane_operations_protocol::PANE_OPERATIONS_VIEW_BYTES];
        assert!(!response_matches(
            RequestSchema::PaneOperations(crate::pane_operations_protocol::Request::ViewQuery),
            &malformed
        ));
    }

    #[test]
    fn snapshot_reply_guard_preserves_negotiation_and_exact_target() {
        use crate::screen_checkpoint::{encode, CheckpointMetadata};
        use crate::terminal::{ScreenSnapshot, TerminalSize};
        use crate::ui_protocol::{prefix_history_reply_with_wraps, prefix_screen_reply_with_wraps};

        let screen = ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap();
        let metadata = CheckpointMetadata::new(TARGET, 0, 1, 1).unwrap();
        let checkpoint = encode(&screen, &metadata).unwrap();
        let wraps = [true, false, false, false, false];
        for negotiated in [false, true] {
            let row_wraps = negotiated.then_some(wraps.as_slice());
            let replies = [
                (
                    RequestSchema::UiScreen(TARGET),
                    prefix_screen_reply_with_wraps(TARGET, 1, checkpoint.clone(), row_wraps)
                        .unwrap(),
                ),
                (
                    RequestSchema::UiHistory(TARGET),
                    prefix_history_reply_with_wraps(
                        TARGET,
                        1,
                        1,
                        false,
                        false,
                        checkpoint.clone(),
                        row_wraps,
                    )
                    .unwrap(),
                ),
            ];
            for (schema, bytes) in replies {
                assert!(snapshot_matches(schema, &bytes, negotiated));
                if negotiated {
                    assert!(!snapshot_matches(schema, &bytes, false));
                }
                let other = TargetRef {
                    session: 99,
                    ..TARGET
                };
                assert!(!snapshot_matches(
                    RequestSchema::UiScreen(other),
                    &bytes,
                    true
                ));
                assert!(!snapshot_matches(
                    RequestSchema::UiHistory(other),
                    &bytes,
                    true
                ));
                assert!(!snapshot_matches(RequestSchema::UiLayout, &bytes, true));
            }
        }
    }

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
                    exit: None,
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
                    exit: Some(PaneExit::Signal(15)),
                }),
                None,
                None,
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn socketpair_close_worker_disconnect_does_not_cancel_owner_transaction() {
        use crate::close_protocol::{encode_close_request, CloseRequest};
        use crate::handshake_protocol::{decode_welcome, Hello, CAPABILITY_READ};
        use crate::launch::LaunchContext;
        use crate::multi_startup::{StartedMultiServer, StartupLaunch};
        use crate::server::multi_session::CloseStatus;
        use crate::session_protocol::{encode_session_hello, SessionHello};
        use crate::transport::AuthenticatedStream;
        use std::ffi::OsString;
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc::sync_channel;
        use std::time::{Duration, Instant};

        let root = std::path::PathBuf::from("/private/tmp").join(format!(
            "panelink-close-worker-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let _cleanup = RemoveFixture(root.clone());
        let server_dir = root.join("server");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&terminfo)
            .unwrap();
        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from("trap '' HUP; printf 'HUP-READY\\n'; while :; do /bin/sleep 10; done"),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            ServerStorage::initialize(&server_dir).unwrap(),
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let (welcome_send, welcome_recv) = sync_channel(1);
        let worker_client = std::thread::spawn(move || {
            let mut stream = AuthenticatedStream::new(client_socket).unwrap();
            let deadline = || Instant::now() + Duration::from_secs(3);
            let hello = encode_session_hello(&SessionHello {
                hello: Hello {
                    nonce: 91,
                    required_capabilities: CAPABILITY_READ,
                    optional_capabilities: 0,
                },
                resume: None,
            })
            .unwrap();
            stream
                .write_frame(
                    &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                    deadline(),
                )
                .unwrap();
            let welcome = stream.read_frame(deadline()).unwrap();
            let welcome = decode_welcome(welcome.payload()).unwrap();
            welcome_send.send(welcome).unwrap();
            let mut payload = Vec::new();
            encode_close_request(
                &CloseRequest {
                    key: OperationKey {
                        generation: welcome.generation,
                        lease: welcome.lease,
                        sequence: 1,
                    },
                    target,
                    expected_layout_revision: 1,
                },
                &mut payload,
            )
            .unwrap();
            stream
                .write_frame(
                    &Frame::new(MessageKind::Request, 1, &payload).unwrap(),
                    deadline(),
                )
                .unwrap();
            // The owner will stop the worker while it is waiting for this
            // terminal-only close response, dropping the reply receiver.
            let _ = stream.read_frame(Instant::now() + Duration::from_secs(5));
        });

        let setup_deadline = Instant::now() + Duration::from_secs(3);
        let welcome = loop {
            driver.tick(|| 1).unwrap();
            if let Ok(welcome) = welcome_recv.try_recv() {
                break welcome;
            }
            assert!(Instant::now() < setup_deadline, "worker did not attach");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(welcome.generation, target.generation);
        while driver.pending_closes.is_empty() {
            driver.tick(|| 2).unwrap();
            assert!(Instant::now() < setup_deadline, "close was not admitted");
            std::thread::sleep(Duration::from_millis(2));
        }
        let (connection, close_key) = {
            let pending = &driver.pending_closes[0];
            let MultiRequest::Request { connection, .. } = &pending.envelope.request else {
                panic!("pending close contains a request")
            };
            (*connection, pending.key)
        };
        assert!(matches!(
            driver
                .owner
                .close_status_state(&connection, close_key, 2)
                .unwrap(),
            CloseStatus::InProgress
        ));
        driver.stop_connections(|| 2);
        worker_client.join().unwrap();

        // Progress the server-owned transaction after the worker and its
        // response receiver have gone away.
        let start = Instant::now();
        driver
            .owner
            .advance_closes_at(start + Duration::from_secs(1))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            driver.owner.poll(3).unwrap();
            driver
                .owner
                .advance_closes_at(start + Duration::from_secs(1))
                .unwrap();
            if matches!(
                driver
                    .owner
                    .close_status_state(&connection, close_key, 3)
                    .unwrap(),
                CloseStatus::Terminal(_)
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "close owner work was cancelled");
            std::thread::yield_now();
        }
        driver.process_pending_closes(&|| 3);
        assert!(driver.pending_closes.is_empty());
        assert!(matches!(
            driver
                .owner
                .close_status_state(&connection, close_key, 3)
                .unwrap(),
            CloseStatus::Terminal(_)
        ));
    }

    #[test]
    fn pane_reply_round_trips_and_rejects_malformed_bytes() {
        let reply = sample_pane_reply();
        let bytes = encode_pane_reply(&reply).unwrap();
        assert_eq!(bytes.len(), PANE_REPLY_BYTES);
        assert_eq!(decode_pane_reply(&bytes), Ok(reply));

        let single = PaneReply {
            panes: [reply.panes[0], None, None, None],
            ..reply
        };
        let bytes = encode_pane_reply(&single).unwrap();
        assert_eq!(decode_pane_reply(&bytes), Ok(single));
        assert!(bytes[80..].iter().all(|byte| *byte == 0));

        let empty = PaneReply {
            layout_revision: 0,
            rows: 0,
            columns: 0,
            panes: [None; crate::pane_layout::MAX_PANES],
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
            (24 + 50, 3),
            (24 + 51, 1),
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
        // Kind zero must not carry a nonzero status value.
        let mut bad = good;
        bad[24 + 52] = 1;
        assert!(decode_pane_reply(&bad).is_err());
        // Old schema-9 replies fail closed instead of interpreting the tail.
        let mut old_schema = good;
        old_schema[..2].copy_from_slice(&9_u16.to_be_bytes());
        assert!(decode_pane_reply(&old_schema).is_err());
        // Absent entries must be all zero.
        let mut bad = encode_pane_reply(&single).unwrap();
        bad[100] = 1;
        assert!(decode_pane_reply(&bad).is_err());
        // Debug never prints the target identity.
        let debug = format!("{:?}", reply.panes[0].unwrap());
        assert!(debug.contains("<redacted>") && !debug.contains("generation"));
    }

    #[test]
    fn pane_exit_codes_and_signals_round_trip_signed_i32_values() {
        let mut reply = sample_pane_reply();
        reply.panes[0].as_mut().unwrap().exit = Some(PaneExit::Code(-17));
        reply.panes[1].as_mut().unwrap().exit = Some(PaneExit::Signal(1234));
        reply.panes[2] = Some(PaneEntry {
            target: TargetRef {
                pane: 4,
                session: 4,
                ..TARGET
            },
            rect: Rect {
                x: 0,
                y: 0,
                rows: 1,
                columns: 1,
            },
            state: SessionState::Exited,
            live: false,
            resize_pending: false,
            exit: Some(PaneExit::Code(i32::MAX)),
        });
        reply.panes[3] = Some(PaneEntry {
            target: TargetRef {
                pane: 5,
                session: 5,
                ..TARGET
            },
            rect: Rect {
                x: 1,
                y: 0,
                rows: 1,
                columns: 1,
            },
            state: SessionState::Exited,
            live: false,
            resize_pending: false,
            exit: Some(PaneExit::Signal(i32::MIN)),
        });
        let bytes = encode_pane_reply(&reply).unwrap();
        assert_eq!(decode_pane_reply(&bytes), Ok(reply));
        assert_eq!(&bytes[24 + 52..24 + 56], &(-17_i32).to_le_bytes());
        assert_eq!(
            &bytes[24 + PANE_ENTRY_BYTES + 52..24 + PANE_ENTRY_BYTES + 56],
            &1234_i32.to_le_bytes()
        );
        assert_eq!(
            &bytes[24 + 2 * PANE_ENTRY_BYTES + 52..24 + 2 * PANE_ENTRY_BYTES + 56],
            &i32::MAX.to_le_bytes()
        );
        assert_eq!(
            &bytes[24 + 3 * PANE_ENTRY_BYTES + 52..24 + 3 * PANE_ENTRY_BYTES + 56],
            &i32::MIN.to_le_bytes()
        );
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct BlockingHistoryBackend {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Option<std::sync::mpsc::Receiver<()>>,
        observed_targets: std::sync::Arc<std::sync::Mutex<Vec<TargetRef>>>,
        completed: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl crate::history_writer::HistoryBackend for BlockingHistoryBackend {
        fn load_rows(
            &mut self,
            _target: TargetRef,
        ) -> io::Result<Option<crate::history_store::SavedHistory>> {
            Ok(None)
        }

        fn begin_history(&mut self, target: TargetRef, _epoch: u64, _gap: bool) -> io::Result<()> {
            self.observed_targets.lock().unwrap().push(target);
            if let Some(release) = self.release.take() {
                let _ = self.entered.send(());
                let _ = release.recv();
            }
            if let Some(completed) = &self.completed {
                completed.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn mark_history(
            &mut self,
            _target: TargetRef,
            _epoch: u64,
            _disabled: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn append_rows(
            &mut self,
            _target: TargetRef,
            _rows: &[Vec<u8>],
            _gap: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn save_screen(
            &mut self,
            _target: TargetRef,
            _buffer: crate::screen_checkpoint::ScreenBuffer,
            _screen: &[u8],
        ) -> io::Result<()> {
            Ok(())
        }

        fn mark_ended(&mut self, _target: TargetRef, _at_unix_ms: u64) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct BlockingLayoutBackend {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Option<std::sync::mpsc::Receiver<()>>,
        completed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl crate::layout_writer::LayoutBackend for BlockingLayoutBackend {
        fn save(
            &mut self,
            _bound_generation: u64,
            _bound_workspace: u64,
            _candidate: &SavedLayout,
        ) -> io::Result<crate::layout_store::SaveOutcome> {
            if let Some(release) = self.release.take() {
                let _ = self.entered.send(());
                let _ = release.recv();
            }
            self.completed.store(true, Ordering::Release);
            Ok(crate::layout_store::SaveOutcome::Written)
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct ReleaseBlockedWriters(Vec<std::sync::mpsc::SyncSender<()>>);

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl ReleaseBlockedWriters {
        fn release(&self) {
            for sender in &self.0 {
                let _ = sender.try_send(());
            }
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl Drop for ReleaseBlockedWriters {
        fn drop(&mut self) {
            self.release();
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn driver_shutdown_shares_metadata_deadline_after_children_exit() {
        use crate::launch::LaunchContext;
        use crate::multi_startup::{StartedMultiServer, StartupLaunch};
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::mpsc::sync_channel;
        use std::time::{Duration, Instant};

        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-shared-metadata-deadline-{}-{}",
            std::process::id(),
            NEXT_BLOCKED_CAPTURE_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let _cleanup = RemoveFixture(root.clone());
        let server_dir = root.join("server");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();

        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![OsString::from("-c"), OsString::from("exit 0")];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            ServerStorage::initialize(&server_dir).unwrap(),
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();

        let (history_entered_send, history_entered_recv) = sync_channel(1);
        let (history_release_send, history_release_recv) = sync_channel(1);
        let (layout_entered_send, layout_entered_recv) = sync_channel(1);
        let (layout_release_send, layout_release_recv) = sync_channel(1);
        let history_completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let layout_completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_guards = ReleaseBlockedWriters(vec![history_release_send, layout_release_send]);

        driver.enable_history_backend_for_test(Box::new(BlockingHistoryBackend {
            entered: history_entered_send,
            release: Some(history_release_recv),
            observed_targets: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            completed: Some(std::sync::Arc::clone(&history_completed)),
        }));
        history_entered_recv
            .recv_timeout(Duration::from_secs(3))
            .expect("history worker should block in its active save");

        let exit_deadline = Instant::now() + Duration::from_secs(3);
        while !driver.owner().layout_terminal_empty() && Instant::now() < exit_deadline {
            driver.tick(|| 1).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            driver.owner().layout_terminal_empty(),
            "child must be positively recorded Exited before measuring metadata drain"
        );

        // Start layout persistence only after the owner has recorded exit, so
        // the first and only observed candidate is the terminal-empty tombstone.
        let layout_writer = LayoutWriter::start_with_backend(
            Box::new(BlockingLayoutBackend {
                entered: layout_entered_send,
                release: Some(layout_release_recv),
                completed: std::sync::Arc::clone(&layout_completed),
            }),
            &driver.storage,
            DEFAULT_WORKSPACE,
        )
        .unwrap();
        let layout_status = layout_writer.status_handle();
        driver.layout = Some(LayoutObserver {
            writer: Some(layout_writer),
            status: layout_status.clone(),
            generation: target.generation,
            last: None,
            sequence: 0,
            disabled: false,
            frozen: false,
        });
        driver.observe_layout();
        layout_entered_recv
            .recv_timeout(Duration::from_secs(3))
            .expect("layout worker should block in its active save");
        assert_eq!(
            layout_status
                .status()
                .last_submitted
                .unwrap()
                .snapshot_sequence,
            1
        );

        let storage_dir = driver.storage.directory_path().to_owned();
        let started = Instant::now();
        drop(driver);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(425),
            "metadata shutdown took {elapsed:?}; expected one shared 250 ms drain budget"
        );
        assert!(
            matches!(
                layout_status.status().last_submitted,
                Some(crate::layout_writer::LayoutKey {
                    generation,
                    snapshot_sequence: 1
                }) if generation == target.generation
            ),
            "layout candidate should be admitted before shutdown"
        );
        assert_eq!(layout_status.status().last_durable, None);
        assert!(!history_completed.load(Ordering::Acquire));
        assert!(!layout_completed.load(Ordering::Acquire));
        assert_eq!(
            ServerStorage::lock_for_maintenance(&storage_dir)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock,
            "blocked metadata workers must retain the singleton until released"
        );

        release_guards.release();
        let release_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match ServerStorage::lock_for_maintenance(&storage_dir) {
                Ok(lock) => {
                    drop(lock);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("maintenance lock failed after release: {error}"),
            }
            assert!(
                Instant::now() < release_deadline,
                "metadata workers did not release the singleton after their gates opened"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct AlwaysFailHistoryBackend;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl crate::history_writer::HistoryBackend for AlwaysFailHistoryBackend {
        fn load_rows(
            &mut self,
            _target: TargetRef,
        ) -> io::Result<Option<crate::history_store::SavedHistory>> {
            Ok(None)
        }

        fn begin_history(&mut self, _target: TargetRef, _epoch: u64, _gap: bool) -> io::Result<()> {
            Err(io::Error::other("injected history backend failure"))
        }

        fn mark_history(
            &mut self,
            _target: TargetRef,
            _epoch: u64,
            _disabled: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn append_rows(
            &mut self,
            _target: TargetRef,
            _rows: &[Vec<u8>],
            _gap: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn save_screen(
            &mut self,
            _target: TargetRef,
            _buffer: crate::screen_checkpoint::ScreenBuffer,
            _screen: &[u8],
        ) -> io::Result<()> {
            Ok(())
        }

        fn mark_ended(&mut self, _target: TargetRef, _at_unix_ms: u64) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn blocked_history_writer_does_not_block_pty_input_or_socket_control() {
        use crate::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
        use crate::launch::LaunchContext;
        use crate::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
        use crate::multi_startup::{StartedMultiServer, StartupLaunch};
        use crate::pane_layout::SplitDirection;
        use crate::protocol::{OperationKey, OperationStatus, Outcome};
        use crate::protocol_wire::{Frame, MessageKind};
        use crate::response_protocol::{decode_response_payload, ResponseBody};
        use crate::screen_checkpoint::decode as decode_checkpoint;
        use crate::session_protocol::{
            decode_session_info, encode_session_hello, encode_session_request, SessionHello,
            SessionRequest,
        };
        use crate::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
        use crate::terminal::Key;
        use crate::transport::AuthenticatedStream;
        use crate::ui_protocol::{
            decode_layout_reply, decode_screen_reply, encode_attach_target, encode_key_input,
            encode_layout_query, encode_screen_query, planned_split_size, KeyInput,
        };
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::net::UnixStream;
        use std::path::PathBuf;
        use std::sync::mpsc::sync_channel;
        use std::time::{Duration, Instant};

        // Permit scheduler delay under the deliberate CPU-load repetitions.
        // Each driver-loop cap still bounds a deadlock, and the tick cap stays
        // far below the overall progress deadline to catch blocking storage IO.
        const IO_DEADLINE: Duration = Duration::from_secs(30);
        const DRIVER_PROGRESS_DEADLINE: Duration = Duration::from_secs(60);
        const MAX_TICK_LATENCY: Duration = Duration::from_millis(250);

        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-live-capture-blocked-{}-{}",
            std::process::id(),
            NEXT_BLOCKED_CAPTURE_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let _cleanup = RemoveFixture(root.clone());
        let server_dir = root.join("server");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();

        let storage = ServerStorage::initialize(&server_dir).unwrap();
        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(
                "printf 'PTY-READY\\r\\n'; IFS= read -r line; printf 'INPUT:%s\\r\\n' \"$line\"; while :; do /bin/sleep 1; done",
            ),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();

        let (entered_send, entered_recv) = sync_channel(1);
        let (release_send, release_recv) = sync_channel(1);
        let observed_targets = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        driver.enable_history_backend_for_test(Box::new(BlockingHistoryBackend {
            entered: entered_send,
            release: Some(release_recv),
            observed_targets: std::sync::Arc::clone(&observed_targets),
            completed: None,
        }));
        entered_recv
            .recv_timeout(IO_DEADLINE)
            .expect("writer should reach the deterministic blocking backend");

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let failure = driver.history_failure_signal();
        let client_cwd = root.clone();
        let client = std::thread::scope(|scope| {
            let task = scope.spawn(move || {
                let deadline = || Instant::now() + IO_DEADLINE;
                let mut stream = AuthenticatedStream::new(client_socket).unwrap();
                let hello = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 17,
                        required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                        optional_capabilities: 0,
                    },
                    resume: None,
                })
                .unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                        deadline(),
                    )
                    .unwrap();
                let welcome = stream.read_frame(deadline()).unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                let welcome = decode_welcome(welcome.payload()).unwrap();
                stream.enable_snapshot_reads().unwrap();

                let mut request_id = 0_u64;
                let send =
                    |stream: &mut AuthenticatedStream, request_id: &mut u64, payload: &[u8]| {
                        *request_id += 1;
                        stream
                            .write_frame(
                                &Frame::new(MessageKind::Request, *request_id, payload).unwrap(),
                                deadline(),
                            )
                            .unwrap();
                        stream.read_frame(deadline()).unwrap()
                    };
                let describe = encode_session_request(&SessionRequest::Describe).unwrap();
                let info =
                    decode_session_info(send(&mut stream, &mut request_id, &describe).payload())
                        .unwrap();
                let key = OperationKey {
                    generation: welcome.generation,
                    lease: welcome.lease,
                    sequence: 1,
                };
                let claim = encode_session_request(&SessionRequest::Claim {
                    key,
                    target,
                    expected_owner_revision: info.owner_revision,
                    ttl_seconds: 30,
                })
                .unwrap();
                let claim_reply = send(&mut stream, &mut request_id, &claim);
                assert_eq!(claim_reply.kind(), MessageKind::Response);
                assert!(matches!(
                    decode_response_payload(claim_reply.payload()).unwrap().body,
                    ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                ));
                let info =
                    decode_session_info(send(&mut stream, &mut request_id, &describe).payload())
                        .unwrap();
                let keys = [
                    Key::Char('B'),
                    Key::Char('L'),
                    Key::Char('O'),
                    Key::Char('C'),
                    Key::Char('K'),
                    Key::Char('E'),
                    Key::Char('D'),
                    Key::Enter,
                ];
                let key = OperationKey { sequence: 2, ..key };
                let input = encode_key_input(&KeyInput {
                    key,
                    target,
                    expected_owner_revision: info.owner_revision,
                    keys: &keys,
                })
                .unwrap();
                let input_reply = send(&mut stream, &mut request_id, &input);
                assert_eq!(input_reply.kind(), MessageKind::Response);
                assert!(matches!(
                    decode_response_payload(input_reply.payload()).unwrap().body,
                    ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                ));

                let deadline_at = Instant::now() + IO_DEADLINE;
                let input_screen = loop {
                    let screen = encode_screen_query(target).unwrap();
                    let reply = send(&mut stream, &mut request_id, &screen);
                    assert_eq!(reply.kind(), MessageKind::Snapshot);
                    let snapshot =
                        crate::ui_protocol::decode_screen_reply(reply.payload()).unwrap();
                    let checkpoint = decode_checkpoint(snapshot.checkpoint()).unwrap();
                    let text = (0..checkpoint.rows())
                        .map(|row| {
                            (0..checkpoint.columns())
                                .map(|column| checkpoint.cell(row, column).unwrap().text())
                                .collect::<String>()
                        })
                        .collect::<String>();
                    if text.contains("INPUT:BLOCKED") {
                        break text;
                    }
                    assert!(
                        Instant::now() < deadline_at,
                        "PTY did not process socket input"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                };
                let layout_request = encode_layout_query();
                let layout = decode_layout_reply(
                    send(&mut stream, &mut request_id, &layout_request).payload(),
                )
                .unwrap();
                let (rows, columns) =
                    planned_split_size(&layout, target, SplitDirection::Right).unwrap();
                let split_key = OperationKey { sequence: 3, ..key };
                let arguments: [&[u8]; 2] = [
                    b"-c",
                    b"printf 'ASYNC-SPLIT-READY\\r\\n'; while :; do /bin/sleep 1; done",
                ];
                let environment = [
                    EnvironmentEntry {
                        name: b"PATH",
                        value: b"/usr/bin:/bin",
                    },
                    EnvironmentEntry {
                        name: b"HOME",
                        value: client_cwd.as_os_str().as_bytes(),
                    },
                ];
                let launch = LaunchRequest {
                    key: split_key,
                    rows,
                    columns,
                    requested_executable: b"/bin/sh",
                    requested_working_directory: client_cwd.as_os_str().as_bytes(),
                    client_working_directory: client_cwd.as_os_str().as_bytes(),
                    arguments: &arguments,
                    environment: &environment,
                };
                let mut split_payload = Vec::new();
                encode_split_request(
                    &SplitRequest {
                        key: split_key,
                        target,
                        expected_layout_revision: layout.revision,
                        direction: SplitDirection::Right,
                        launch: &launch,
                    },
                    &mut split_payload,
                )
                .unwrap();
                let split_reply = send(&mut stream, &mut request_id, &split_payload);
                assert_eq!(split_reply.kind(), MessageKind::Response);
                let split = decode_split_result(split_reply.payload()).unwrap();
                assert_eq!(split.outcome, Outcome::Applied);
                let split_target = split.new_target.expect("applied split target");
                (input_screen, split_target)
            });

            let deadline = Instant::now() + DRIVER_PROGRESS_DEADLINE;
            while !task.is_finished() {
                let tick_started = Instant::now();
                driver.tick(|| 100).unwrap();
                assert!(
                    tick_started.elapsed() < MAX_TICK_LATENCY,
                    "owner tick waited for blocked history storage"
                );
                assert!(Instant::now() < deadline, "socket control did not progress");
                std::thread::sleep(Duration::from_millis(5));
            }
            task.join().unwrap()
        });
        let (client, split_target) = client;
        assert!(client.contains("PTY-READY"));
        assert!(client.contains("INPUT:BLOCKED"));
        release_send.send(()).unwrap();

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let screen = std::thread::scope(|scope| {
            let task = scope.spawn(move || {
                let mut stream = AuthenticatedStream::new(client_socket).unwrap();
                let mut hello = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 18,
                        required_capabilities: CAPABILITY_READ,
                        optional_capabilities: 0,
                    },
                    resume: None,
                })
                .unwrap();
                hello.extend_from_slice(&encode_attach_target(split_target).unwrap());
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                        Instant::now() + IO_DEADLINE,
                    )
                    .unwrap();
                let welcome = stream.read_frame(Instant::now() + IO_DEADLINE).unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                let _welcome = decode_welcome(welcome.payload()).unwrap();
                stream.enable_snapshot_reads().unwrap();
                let query = encode_screen_query(split_target).unwrap();
                let deadline = Instant::now() + IO_DEADLINE;
                let mut request_id = 0_u64;
                loop {
                    request_id += 1;
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Request, request_id, &query).unwrap(),
                            Instant::now() + IO_DEADLINE,
                        )
                        .unwrap();
                    let reply = stream.read_frame(Instant::now() + IO_DEADLINE).unwrap();
                    assert_eq!(reply.kind(), MessageKind::Snapshot);
                    let snapshot = decode_screen_reply(reply.payload()).unwrap();
                    let checkpoint = decode_checkpoint(snapshot.checkpoint()).unwrap();
                    let text = (0..checkpoint.rows())
                        .map(|row| {
                            (0..checkpoint.columns())
                                .map(|column| checkpoint.cell(row, column).unwrap().text())
                                .collect::<String>()
                        })
                        .collect::<String>();
                    if text.contains("ASYNC-SPLIT-READY") {
                        return text;
                    }
                    assert!(Instant::now() < deadline, "split PTY did not start");
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            let deadline = Instant::now() + DRIVER_PROGRESS_DEADLINE;
            while !task.is_finished() {
                driver.tick(|| 100).unwrap();
                assert!(Instant::now() < deadline, "split screen query timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
            task.join().unwrap()
        });
        assert!(screen.contains("ASYNC-SPLIT-READY"));
        let capture_deadline = Instant::now() + IO_DEADLINE;
        while !observed_targets.lock().unwrap().contains(&split_target) {
            driver.tick(|| 100).unwrap();
            assert!(
                Instant::now() < capture_deadline,
                "split target was not captured"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!failure.load(Ordering::Acquire));
        drop(driver);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn full_capture_registry_does_not_reject_socket_split_or_child_launch() {
        use crate::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
        use crate::history_capture::MAX_CAPTURE_TARGETS;
        use crate::launch::LaunchContext;
        use crate::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
        use crate::multi_startup::{StartedMultiServer, StartupLaunch};
        use crate::pane_layout::SplitDirection;
        use crate::protocol::{OperationKey, Outcome};
        use crate::protocol_wire::{Frame, MessageKind};
        use crate::screen_checkpoint::decode as decode_checkpoint;
        use crate::session_protocol::{
            decode_session_info, encode_session_hello, encode_session_request, SessionHello,
            SessionRequest,
        };
        use crate::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
        use crate::transport::AuthenticatedStream;
        use crate::ui_protocol::{
            decode_layout_reply, decode_screen_reply, encode_attach_target, encode_layout_query,
            encode_screen_query, planned_split_size,
        };
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::net::UnixStream;
        use std::path::PathBuf;
        use std::sync::mpsc::sync_channel;

        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-capture-capacity-{}-{}",
            std::process::id(),
            NEXT_BLOCKED_CAPTURE_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let _cleanup = RemoveFixture(root.clone());
        let server_dir = root.join("server");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();
        let storage = ServerStorage::initialize(&server_dir).unwrap();
        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from("printf 'CAPACITY-ROOT-READY\\r\\n'; while :; do /bin/sleep 1; done"),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();

        let (entered_send, entered_recv) = sync_channel(1);
        let (release_send, release_recv) = sync_channel(1);
        release_send.send(()).unwrap();
        driver.enable_history_backend_for_test(Box::new(BlockingHistoryBackend {
            entered: entered_send,
            release: Some(release_recv),
            observed_targets: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            completed: None,
        }));
        entered_recv
            .recv_timeout(Duration::from_secs(3))
            .expect("initial capture reaches the writer");
        {
            let shared = driver.history_capture.as_ref().unwrap();
            let mut capture = lock_capture(shared);
            for slot in 1..MAX_CAPTURE_TARGETS {
                assert!(capture.register_target(
                    TargetRef {
                        generation: target.generation,
                        workspace: target.workspace,
                        pane: 100 + slot as u64,
                        session: 100 + slot as u64,
                        occupant_revision: 1,
                    },
                    vt100::HistoryCursor {
                        epoch: 1,
                        ordinal: 0,
                    }
                ));
            }
        }

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let client_cwd = root.clone();
        let split_target = std::thread::scope(|scope| {
            let task = scope.spawn(move || {
                let deadline = || Instant::now() + Duration::from_secs(3);
                let mut stream = AuthenticatedStream::new(client_socket).unwrap();
                let hello = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 29,
                        required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                        optional_capabilities: 0,
                    },
                    resume: None,
                })
                .unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                        deadline(),
                    )
                    .unwrap();
                let welcome = stream.read_frame(deadline()).unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                let welcome = decode_welcome(welcome.payload()).unwrap();
                stream.enable_snapshot_reads().unwrap();
                let mut request_id = 0_u64;
                let mut send = |payload: &[u8]| {
                    request_id += 1;
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Request, request_id, payload).unwrap(),
                            deadline(),
                        )
                        .unwrap();
                    stream.read_frame(deadline()).unwrap()
                };
                let describe = encode_session_request(&SessionRequest::Describe).unwrap();
                let info = decode_session_info(send(&describe).payload()).unwrap();
                let key = OperationKey {
                    generation: welcome.generation,
                    lease: welcome.lease,
                    sequence: 1,
                };
                let claim = encode_session_request(&SessionRequest::Claim {
                    key,
                    target,
                    expected_owner_revision: info.owner_revision,
                    ttl_seconds: 30,
                })
                .unwrap();
                assert_eq!(send(&claim).kind(), MessageKind::Response);
                let layout_request = encode_layout_query();
                let layout = decode_layout_reply(send(&layout_request).payload()).unwrap();
                let (rows, columns) =
                    planned_split_size(&layout, target, SplitDirection::Right).unwrap();
                let split_key = OperationKey { sequence: 2, ..key };
                let arguments: [&[u8]; 2] = [
                    b"-c",
                    b"printf 'CAPACITY-SPLIT-READY\\r\\n'; while :; do /bin/sleep 1; done",
                ];
                let environment = [
                    EnvironmentEntry {
                        name: b"PATH",
                        value: b"/usr/bin:/bin",
                    },
                    EnvironmentEntry {
                        name: b"HOME",
                        value: client_cwd.as_os_str().as_bytes(),
                    },
                ];
                let launch = LaunchRequest {
                    key: split_key,
                    rows,
                    columns,
                    requested_executable: b"/bin/sh",
                    requested_working_directory: client_cwd.as_os_str().as_bytes(),
                    client_working_directory: client_cwd.as_os_str().as_bytes(),
                    arguments: &arguments,
                    environment: &environment,
                };
                let mut split_payload = Vec::new();
                encode_split_request(
                    &SplitRequest {
                        key: split_key,
                        target,
                        expected_layout_revision: layout.revision,
                        direction: SplitDirection::Right,
                        launch: &launch,
                    },
                    &mut split_payload,
                )
                .unwrap();
                let reply = send(&split_payload);
                assert_eq!(reply.kind(), MessageKind::Response);
                let result = decode_split_result(reply.payload()).unwrap();
                assert_eq!(result.outcome, Outcome::Applied);
                result.new_target.expect("child target").to_owned()
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !task.is_finished() {
                driver.tick(|| 100).unwrap();
                assert!(Instant::now() < deadline, "split control did not finish");
                std::thread::sleep(Duration::from_millis(1));
            }
            task.join().unwrap()
        });
        assert!(driver.history_capture_failed());

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let screen = std::thread::scope(|scope| {
            let task = scope.spawn(move || {
                let mut stream = AuthenticatedStream::new(client_socket).unwrap();
                let mut hello = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 30,
                        required_capabilities: CAPABILITY_READ,
                        optional_capabilities: 0,
                    },
                    resume: None,
                })
                .unwrap();
                hello.extend_from_slice(&encode_attach_target(split_target).unwrap());
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                        Instant::now() + Duration::from_secs(3),
                    )
                    .unwrap();
                let welcome = stream
                    .read_frame(Instant::now() + Duration::from_secs(3))
                    .unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                stream.enable_snapshot_reads().unwrap();
                let query = encode_screen_query(split_target).unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut request_id = 0_u64;
                loop {
                    request_id += 1;
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Request, request_id, &query).unwrap(),
                            Instant::now() + Duration::from_secs(3),
                        )
                        .unwrap();
                    let reply = stream
                        .read_frame(Instant::now() + Duration::from_secs(3))
                        .unwrap();
                    if reply.kind() == MessageKind::Snapshot {
                        let snapshot = decode_screen_reply(reply.payload()).unwrap();
                        let checkpoint = decode_checkpoint(snapshot.checkpoint()).unwrap();
                        let text = (0..checkpoint.rows())
                            .map(|row| {
                                (0..checkpoint.columns())
                                    .map(|column| checkpoint.cell(row, column).unwrap().text())
                                    .collect::<String>()
                            })
                            .collect::<String>();
                        if text.contains("CAPACITY-SPLIT-READY") {
                            return text;
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "uncaptured split child did not start"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !task.is_finished() {
                driver.tick(|| 100).unwrap();
                assert!(Instant::now() < deadline, "child screen query timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
            task.join().unwrap()
        });
        assert!(screen.contains("CAPACITY-SPLIT-READY"));
        drop(driver);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn persistent_history_writer_failure_does_not_stop_live_shell_input() {
        use crate::handshake_protocol::{
            decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ, CAPABILITY_SCREEN_ROW_WRAPS,
        };
        use crate::launch::LaunchContext;
        use crate::multi_startup::{StartedMultiServer, StartupLaunch};
        use crate::protocol::{OperationKey, OperationStatus, Outcome};
        use crate::protocol_wire::{Frame, MessageKind};
        use crate::response_protocol::{decode_response_payload, ResponseBody};
        use crate::screen_checkpoint::decode as decode_checkpoint;
        use crate::session_protocol::{
            decode_session_info, encode_session_hello, encode_session_request, SessionHello,
            SessionRequest,
        };
        use crate::terminal::Key;
        use crate::transport::AuthenticatedStream;
        use crate::ui_protocol::{
            decode_history_reply_with_wraps, decode_screen_reply_with_wraps, encode_history_query,
            encode_key_input, encode_screen_query, KeyInput,
        };
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::net::UnixStream;
        use std::path::PathBuf;

        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-live-history-failure-{}-{}",
            std::process::id(),
            NEXT_BLOCKED_CAPTURE_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let _cleanup = RemoveFixture(root.clone());
        let server_dir = root.join("server");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();
        let storage = ServerStorage::initialize(&server_dir).unwrap();
        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(format!(
                "printf 'FAILURE-READY\\r\\n'; IFS= read -r line; i=1; while [ \"$i\" -le 7 ]; do printf 'HISTORY-%s\\r\\n' \"$i\"; i=$((i + 1)); done; printf '%s\\r\\n' '{}'; printf 'AFTER-FAILURE:%s\\r\\n' \"$line\"; while :; do /bin/sleep 1; done",
                format_args!("WRAPMARK{}", "W".repeat(70))
            )),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();
        driver.enable_history_backend_for_test(Box::new(AlwaysFailHistoryBackend));
        assert!(driver.history_available());

        let failure_deadline = Instant::now() + Duration::from_secs(3);
        while !driver.history_capture_failed() {
            let tick_started = Instant::now();
            driver.tick(|| 100).unwrap();
            assert!(tick_started.elapsed() < Duration::from_millis(250));
            assert!(
                Instant::now() < failure_deadline,
                "backend failure was not reported"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server_socket).unwrap())
            .unwrap();
        let client = std::thread::scope(|scope| {
            let task = scope.spawn(move || {
                let deadline = || Instant::now() + Duration::from_secs(3);
                let mut stream = AuthenticatedStream::new(client_socket).unwrap();
                let hello = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 41,
                        required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                        optional_capabilities: CAPABILITY_SCREEN_ROW_WRAPS,
                    },
                    resume: None,
                })
                .unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
                        deadline(),
                    )
                    .unwrap();
                let welcome = stream.read_frame(deadline()).unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                let welcome = decode_welcome(welcome.payload()).unwrap();
                assert_ne!(
                    welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
                    0
                );
                stream.enable_snapshot_reads().unwrap();
                let mut request_id = 0_u64;
                let mut send = |payload: &[u8]| {
                    request_id += 1;
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Request, request_id, payload).unwrap(),
                            deadline(),
                        )
                        .unwrap();
                    stream.read_frame(deadline()).unwrap()
                };
                let describe = encode_session_request(&SessionRequest::Describe).unwrap();
                let info = decode_session_info(send(&describe).payload()).unwrap();
                let claim_key = OperationKey {
                    generation: welcome.generation,
                    lease: welcome.lease,
                    sequence: 1,
                };
                let claim = encode_session_request(&SessionRequest::Claim {
                    key: claim_key,
                    target,
                    expected_owner_revision: info.owner_revision,
                    ttl_seconds: 30,
                })
                .unwrap();
                let claim_reply = send(&claim);
                assert!(matches!(
                    decode_response_payload(claim_reply.payload()).unwrap().body,
                    ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                ));
                let current_info = decode_session_info(send(&describe).payload()).unwrap();
                let keys: Vec<Key> = "SURVIVES"
                    .chars()
                    .map(Key::Char)
                    .chain([Key::Enter])
                    .collect();
                let input_key = OperationKey {
                    sequence: 2,
                    ..claim_key
                };
                let input = encode_key_input(&KeyInput {
                    key: input_key,
                    target,
                    expected_owner_revision: current_info.owner_revision,
                    keys: &keys,
                })
                .unwrap();
                let input_reply = send(&input);
                assert!(matches!(
                    decode_response_payload(input_reply.payload()).unwrap().body,
                    ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                ));
                let query = encode_screen_query(target).unwrap();
                let deadline_at = Instant::now() + Duration::from_secs(3);
                loop {
                    let reply = send(&query);
                    if reply.kind() == MessageKind::Snapshot {
                        let screen = decode_screen_reply_with_wraps(reply.payload(), true).unwrap();
                        let checkpoint = decode_checkpoint(screen.checkpoint()).unwrap();
                        let text = (0..checkpoint.rows())
                            .map(|row| {
                                (0..checkpoint.columns())
                                    .map(|column| checkpoint.cell(row, column).unwrap().text())
                                    .collect::<String>()
                            })
                            .collect::<String>();
                        if text.contains("AFTER-FAILURE:SURVIVES") && text.contains("WRAPMARK") {
                            assert!(screen.wrap_metadata_available());
                            assert!(
                                (0..checkpoint.rows())
                                    .any(|row| screen.row_wrapped(row) == Some(true)),
                                "runtime snapshot omitted the child's actual soft-wrap row"
                            );
                            let history_query = encode_history_query(target, 1).unwrap();
                            let history_reply = send(&history_query);
                            assert_eq!(history_reply.kind(), MessageKind::Snapshot);
                            let history =
                                decode_history_reply_with_wraps(history_reply.payload(), true)
                                    .unwrap();
                            assert_eq!(history.offset_rows(), 1);
                            assert!(history.wrap_metadata_available());
                            let history_checkpoint =
                                decode_checkpoint(history.checkpoint()).unwrap();
                            assert!(
                                (0..history_checkpoint.rows())
                                    .any(|row| history.row_wrapped(row) == Some(true)),
                                "history reply did not carry wraps from its requested viewport"
                            );
                            return text;
                        }
                    }
                    assert!(
                        Instant::now() < deadline_at,
                        "shell did not process input after storage failure"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !task.is_finished() {
                let tick_started = Instant::now();
                driver.tick(|| 100).unwrap();
                assert!(tick_started.elapsed() < Duration::from_millis(250));
                assert!(
                    Instant::now() < deadline,
                    "socket control stalled after history failure"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            task.join().unwrap()
        });
        assert!(client.contains("AFTER-FAILURE:SURVIVES"));
        assert!(driver.history_capture_failed());
        drop(driver);
    }
}
