//! Opt-in coordinator between a provisioned provider source and durable binding storage.
//!
//! The owner-facing methods only poll bounded channels and mutate the typed
//! receiver. All accept, handshake, snapshot, and storage work stays on workers.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::TargetRef;
use crate::managed_provider_store::{ManagedLaunchOutcome, ManagedProviderScope};
use crate::provider_binding::{Binding, BindingState, LastConfirmedRecord};
use crate::provider_binding_worker::{
    BindingFinishOutcome, BindingJobToken, BindingSubmit, BindingWorkerPoll, BindingWorkerScope,
    ProviderBindingWorker,
};
use crate::provider_bridge::{Admission, BridgeReceiver, LeasePolicy, SUPPORTED_PROVIDER_VERSION};
use crate::provider_bridge_transport::Grant;
use crate::provider_bridge_v2::{provision, FreshReader};
use crate::provider_recovery_control::{
    valid_session_id, valid_ticket, NavigateOutcome, ValidationOutcome,
};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
#[cfg(test)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Prototype policy for expiry and source polling.
pub const BRIDGE_LEASE: Duration = Duration::from_secs(3);
/// Requested interval between fresh source snapshots.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);
/// Maximum one handshake or snapshot operation may block the IO worker.
pub const IO_DEADLINE: Duration = Duration::from_millis(300);
/// Maximum shared stop wait for this coordinator.
pub const MAX_FINISH_WAIT: Duration = Duration::from_millis(250);

const MAX_RECEIPT_AGE: Duration = Duration::from_millis(3_000);
const ACCEPT_POLL: Duration = Duration::from_millis(10);
const SOURCE_START_TIMEOUT: Duration = Duration::from_secs(15);

/// Public lifecycle state. No session identifiers or credentials are included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeStatus {
    Initializing,
    Unbound,
    Pending,
    LastConfirmed,
    Stale,
    Ambiguous,
    Unavailable,
    StorageFailed,
}

/// Fixed metadata snapshot for the owner/UI. Debug output redacts target IDs.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RuntimeSnapshot {
    target: TargetRef,
    bridge_instance: Option<u64>,
    status: RuntimeStatus,
    last_confirmed_revision: Option<u64>,
}

impl RuntimeSnapshot {
    pub const fn target(self) -> TargetRef {
        self.target
    }
    pub const fn bridge_instance(self) -> Option<u64> {
        self.bridge_instance
    }
    pub const fn status(self) -> RuntimeStatus {
        self.status
    }
    pub const fn last_confirmed_revision(self) -> Option<u64> {
        self.last_confirmed_revision
    }
}

impl fmt::Debug for RuntimeSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeSnapshot { <redacted> }")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFinishOutcome {
    Finished,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlSubmitError {
    Unavailable,
    Busy,
    Invalid,
    Exhausted,
}

pub enum ControlReply {
    Validated {
        request_id: u64,
        outcome: ValidationOutcome,
        received_at: Instant,
    },
    Navigated {
        request_id: u64,
        outcome: NavigateOutcome,
        received_at: Instant,
    },
    Failed {
        request_id: u64,
    },
}

impl fmt::Debug for ControlReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validated {
                request_id,
                outcome,
                ..
            } => f
                .debug_struct("Validated")
                .field("request_id", request_id)
                .field("outcome", outcome)
                .finish(),
            Self::Navigated {
                request_id,
                outcome,
                ..
            } => f
                .debug_struct("Navigated")
                .field("request_id", request_id)
                .field("outcome", outcome)
                .finish(),
            Self::Failed { request_id } => f
                .debug_struct("Failed")
                .field("request_id", request_id)
                .finish(),
        }
    }
}

struct FrameReceipt {
    frame: crate::provider_bridge::Frame,
    received_at: Instant,
    verified_at_unix_ms: u64,
    reconciliation_epoch: u64,
}

struct IoReady {
    instance: u64,
}

/// Single-owner coordinator. Poll is nonblocking and consumes at most one source frame.
pub struct ProviderBridgeRuntime {
    target: TargetRef,
    project_identity: Vec<u8>,
    base: Instant,
    io_ready: Receiver<Result<IoReady, ()>>,
    frames: Receiver<FrameReceipt>,
    control_replies: Receiver<ControlReply>,
    io_done: Receiver<()>,
    io_stopped: Arc<AtomicBool>,
    io_failed: Arc<AtomicBool>,
    requested_epoch: Arc<AtomicU64>,
    snapshots_requested: Arc<AtomicBool>,
    reconciliation_epoch: u64,
    io_control: SyncSender<IoCommand>,
    next_control_id: u64,
    control_inflight: Option<u64>,
    validated_id: Option<String>,
    navigation_accepted: bool,
    io_thread: Option<JoinHandle<()>>,
    writer: Option<ProviderBindingWorker>,
    writer_ready: bool,
    loaded_record: Option<LastConfirmedRecord>,
    receiver: Option<BridgeReceiver>,
    instance: Option<u64>,
    pending_job: Option<BindingJobToken>,
    last_source_sequence: u64,
    storage_failed: bool,
    unavailable_latched: bool,
    finished: bool,
}

impl fmt::Debug for ProviderBridgeRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderBridgeRuntime { <redacted> }")
    }
}

impl ProviderBridgeRuntime {
    /// Start bounded IO and storage workers for one already-configured listener.
    /// The caller creates the private storage directory and grants the listener's
    /// lifecycle only after the managed target identity is known.
    pub fn start(
        path: &Path,
        target: TargetRef,
        project: &[u8],
        listener: UnixListener,
    ) -> io::Result<Self> {
        Self::start_inner(path, target, project, listener, None)
    }

    /// Production path: atomically publish the managed launch and selected binding.
    pub fn start_managed(
        path: &Path,
        scope: ManagedProviderScope,
        outcome: ManagedLaunchOutcome,
        listener: UnixListener,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let target = scope.target();
        let project = scope.project().to_vec();
        Self::start_source(
            path,
            target,
            &project,
            IoSource::Listener(listener),
            None,
            Some((scope, outcome)),
        )
    }

    fn start_inner(
        path: &Path,
        target: TargetRef,
        project: &[u8],
        listener: UnixListener,
        instance: Option<u64>,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        Self::start_source(
            path,
            target,
            project,
            IoSource::Listener(listener),
            instance,
            None,
        )
    }

    #[cfg(test)]
    fn start_with_stream(
        path: &Path,
        target: TargetRef,
        project: &[u8],
        stream: UnixStream,
        instance: u64,
    ) -> io::Result<Self> {
        Self::start_source(
            path,
            target,
            project,
            IoSource::Connected(stream),
            Some(instance),
            None,
        )
    }

    fn start_source(
        path: &Path,
        target: TargetRef,
        project: &[u8],
        source: IoSource,
        instance: Option<u64>,
        managed: Option<(ManagedProviderScope, ManagedLaunchOutcome)>,
    ) -> io::Result<Self> {
        let base = Instant::now();
        let mut writer_scope =
            BindingWorkerScope::new(path, target, project, SUPPORTED_PROVIDER_VERSION);
        if let Some((scope, outcome)) = managed {
            writer_scope = writer_scope.with_managed(scope, outcome)?;
        }
        let writer = ProviderBindingWorker::start(writer_scope)?;
        let (ready_tx, io_ready) = mpsc::sync_channel(1);
        let (frame_tx, frames) = mpsc::sync_channel(1);
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let (control_reply_tx, control_replies) = mpsc::sync_channel(1);
        let (done_tx, io_done) = mpsc::sync_channel(1);
        let io_stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = Arc::clone(&io_stopped);
        let io_failed = Arc::new(AtomicBool::new(false));
        let thread_failed = Arc::clone(&io_failed);
        let requested_epoch = Arc::new(AtomicU64::new(0));
        let thread_epoch = Arc::clone(&requested_epoch);
        let snapshots_requested = Arc::new(AtomicBool::new(false));
        let thread_snapshots = Arc::clone(&snapshots_requested);
        let io_project = project.to_vec();
        let thread = thread::Builder::new()
            .name("panelink-provider-bridge-io".into())
            .spawn(move || {
                io_worker(
                    IoWorkerConfig {
                        source,
                        target,
                        project: io_project,
                        instance,
                    },
                    IoWorkerChannels {
                        control: control_rx,
                        control_replies: control_reply_tx,
                        ready: ready_tx,
                        frames: frame_tx,
                        done: done_tx,
                        stopped: thread_stopped,
                        failed: thread_failed,
                        requested_epoch: thread_epoch,
                        snapshots_requested: thread_snapshots,
                    },
                );
            })?;
        Ok(Self {
            target,
            project_identity: project.to_vec(),
            base,
            io_ready,
            frames,
            control_replies,
            io_done,
            io_stopped,
            io_failed,
            requested_epoch,
            snapshots_requested,
            reconciliation_epoch: 0,
            io_control: control_tx,
            next_control_id: 0,
            control_inflight: None,
            validated_id: None,
            navigation_accepted: false,
            io_thread: Some(thread),
            writer: Some(writer),
            writer_ready: false,
            loaded_record: None,
            receiver: None,
            instance: None,
            pending_job: None,
            last_source_sequence: 0,
            storage_failed: false,
            unavailable_latched: false,
            finished: false,
        })
    }

    /// Process worker readiness, at most one source frame, one save completion,
    /// and idle lease expiry. It performs no disk or network IO and never waits.
    pub fn poll(&mut self, now: Instant) -> RuntimeSnapshot {
        self.poll_initialization();
        // Capture processing time after dequeuing. A worker can publish between
        // the caller reading its clock and this poll reading the channel.
        let receipt = self.receiver.is_some().then(|| self.frames.try_recv());
        let now = now.max(Instant::now());
        let now_ms = elapsed_ms(self.base, now);
        self.observe_io_failure(now_ms);
        if self
            .writer
            .as_ref()
            .is_some_and(ProviderBindingWorker::is_unavailable)
        {
            self.storage_failed = true;
            self.io_stopped.store(true, Ordering::Release);
        }

        if self.receiver.is_some() {
            match receipt.expect("receiver was initialized before dequeue") {
                Ok(receipt) => self.admit_receipt(receipt, now, now_ms),
                Err(TryRecvError::Disconnected) => self.latch_unavailable(now_ms),
                Err(TryRecvError::Empty) => {}
            }
            self.poll_completion(now_ms);
            if self
                .writer
                .as_ref()
                .is_some_and(ProviderBindingWorker::is_unavailable)
            {
                self.storage_failed = true;
                self.io_stopped.store(true, Ordering::Release);
            }
            if let Some(receiver) = self.receiver.as_mut() {
                if receiver.tick(now_ms).is_err() {
                    self.latch_unavailable(now_ms);
                }
            }
            self.schedule_current_pending();
        }
        self.snapshot()
    }

    /// Explicitly require a fresh post-request source snapshot before admission.
    pub fn request_reconciliation(&mut self) {
        if self.unavailable_latched
            || self.io_failed.load(Ordering::Acquire)
            || self.storage_failed
            || self
                .writer
                .as_ref()
                .is_some_and(ProviderBindingWorker::is_unavailable)
        {
            return;
        }
        let Some(epoch) = self.reconciliation_epoch.checked_add(1) else {
            self.latch_unavailable(elapsed_ms(self.base, Instant::now()));
            return;
        };
        let Some(receiver) = self.receiver.as_mut() else {
            return;
        };
        self.reconciliation_epoch = epoch;
        self.requested_epoch.store(epoch, Ordering::Release);
        receiver.request_reconciliation();
        self.snapshots_requested.store(true, Ordering::Release);
    }

    /// Submit an exact-ID read-only provider validation request.
    pub fn request_validation(&mut self, id: &str) -> Result<u64, ControlSubmitError> {
        if !valid_session_id(id) {
            return Err(ControlSubmitError::Invalid);
        }
        if !self.recovery_available() {
            return Err(ControlSubmitError::Unavailable);
        }
        if self.control_inflight.is_some() {
            return Err(ControlSubmitError::Busy);
        }
        let request_id = self
            .next_control_id
            .checked_add(1)
            .ok_or(ControlSubmitError::Exhausted)?;
        let command = IoCommand::Validate {
            request_id,
            id: id.to_owned(),
        };
        match self.io_control.try_send(command) {
            Ok(()) => {
                self.next_control_id = request_id;
                self.control_inflight = Some(request_id);
                Ok(request_id)
            }
            Err(TrySendError::Full(_)) => Err(ControlSubmitError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(ControlSubmitError::Unavailable),
        }
    }

    /// Submit the runtime's sole navigation attempt. A canceled accepted request
    /// is still consumed and returns `NotIssued`; it cannot be retried here.
    pub fn request_navigation(
        &mut self,
        id: &str,
        revision: u64,
        ticket: [u8; 16],
        cancel: Arc<AtomicBool>,
    ) -> Result<u64, ControlSubmitError> {
        if !valid_session_id(id) || revision == 0 || !valid_ticket(&ticket) {
            return Err(ControlSubmitError::Invalid);
        }
        if self.validated_id.as_deref() != Some(id) {
            return Err(ControlSubmitError::Unavailable);
        }
        if !self.recovery_available() {
            return Err(ControlSubmitError::Unavailable);
        }
        if self.control_inflight.is_some() {
            return Err(ControlSubmitError::Busy);
        }
        if self.navigation_accepted {
            return Err(ControlSubmitError::Unavailable);
        }
        let request_id = self
            .next_control_id
            .checked_add(1)
            .ok_or(ControlSubmitError::Exhausted)?;
        let command = IoCommand::Navigate {
            request_id,
            id: id.to_owned(),
            revision,
            ticket,
            cancel,
        };
        match self.io_control.try_send(command) {
            Ok(()) => {
                self.next_control_id = request_id;
                self.control_inflight = Some(request_id);
                self.navigation_accepted = true;
                Ok(request_id)
            }
            Err(TrySendError::Full(_)) => Err(ControlSubmitError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(ControlSubmitError::Unavailable),
        }
    }

    /// Consume the one bounded reply. A mismatched reply permanently fails closed.
    pub fn poll_control_reply(&mut self) -> Option<ControlReply> {
        let reply = match self.control_replies.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                if self.control_inflight.is_some() {
                    self.unavailable_latched = true;
                }
                return None;
            }
        };
        let request_id = match &reply {
            ControlReply::Validated { request_id, .. }
            | ControlReply::Navigated { request_id, .. }
            | ControlReply::Failed { request_id } => *request_id,
        };
        if self.control_inflight != Some(request_id) {
            self.unavailable_latched = true;
            return None;
        }
        self.control_inflight = None;
        if let ControlReply::Validated { outcome, .. } = &reply {
            self.validated_id = match outcome {
                ValidationOutcome::Verified(session) => Some(session.session_id().to_owned()),
                _ => None,
            };
        }
        if matches!(reply, ControlReply::Failed { .. }) {
            self.unavailable_latched = true;
        }
        Some(reply)
    }

    /// Next unreserved revision, available only while both workers and the source are ready.
    pub fn next_binding_revision(&self) -> Option<u64> {
        if !self.recovery_available() {
            return None;
        }
        self.receiver.as_ref()?.revision_floor().checked_add(1)
    }

    /// Return only an already-confirmed exact-ID revision after the caller has polled.
    pub fn confirmed_revision_for(&self, id: &str, minimum_revision: u64) -> Option<u64> {
        if !self.recovery_available() {
            return None;
        }
        let receiver = self.receiver.as_ref()?;
        if receiver.state() != BindingState::Verified {
            return None;
        }
        receiver.last_confirmed().and_then(|binding| {
            (binding.session_id() == id && binding.revision() >= minimum_revision)
                .then_some(binding.revision())
        })
    }

    fn recovery_available(&self) -> bool {
        self.writer_ready
            && self.receiver.is_some()
            && self.instance.is_some()
            && !self.unavailable_latched
            && !self.storage_failed
            && !self.io_failed.load(Ordering::Acquire)
            && !self
                .writer
                .as_ref()
                .is_none_or(ProviderBindingWorker::is_unavailable)
    }

    /// Read fixed status and last-confirmed revision without IO.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let last_confirmed_revision = self
            .receiver
            .as_ref()
            .and_then(BridgeReceiver::last_confirmed)
            .map(Binding::revision);
        let status = if self.storage_failed {
            RuntimeStatus::StorageFailed
        } else if self.unavailable_latched || self.io_failed.load(Ordering::Acquire) {
            RuntimeStatus::Unavailable
        } else if !self.writer_ready || self.receiver.is_none() {
            RuntimeStatus::Initializing
        } else {
            match self.receiver.as_ref().map(BridgeReceiver::state) {
                Some(BindingState::Unbound) => RuntimeStatus::Unbound,
                Some(BindingState::Pending) => RuntimeStatus::Pending,
                Some(BindingState::Verified) => RuntimeStatus::LastConfirmed,
                Some(BindingState::Stale) => RuntimeStatus::Stale,
                Some(BindingState::Ambiguous) => RuntimeStatus::Ambiguous,
                Some(BindingState::Unavailable) | None => RuntimeStatus::Unavailable,
            }
        };
        RuntimeSnapshot {
            target: self.target,
            bridge_instance: self.instance,
            status,
            last_confirmed_revision,
        }
    }

    /// Cancel IO and storage admission, then wait only within one shared deadline.
    pub fn finish(mut self, deadline: Instant) -> RuntimeFinishOutcome {
        let stop_deadline = deadline.min(Instant::now() + MAX_FINISH_WAIT);
        self.io_stopped.store(true, Ordering::Release);
        self.io_thread.take();
        let io_finished = matches!(
            self.io_done
                .recv_timeout(stop_deadline.saturating_duration_since(Instant::now())),
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
        );
        let writer_result = self
            .writer
            .take()
            .map(|writer| writer.finish(stop_deadline));
        self.finished = true;
        if io_finished
            && writer_result.is_none_or(|outcome| outcome == BindingFinishOutcome::Finished)
        {
            RuntimeFinishOutcome::Finished
        } else {
            RuntimeFinishOutcome::TimedOut
        }
    }

    fn poll_initialization(&mut self) {
        if self.receiver.is_none() {
            match self.io_ready.try_recv() {
                Ok(Ok(ready)) => self.instance = Some(ready.instance),
                Ok(Err(())) | Err(TryRecvError::Disconnected) => self.unavailable_latched = true,
                Err(TryRecvError::Empty) => {}
            }
            if let Some(writer) = self.writer.as_mut() {
                match writer.poll() {
                    BindingWorkerPoll::Ready(ready) => {
                        self.writer_ready = true;
                        self.loaded_record = ready.last_confirmed;
                    }
                    BindingWorkerPoll::InitializationFailed => {
                        self.storage_failed = true;
                        self.io_stopped.store(true, Ordering::Release);
                    }
                    _ => {}
                }
            }
            if self.receiver.is_none() && self.writer_ready && self.instance.is_some() {
                let record = self.loaded_record.take();
                self.build_receiver(record);
            }
        }
    }

    fn build_receiver(&mut self, record: Option<LastConfirmedRecord>) {
        if self.receiver.is_some() {
            return;
        }
        let Some(instance) = self.instance else {
            return;
        };
        let policy = match LeasePolicy::new(BRIDGE_LEASE.as_millis() as u64) {
            Ok(policy) => policy,
            Err(_) => {
                self.unavailable_latched = true;
                return;
            }
        };
        let receiver = match record {
            Some(record) => {
                let revision_floor = record.binding().revision();
                BridgeReceiver::restore(
                    self.target,
                    &self.project_identity,
                    instance,
                    policy,
                    record,
                    revision_floor,
                )
            }
            None => BridgeReceiver::new(self.target, &self.project_identity, instance, policy),
        };
        match receiver {
            Ok(receiver) => {
                self.receiver = Some(receiver);
                self.request_reconciliation();
            }
            Err(_) => self.unavailable_latched = true,
        }
    }

    fn admit_receipt(&mut self, receipt: FrameReceipt, now: Instant, now_ms: u64) {
        let age = now.checked_duration_since(receipt.received_at);
        if age.is_none_or(|age| age >= MAX_RECEIPT_AGE) {
            self.latch_unavailable(now_ms);
            return;
        }
        let sequence = receipt.frame.sequence();
        if self.last_source_sequence.checked_add(1) != Some(sequence) {
            self.latch_unavailable(now_ms);
            return;
        }
        self.last_source_sequence = sequence;
        let frame_ms = elapsed_ms(self.base, receipt.received_at);
        let Some(receiver) = self.receiver.as_mut() else {
            return;
        };
        if receipt.reconciliation_epoch != self.reconciliation_epoch {
            if receiver
                .discard_for_reconciliation(receipt.frame, now_ms, frame_ms)
                .is_err()
            {
                self.latch_unavailable(now_ms);
            }
            return;
        }
        let should_reconcile_after_route = matches!(
            receipt.frame.selection(),
            crate::provider_bridge::Selection::Home
                | crate::provider_bridge::Selection::Child
                | crate::provider_bridge::Selection::Unknown
        );
        match receiver.receive_at(receipt.frame, now_ms, frame_ms, receipt.verified_at_unix_ms) {
            Ok(Admission::Pending { .. }) => self.schedule_current_pending(),
            Ok(Admission::Revoked) if should_reconcile_after_route => {
                self.request_reconciliation();
            }
            Ok(_) => {}
            Err(_) => self.latch_unavailable(now_ms),
        }
    }

    fn poll_completion(&mut self, now_ms: u64) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let mut completion = match writer.poll() {
            BindingWorkerPoll::Completed(completion) => completion,
            BindingWorkerPoll::InitializationFailed => {
                self.storage_failed = true;
                return;
            }
            _ => return,
        };
        if completion.result().is_err() {
            self.storage_failed = true;
            self.io_stopped.store(true, Ordering::Release);
            return;
        }
        let token = completion.token();
        if self.pending_job != Some(token) {
            self.storage_failed = true;
            self.io_stopped.store(true, Ordering::Release);
            return;
        }
        let Some(durable) = completion.take_durable_binding() else {
            self.storage_failed = true;
            return;
        };
        let Some(receiver) = self.receiver.as_mut() else {
            return;
        };
        let matches_pending = receiver
            .pending()
            .is_some_and(|pending| pending == &durable)
            && token.target() == durable.target()
            && token.bridge_instance() == durable.bridge_instance()
            && token.revision() == durable.revision();
        if !matches_pending {
            self.pending_job = None;
            self.schedule_current_pending();
            return;
        }
        if receiver
            .confirm_pending(token.revision(), now_ms, |pending| {
                if pending == &durable {
                    Ok(())
                } else {
                    Err(())
                }
            })
            .is_err()
        {
            self.pending_job = None;
            self.schedule_current_pending();
            return;
        }
        self.pending_job = None;
        self.schedule_current_pending();
    }

    fn schedule_current_pending(&mut self) {
        if self.pending_job.is_some() || self.storage_failed || self.unavailable_latched {
            return;
        }
        let Some(pending) = self.receiver.as_ref().and_then(BridgeReceiver::pending) else {
            return;
        };
        if let Some(writer) = self.writer.as_mut() {
            if let BindingSubmit::Accepted(token) = writer.submit(pending) {
                self.pending_job = Some(token);
            }
        }
    }

    fn observe_io_failure(&mut self, now_ms: u64) {
        if self.io_failed.load(Ordering::Acquire) && !self.unavailable_latched {
            self.latch_unavailable(now_ms);
        }
    }

    fn latch_unavailable(&mut self, now_ms: u64) {
        self.unavailable_latched = true;
        if let Some(receiver) = self.receiver.as_mut() {
            let _ = receiver.mark_unavailable(now_ms);
        }
    }
}

impl Drop for ProviderBridgeRuntime {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.io_stopped.store(true, Ordering::Release);
        self.io_thread.take();
        self.writer.take();
    }
}

enum IoCommand {
    Validate {
        request_id: u64,
        id: String,
    },
    Navigate {
        request_id: u64,
        id: String,
        revision: u64,
        ticket: [u8; 16],
        cancel: Arc<AtomicBool>,
    },
}

enum IoSource {
    Listener(UnixListener),
    #[cfg(test)]
    Connected(UnixStream),
}

struct IoWorkerConfig {
    source: IoSource,
    target: TargetRef,
    project: Vec<u8>,
    instance: Option<u64>,
}

struct IoWorkerChannels {
    control: Receiver<IoCommand>,
    control_replies: SyncSender<ControlReply>,
    ready: SyncSender<Result<IoReady, ()>>,
    frames: SyncSender<FrameReceipt>,
    done: SyncSender<()>,
    stopped: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    requested_epoch: Arc<AtomicU64>,
    snapshots_requested: Arc<AtomicBool>,
}

fn io_worker(config: IoWorkerConfig, channels: IoWorkerChannels) {
    let IoWorkerConfig {
        source,
        target,
        project,
        instance,
    } = config;
    let IoWorkerChannels {
        control,
        control_replies,
        ready,
        frames,
        done,
        stopped,
        failed,
        requested_epoch,
        snapshots_requested,
    } = channels;
    let result = (|| {
        let instance = match instance {
            Some(instance) if instance != 0 => instance,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "bridge instance is invalid",
                ))
            }
            None => random_instance()?,
        };
        let source_deadline = Instant::now() + SOURCE_START_TIMEOUT;
        let mut stream = match source {
            #[cfg(test)]
            IoSource::Connected(stream) => stream,
            IoSource::Listener(listener) => loop {
                if stopped.load(Ordering::Acquire) {
                    return Ok(());
                }
                if Instant::now() >= source_deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "provider source unavailable",
                    ));
                }
                let accepted = {
                    let _guard = crate::spawn_gate::acquire_until(source_deadline)?;
                    listener.accept()
                };
                match accepted {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(ACCEPT_POLL)
                    }
                    Err(error) => return Err(error),
                }
            },
        };
        let grant = Grant::issue(target, &project, SUPPORTED_PROVIDER_VERSION, instance)?;
        let handshake_deadline = Instant::now() + IO_DEADLINE;
        provision(&mut stream, &grant, handshake_deadline)?;
        let mut reader = FreshReader::accept(stream, grant, handshake_deadline)?;
        ready
            .send(Ok(IoReady { instance }))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "bridge owner stopped"))?;
        let mut next_snapshot = Instant::now();
        loop {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            match control.try_recv() {
                Ok(IoCommand::Validate { request_id, id }) => {
                    match reader.validate_session(&id, Instant::now() + IO_DEADLINE) {
                        Ok(outcome) => {
                            let reply = ControlReply::Validated {
                                request_id,
                                outcome,
                                received_at: Instant::now(),
                            };
                            if control_replies.try_send(reply).is_err() {
                                return Err(io::Error::new(
                                    io::ErrorKind::WouldBlock,
                                    "control reply queue full",
                                ));
                            }
                        }
                        Err(_) => {
                            let _ = control_replies.try_send(ControlReply::Failed { request_id });
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "provider validation failed",
                            ));
                        }
                    }
                    continue;
                }
                Ok(IoCommand::Navigate {
                    request_id,
                    id,
                    revision,
                    ticket,
                    cancel,
                }) => {
                    if cancel.load(Ordering::Acquire) {
                        if control_replies
                            .try_send(ControlReply::Navigated {
                                request_id,
                                outcome: NavigateOutcome::NotIssued,
                                received_at: Instant::now(),
                            })
                            .is_err()
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "control reply queue full",
                            ));
                        }
                    } else {
                        match reader.navigate_session(
                            &id,
                            revision,
                            &ticket,
                            Instant::now() + IO_DEADLINE,
                        ) {
                            Ok(outcome) => {
                                if control_replies
                                    .try_send(ControlReply::Navigated {
                                        request_id,
                                        outcome,
                                        received_at: Instant::now(),
                                    })
                                    .is_err()
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::WouldBlock,
                                        "control reply queue full",
                                    ));
                                }
                            }
                            Err(_) => {
                                let _ =
                                    control_replies.try_send(ControlReply::Failed { request_id });
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "provider navigation failed",
                                ));
                            }
                        }
                    }
                    continue;
                }
                Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => {}
            }
            if !snapshots_requested.load(Ordering::Acquire) {
                thread::sleep(ACCEPT_POLL);
                continue;
            }
            if Instant::now() < next_snapshot {
                thread::sleep(ACCEPT_POLL);
                continue;
            }
            let reconciliation_epoch = requested_epoch.load(Ordering::Acquire);
            let frame = reader.snapshot(Instant::now() + IO_DEADLINE)?;
            let receipt = FrameReceipt {
                frame,
                received_at: Instant::now(),
                verified_at_unix_ms: unix_ms(),
                reconciliation_epoch,
            };
            match frames.try_send(receipt) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "bridge owner is behind",
                    ))
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
            next_snapshot = Instant::now() + SNAPSHOT_INTERVAL;
        }
        Ok(())
    })();
    if result.is_err() && !stopped.load(Ordering::Acquire) {
        failed.store(true, Ordering::Release);
        let _ = ready.try_send(Err(()));
    }
    let _ = done.try_send(());
}

fn random_instance() -> io::Result<u64> {
    loop {
        let mut bytes = [0u8; 8];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let instance = u64::from_ne_bytes(bytes);
        if instance != 0 {
            return Ok(instance);
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn elapsed_ms(base: Instant, now: Instant) -> u64 {
    now.checked_duration_since(base)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::VerificationSource;
    use crate::provider_binding_store::ProviderBindingStore;
    use crate::provider_bridge::{Frame, FrameScope, Selection};
    use crate::provider_bridge_v2::FreshSender;
    use crate::provider_recovery_control::ValidationDecision;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    struct Fixture {
        root: PathBuf,
        store: PathBuf,
        target: TargetRef,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = Path::new("/private/tmp").join(format!("panelink-bridge-runtime-{nonce}"));
            let store = root.join("store");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("private fixture");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&store)
                .expect("private store");
            Self {
                root,
                store,
                target: TargetRef {
                    generation: 31,
                    workspace: 37,
                    pane: 41,
                    session: 43,
                    occupant_revision: 47,
                },
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn provisioned_source_is_reconciled_and_confirmed_after_storage() {
        // The caller can read its clock before the IO worker publishes a
        // receipt. Dequeue-time processing must not reject that receipt as
        // coming from the future or regress expiry on repeated owner polls.
        let caller_clock_before_publication = Instant::now();
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            53,
        )
        .expect("runtime");
        let target = fixture.target;
        let sequences = Arc::new(AtomicU64::new(0));
        let source_sequences = Arc::clone(&sequences);
        let source = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(
                source_stream,
                Instant::now() + Duration::from_secs(2),
            )
            .expect("provisioned handshake");
            loop {
                let sequence = source_sequences.fetch_add(1, Ordering::AcqRel) + 1;
                let result = sender.respond_once(Instant::now() + Duration::from_secs(2), || {
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        53,
                        sequence,
                        FrameScope::TopLevel,
                        Selection::session(
                            "session_runtime",
                            VerificationSource::TuiRouteSnapshot,
                        )?,
                    )
                });
                if result.is_err() {
                    break;
                }
            }
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut status = RuntimeStatus::Initializing;
        while Instant::now() < deadline {
            status = runtime.poll(caller_clock_before_publication).status();
            if status == RuntimeStatus::LastConfirmed {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(status, RuntimeStatus::LastConfirmed);
        assert_eq!(runtime.snapshot().last_confirmed_revision(), Some(1));
        let finish = runtime.finish(Instant::now() + Duration::from_secs(2));
        assert!(matches!(
            finish,
            RuntimeFinishOutcome::Finished | RuntimeFinishOutcome::TimedOut
        ));
        let _ = source.join();
        assert!(sequences.load(Ordering::Acquire) >= 1);
    }

    #[test]
    fn owner_poll_returns_while_listener_and_storage_initialize() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            59,
        )
        .expect("runtime");
        let started = Instant::now();
        let status = runtime.poll(Instant::now()).status();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(matches!(
            status,
            RuntimeStatus::Initializing | RuntimeStatus::Unbound
        ));
        drop(source_stream);
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
    }

    #[test]
    fn queued_old_epoch_is_consumed_but_cannot_satisfy_new_reconciliation() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            61,
        )
        .expect("runtime");
        let target = fixture.target;
        let responses = Arc::new(AtomicU64::new(0));
        let source_responses = Arc::clone(&responses);
        let source = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(
                source_stream,
                Instant::now() + Duration::from_secs(2),
            )
            .expect("provisioned handshake");
            let mut sequence = 0;
            loop {
                let result = sender.respond_once(Instant::now() + Duration::from_secs(2), || {
                    sequence += 1;
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        61,
                        sequence,
                        FrameScope::TopLevel,
                        Selection::session(
                            "session_after_barrier",
                            VerificationSource::TuiRouteSnapshot,
                        )?,
                    )
                });
                if result.is_err() {
                    break;
                }
                source_responses.fetch_add(1, Ordering::AcqRel);
            }
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.receiver.is_none() && Instant::now() < deadline {
            runtime.poll(Instant::now());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(runtime.receiver.is_some(), "runtime initialized");
        while responses.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            responses.load(Ordering::Acquire) > 0,
            "old-epoch response sent"
        );
        thread::sleep(Duration::from_millis(5));
        runtime.request_reconciliation();
        assert_eq!(runtime.reconciliation_epoch, 2);
        runtime.poll(Instant::now());
        assert!(runtime
            .receiver
            .as_ref()
            .expect("receiver")
            .pending()
            .is_none());

        let mut status = RuntimeStatus::Stale;
        while Instant::now() < deadline {
            status = runtime.poll(Instant::now()).status();
            if status == RuntimeStatus::LastConfirmed {
                break;
            }
            thread::sleep(Duration::from_millis(3));
        }
        assert_eq!(status, RuntimeStatus::LastConfirmed);
        assert_eq!(runtime.snapshot().last_confirmed_revision(), Some(1));
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
        let _ = source.join();
    }

    #[test]
    fn home_revocation_schedules_a_fresh_barrier_for_a_later_top_level_session() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            63,
        )
        .expect("runtime");
        let target = fixture.target;
        let source = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(
                source_stream,
                Instant::now() + Duration::from_secs(2),
            )
            .expect("provisioned handshake");
            let mut sequence = 0;
            loop {
                let result = sender.respond_once(Instant::now() + Duration::from_secs(2), || {
                    sequence += 1;
                    let selection = match sequence {
                        2 => Selection::Home,
                        3.. => Selection::session(
                            "session_after_home",
                            VerificationSource::TuiRouteSnapshot,
                        )?,
                        _ => Selection::session(
                            "session_before_home",
                            VerificationSource::TuiRouteSnapshot,
                        )?,
                    };
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        63,
                        sequence,
                        FrameScope::TopLevel,
                        selection,
                    )
                });
                if result.is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut status = RuntimeStatus::Initializing;
        while Instant::now() < deadline {
            status = runtime.poll(Instant::now()).status();
            if runtime.snapshot().last_confirmed_revision() == Some(2) {
                break;
            }
            thread::sleep(Duration::from_millis(3));
        }
        assert_eq!(status, RuntimeStatus::LastConfirmed);
        assert_eq!(runtime.snapshot().last_confirmed_revision(), Some(2));
        assert_eq!(runtime.reconciliation_epoch, 2);
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
        let _ = source.join();
    }

    #[test]
    fn newer_pending_binding_is_saved_after_older_completion_is_rejected() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            67,
        )
        .expect("runtime");
        let target = fixture.target;
        let source = FreshSender::connect_provisioned(
            source_stream,
            Instant::now() + Duration::from_secs(2),
        )
        .expect("provisioned handshake");
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.receiver.is_none() && Instant::now() < deadline {
            runtime.poll(Instant::now());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(runtime.receiver.is_some(), "runtime initialized");
        runtime.io_stopped.store(true, Ordering::Release);

        let received_at = Instant::now();
        let received_ms = elapsed_ms(runtime.base, received_at);
        let make_frame = |sequence, id: &str| {
            Frame::new(
                1,
                target,
                b"/private/project",
                SUPPORTED_PROVIDER_VERSION,
                67,
                sequence,
                FrameScope::TopLevel,
                Selection::session(id, VerificationSource::TuiRouteSnapshot).unwrap(),
            )
            .unwrap()
        };
        runtime.admit_receipt(
            FrameReceipt {
                frame: make_frame(1, "session_first"),
                received_at,
                verified_at_unix_ms: unix_ms(),
                reconciliation_epoch: runtime.reconciliation_epoch,
            },
            received_at,
            received_ms,
        );
        let second_at = Instant::now();
        runtime.admit_receipt(
            FrameReceipt {
                frame: make_frame(2, "session_newer"),
                received_at: second_at,
                verified_at_unix_ms: unix_ms(),
                reconciliation_epoch: runtime.reconciliation_epoch,
            },
            second_at,
            elapsed_ms(runtime.base, second_at),
        );
        assert_eq!(runtime.pending_job.map(BindingJobToken::revision), Some(1));
        assert_eq!(
            runtime
                .receiver
                .as_ref()
                .and_then(BridgeReceiver::pending)
                .map(Binding::revision),
            Some(2)
        );

        while Instant::now() < deadline
            && runtime.pending_job.map(BindingJobToken::revision) == Some(1)
        {
            runtime.poll_completion(elapsed_ms(runtime.base, Instant::now()));
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(runtime.pending_job.map(BindingJobToken::revision), Some(2));
        while Instant::now() < deadline && runtime.snapshot().last_confirmed_revision() != Some(2) {
            runtime.poll_completion(elapsed_ms(runtime.base, Instant::now()));
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(runtime.snapshot().last_confirmed_revision(), Some(2));
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
        let store = ProviderBindingStore::open(
            &fixture.store,
            fixture.target,
            b"/private/project",
            SUPPORTED_PROVIDER_VERSION,
        )
        .expect("reopen store");
        let saved = store.load().expect("load saved binding").expect("record");
        assert_eq!(saved.binding().revision(), 2);
        assert_eq!(saved.binding().session_id(), "session_newer");
        drop(source);
    }

    #[test]
    fn validation_is_bounded_read_only_and_canceled_navigation_is_one_shot() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            71,
        )
        .expect("runtime");
        let target = fixture.target;
        let initial_snapshot_sent = Arc::new(AtomicBool::new(false));
        let source_initial_snapshot_sent = Arc::clone(&initial_snapshot_sent);
        let source = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(
                source_stream,
                Instant::now() + Duration::from_secs(2),
            )
            .expect("provisioned handshake");
            let mut sequence = 0;
            sender
                .respond_once(Instant::now() + Duration::from_secs(2), || {
                    sequence += 1;
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        71,
                        sequence,
                        FrameScope::TopLevel,
                        Selection::session(
                            "session_selected",
                            VerificationSource::TuiRouteSnapshot,
                        )
                        .expect("selection"),
                    )
                })
                .expect("initial snapshot");
            source_initial_snapshot_sent.store(true, Ordering::Release);
            sender
                .respond_validate_once(Instant::now() + Duration::from_secs(2), |id| {
                    assert_eq!(id, "session_manual");
                    ValidationDecision::Verified {
                        project_identity: b"/private/project".to_vec(),
                    }
                })
                .expect("validation request");
            loop {
                let result = sender.respond_once(Instant::now() + Duration::from_secs(2), || {
                    sequence += 1;
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        71,
                        sequence,
                        FrameScope::TopLevel,
                        Selection::session(
                            "session_selected",
                            VerificationSource::TuiRouteSnapshot,
                        )
                        .expect("selection"),
                    )
                });
                if result.is_err() {
                    break;
                }
            }
        });

        let ready_deadline = Instant::now() + Duration::from_secs(2);
        while (runtime.receiver.is_none()
            || !runtime.writer_ready
            || !initial_snapshot_sent.load(Ordering::Acquire))
            && Instant::now() < ready_deadline
        {
            runtime.poll(Instant::now());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(runtime.receiver.is_some());
        let request_id = runtime
            .request_validation("session_manual")
            .expect("submit validation");
        assert_eq!(
            runtime.request_validation("session_other"),
            Err(ControlSubmitError::Busy)
        );
        let reply_deadline = Instant::now() + Duration::from_secs(2);
        let reply = loop {
            if let Some(reply) = runtime.poll_control_reply() {
                break reply;
            }
            assert!(Instant::now() < reply_deadline, "validation reply arrived");
            thread::sleep(Duration::from_millis(2));
        };
        match reply {
            ControlReply::Validated {
                request_id: returned,
                outcome: ValidationOutcome::Verified(session),
                ..
            } => {
                assert_eq!(returned, request_id);
                assert_eq!(session.session_id(), "session_manual");
                assert_eq!(session.project_identity(), b"/private/project");
            }
            other => panic!("unexpected validation result: {other:?}"),
        }
        let cancel = Arc::new(AtomicBool::new(true));
        let nav_id = runtime
            .request_navigation("session_manual", 9, [1; 16], cancel)
            .expect("accepted canceled navigation");
        let nav_deadline = Instant::now() + Duration::from_secs(2);
        let reply = loop {
            if let Some(reply) = runtime.poll_control_reply() {
                break reply;
            }
            assert!(Instant::now() < nav_deadline, "navigation reply arrived");
            thread::sleep(Duration::from_millis(2));
        };
        assert!(matches!(
            reply,
            ControlReply::Navigated {
                request_id,
                outcome: NavigateOutcome::NotIssued,
                ..
            } if request_id == nav_id
        ));
        assert_eq!(
            runtime.request_navigation(
                "session_manual",
                10,
                [2; 16],
                Arc::new(AtomicBool::new(false))
            ),
            Err(ControlSubmitError::Unavailable)
        );
        assert_eq!(runtime.confirmed_revision_for("session_manual", 1), None);
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
        source.join().expect("source exits on runtime shutdown");
    }

    #[test]
    fn failed_control_reply_latches_source_unavailable_without_reconnect() {
        let fixture = Fixture::new();
        let (runtime_stream, source_stream) = UnixStream::pair().expect("connected test stream");
        let mut runtime = ProviderBridgeRuntime::start_with_stream(
            &fixture.store,
            fixture.target,
            b"/private/project",
            runtime_stream,
            73,
        )
        .expect("runtime");
        let target = fixture.target;
        let first_snapshot = Arc::new(AtomicBool::new(false));
        let source_first_snapshot = Arc::clone(&first_snapshot);
        let source = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(
                source_stream,
                Instant::now() + Duration::from_secs(2),
            )
            .expect("provisioned handshake");
            sender
                .respond_once(Instant::now() + Duration::from_secs(2), || {
                    Frame::new(
                        1,
                        target,
                        b"/private/project",
                        SUPPORTED_PROVIDER_VERSION,
                        73,
                        1,
                        FrameScope::TopLevel,
                        Selection::session("session_source", VerificationSource::TuiRouteSnapshot)
                            .expect("selection"),
                    )
                })
                .expect("initial snapshot");
            source_first_snapshot.store(true, Ordering::Release);
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !first_snapshot.load(Ordering::Acquire) && Instant::now() < deadline {
            runtime.poll(Instant::now());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(first_snapshot.load(Ordering::Acquire));
        source
            .join()
            .expect("source exits after closing connection");
        let request_id = runtime
            .request_validation("session_manual")
            .expect("submit before worker observes closure");
        let reply_deadline = Instant::now() + Duration::from_secs(2);
        let reply = loop {
            runtime.poll(Instant::now());
            if let Some(reply) = runtime.poll_control_reply() {
                break reply;
            }
            assert!(Instant::now() < reply_deadline, "failed reply arrived");
            thread::sleep(Duration::from_millis(2));
        };
        assert!(matches!(reply, ControlReply::Failed { request_id: id } if id == request_id));
        assert_eq!(runtime.snapshot().status(), RuntimeStatus::Unavailable);
        assert_eq!(
            runtime.request_validation("session_manual"),
            Err(ControlSubmitError::Unavailable)
        );
        // The failure reply is sent before the worker's completion signal.
        // Receiving the reply does not establish that the worker has exited.
        assert!(matches!(
            runtime.io_done.recv_timeout(Duration::from_secs(2)),
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        let _ = runtime.finish(Instant::now() + Duration::from_secs(1));
    }
}
