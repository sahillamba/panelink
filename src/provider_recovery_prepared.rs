//! One-shot, historical prepared-choice storage for manual provider recovery.
//!
//! Prepared choices are deliberately separate from selected provider bindings.
//! This record does not contain a `Binding`, prove current provider identity,
//! authorize navigation, or represent a resume receipt from the provider.

use crate::control_protocol::TargetRef;
use crate::managed_provider_store::ManagedProviderScope;
use crate::private_record_store::{AtomicPrivateRecordStore, PublishOutcome};
use crate::protocol::OperationKey;
use crate::provider_binding::{
    MAX_PROJECT_IDENTITY_BYTES, MAX_PROVIDER_SESSION_ID_BYTES, MAX_PROVIDER_VERSION_BYTES,
};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CURRENT_NAME: &str = "prepared-resume.record";
const TEMP_NAME: &str = ".prepared-resume.record.tmp";
const LOCK_NAME: &str = ".prepared-resume.record.lock";
const MAGIC: &[u8; 8] = b"PLPRSM01";
const HEADER_BYTES: usize = 153;
const MAX_EXECUTABLE_PATH_BYTES: usize = 4_096;
/// Maximum encoded prepared-recovery record bytes.
pub const MAX_PREPARED_RESUME_RECORD_BYTES: usize = HEADER_BYTES
    + MAX_PROVIDER_VERSION_BYTES
    + MAX_PROJECT_IDENTITY_BYTES
    + MAX_EXECUTABLE_PATH_BYTES
    + MAX_PROVIDER_SESSION_ID_BYTES;
/// Startup waits are bounded and belong outside the owner hot path.
pub const MAX_PREPARED_WORKER_STARTUP_WAIT: Duration = Duration::from_millis(250);
/// Explicit finish waits are bounded; a timed-out worker retains its store lock.
pub const MAX_PREPARED_WORKER_FINISH_WAIT: Duration = Duration::from_millis(250);

/// Random one-shot operation ticket, not a bridge grant or authentication secret.
#[derive(Clone, Eq, PartialEq)]
pub struct PreparedResumeTicket([u8; 16]);

impl PreparedResumeTicket {
    /// Parse the exact nonzero ticket bytes supplied by the crate's explicit
    /// recovery CLI. The store still checks that these bytes match its locked
    /// current Prepared record before allowing the one-way spend transition.
    pub fn from_bytes(bytes: [u8; 16]) -> io::Result<Self> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(refused());
        }
        Ok(Self(bytes))
    }

    /// Return the opaque ticket bytes for a caller-controlled recovery handoff.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl std::fmt::Debug for PreparedResumeTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedResumeTicket { <redacted> }")
    }
}

/// Receipt that one prepared choice was atomically published.
pub struct PreparedResumeReceipt {
    ticket: PreparedResumeTicket,
    prospective_revision: u64,
}

impl PreparedResumeReceipt {
    /// Ticket identifying this one prepared operation.
    pub fn ticket(&self) -> &PreparedResumeTicket {
        &self.ticket
    }

    /// Binding revision reserved for a future independently verified selection.
    pub const fn prospective_revision(&self) -> u64 {
        self.prospective_revision
    }
}

impl std::fmt::Debug for PreparedResumeReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedResumeReceipt { <redacted> }")
    }
}

/// Receipt that the one-shot operation was durably spent before navigation.
pub struct SpentResumeReceipt {
    prospective_revision: u64,
}

impl SpentResumeReceipt {
    /// Binding revision associated with the consumed operation.
    pub const fn prospective_revision(&self) -> u64 {
        self.prospective_revision
    }
}

impl std::fmt::Debug for SpentResumeReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpentResumeReceipt { <redacted> }")
    }
}

/// Persisted phase of the explicit manual recovery operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedResumeState {
    /// Choice and source validation metadata were saved; navigation is not yet issued.
    Prepared,
    /// The one-shot ticket was saved as spent before navigation may be issued.
    Spent,
}

/// Historical prepared-choice record; it does not represent a selected binding.
pub struct PreparedResumeRecord {
    scope: ManagedProviderScope,
    choice_id: String,
    prospective_revision: u64,
    source_bridge_instance: u64,
    host_validation_unix_ms: u64,
    ticket: PreparedResumeTicket,
    state: PreparedResumeState,
}

impl PreparedResumeRecord {
    /// Exact configured managed target and provider scope.
    pub fn scope(&self) -> &ManagedProviderScope {
        &self.scope
    }

    /// Exact explicit provider session choice.
    pub fn choice_id(&self) -> &str {
        &self.choice_id
    }

    /// Prospective revision for a new, independently verified binding.
    pub const fn prospective_revision(&self) -> u64 {
        self.prospective_revision
    }

    /// Bridge instance that supplied the pre-navigation validation.
    pub const fn source_bridge_instance(&self) -> u64 {
        self.source_bridge_instance
    }

    /// Host wall-clock validation timestamp in Unix milliseconds.
    pub const fn host_validation_unix_ms(&self) -> u64 {
        self.host_validation_unix_ms
    }

    /// Opaque operation ticket for the one-shot prepared/spent transition.
    pub fn ticket(&self) -> &PreparedResumeTicket {
        &self.ticket
    }

    /// Persisted operation phase; historical when loaded.
    pub const fn state(&self) -> PreparedResumeState {
        self.state
    }
}

impl std::fmt::Debug for PreparedResumeRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedResumeRecord { <redacted> }")
    }
}

/// Locked, target-scoped one-slot prepared recovery writer.
pub struct PreparedResumeStore {
    slot: AtomicPrivateRecordStore,
    scope: ManagedProviderScope,
}

impl std::fmt::Debug for PreparedResumeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedResumeStore { <redacted> }")
    }
}

impl PreparedResumeStore {
    /// Open this exact target's prepared-choice slot and take its lifetime lock.
    pub fn open(path: &Path, scope: ManagedProviderScope) -> io::Result<Self> {
        let slot = AtomicPrivateRecordStore::open(
            path,
            CURRENT_NAME,
            TEMP_NAME,
            LOCK_NAME,
            MAX_PREPARED_RESUME_RECORD_BYTES,
        )?;
        Ok(Self { slot, scope })
    }

    /// Load one historical prepared/spent choice for this exact scope.
    pub fn load(&self) -> io::Result<Option<PreparedResumeRecord>> {
        let Some(bytes) = self.slot.read()? else {
            return Ok(None);
        };
        let record = decode(&bytes)?;
        if record.scope != self.scope {
            return Err(invalid_record());
        }
        Ok(Some(record))
    }

    /// Persist the first explicit choice and return its ticket only after sync.
    ///
    /// The slot is never overwritten: an existing Prepared or Spent record
    /// requires a separate target/generation and a new operator decision.
    pub fn prepare(
        &mut self,
        choice_id: &str,
        prospective_revision: u64,
        source_bridge_instance: u64,
        host_validation_unix_ms: u64,
    ) -> io::Result<PreparedResumeReceipt> {
        validate_choice(choice_id)?;
        if prospective_revision == 0 || source_bridge_instance == 0 || host_validation_unix_ms == 0
        {
            return Err(refused());
        }
        if self.slot.read()?.is_some() {
            return Err(refused());
        }
        let ticket = new_ticket()?;
        let record = PreparedResumeRecord {
            scope: self.scope.clone(),
            choice_id: choice_id.to_owned(),
            prospective_revision,
            source_bridge_instance,
            host_validation_unix_ms,
            ticket: ticket.clone(),
            state: PreparedResumeState::Prepared,
        };
        let bytes = encode(&record)?;
        match self.slot.publish(&bytes)? {
            PublishOutcome::Written => Ok(PreparedResumeReceipt {
                ticket,
                prospective_revision,
            }),
            // A concurrent writer cannot hold the same lock; this can only be
            // an unexpected exact duplicate and must not mint a second receipt.
            PublishOutcome::AlreadyDurable => Err(refused()),
        }
    }

    /// Persist the irreversible Spent state before the caller issues navigation.
    pub fn mark_spent(
        &mut self,
        ticket: &PreparedResumeTicket,
        prospective_revision: u64,
    ) -> io::Result<SpentResumeReceipt> {
        let mut record = self.load()?.ok_or_else(refused)?;
        if record.state != PreparedResumeState::Prepared
            || &record.ticket != ticket
            || record.prospective_revision != prospective_revision
        {
            return Err(refused());
        }
        record.state = PreparedResumeState::Spent;
        let bytes = encode(&record)?;
        match self.slot.publish(&bytes)? {
            PublishOutcome::Written => Ok(SpentResumeReceipt {
                prospective_revision,
            }),
            PublishOutcome::AlreadyDurable => Err(refused()),
        }
    }

    /// Explicitly discard a recognized interrupted temp, never promote it.
    pub fn discard_pending(&self) -> io::Result<bool> {
        self.slot.discard_pending()
    }
}

/// Operation represented by a one-in-flight worker token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedWorkerOperation {
    /// Prepare the exact explicit choice.
    Prepare,
    /// Irreversibly mark the exact ticket as spent.
    Spend,
}

/// Nonzero immutable identity for one accepted prepared-store operation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PreparedWorkerToken {
    target: TargetRef,
    sequence: u64,
    revision: u64,
    operation: PreparedWorkerOperation,
}

impl PreparedWorkerToken {
    /// Exact managed target.
    pub const fn target(self) -> TargetRef {
        self.target
    }

    /// Monotonic nonzero request number.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Binding revision included in the request.
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Operation accepted by the worker.
    pub const fn operation(self) -> PreparedWorkerOperation {
        self.operation
    }
}

impl std::fmt::Debug for PreparedWorkerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedWorkerToken { <redacted> }")
    }
}

/// Result of one nonblocking worker submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedWorkerSubmit {
    /// The one available request was accepted.
    Accepted(PreparedWorkerToken),
    /// A prior request has not yet been consumed by `poll`.
    Busy,
    /// Inputs did not satisfy the typed bounded contract.
    Invalid,
    /// Initialization failed, the worker stopped, or a storage error latched.
    Unavailable,
    /// The monotonic token counter cannot advance without wrapping.
    SequenceExhausted,
}

/// Fixed failure category returned by the prepared store worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedWorkerError {
    /// Opening or loading the store failed.
    InitializationFailed,
    /// A prepare/spend save failed or became durability-ambiguous.
    StorageFailed,
    /// The worker stopped before it produced a completion.
    WorkerStopped,
}

/// Result proof constructed only by a completed worker operation.
pub enum PreparedWorkerResult {
    /// The first prepared record was atomically written.
    Prepared(PreparedResumeReceipt),
    /// The ticket was atomically marked spent before any navigation.
    Spent(SpentResumeReceipt),
}

impl std::fmt::Debug for PreparedWorkerResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedWorkerResult { <redacted> }")
    }
}

/// Exact worker-generated completion proof. Fields cannot be caller-forged.
pub struct PreparedWorkerCompletion {
    token: PreparedWorkerToken,
    result: Option<Result<PreparedWorkerResult, PreparedWorkerError>>,
}

impl PreparedWorkerCompletion {
    /// Accepted operation identity to match against owner state.
    pub const fn token(&self) -> PreparedWorkerToken {
        self.token
    }

    /// Take the worker-produced result exactly once.
    pub fn take_result(&mut self) -> Option<Result<PreparedWorkerResult, PreparedWorkerError>> {
        self.result.take()
    }
}

impl std::fmt::Debug for PreparedWorkerCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedWorkerCompletion { <redacted> }")
    }
}

/// Nonblocking state returned by the worker poll.
pub enum PreparedWorkerPoll {
    /// Initial store open/load has not completed yet.
    Pending,
    /// Store opened and loaded its historical prepared/spent row.
    Ready(Option<PreparedResumeRecord>),
    /// Store initialization failed.
    InitializationFailed,
    /// No write completion is ready.
    NoCompletion,
    /// One accepted request completed; owner must match its token.
    Completed(PreparedWorkerCompletion),
}

impl std::fmt::Debug for PreparedWorkerPoll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pending => "Pending",
            Self::Ready(_) => "Ready { <redacted> }",
            Self::InitializationFailed => "InitializationFailed",
            Self::NoCompletion => "NoCompletion",
            Self::Completed(_) => "Completed { <redacted> }",
        })
    }
}

/// Explicit bounded worker shutdown result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedWorkerFinish {
    /// The worker released the store before the deadline.
    Finished,
    /// The worker may still be in storage IO and retains its lock.
    TimedOut,
}

enum PreparedCommand {
    Prepare {
        token: PreparedWorkerToken,
        choice_id: String,
        source_bridge_instance: u64,
        host_validation_unix_ms: u64,
    },
    Spend {
        token: PreparedWorkerToken,
        ticket: PreparedResumeTicket,
    },
}

/// Owner-side one-in-flight adapter; submit and poll never perform disk IO.
pub struct AsyncPreparedResumeWorker {
    scope: ManagedProviderScope,
    sender: Option<SyncSender<PreparedCommand>>,
    init: Receiver<Result<Option<PreparedResumeRecord>, ()>>,
    completions: Receiver<PreparedWorkerCompletion>,
    stopped: Receiver<()>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    sequence: u64,
    inflight: Option<PreparedWorkerToken>,
    initialized: bool,
    failed: bool,
}

impl std::fmt::Debug for AsyncPreparedResumeWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AsyncPreparedResumeWorker { <redacted> }")
    }
}

impl AsyncPreparedResumeWorker {
    /// Spawn a dedicated store owner. Open/load run only on its thread.
    pub fn start(path: impl AsRef<Path>, scope: ManagedProviderScope) -> io::Result<Self> {
        Self::start_inner(
            path.as_ref().to_owned(),
            scope,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    fn start_with_test_gate(
        path: impl AsRef<Path>,
        scope: ManagedProviderScope,
        gate: TestGate,
        point: TestGatePoint,
    ) -> io::Result<Self> {
        Self::start_inner(path.as_ref().to_owned(), scope, Some((gate, point)))
    }

    fn start_inner(
        path: std::path::PathBuf,
        scope: ManagedProviderScope,
        #[cfg(test)] test_gate: Option<(TestGate, TestGatePoint)>,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (init_sender, init) = mpsc::sync_channel(1);
        let (completion_sender, completions) = mpsc::sync_channel(1);
        let (stopped_sender, stopped) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let thread_scope = scope.clone();
        let thread = thread::Builder::new()
            .name("panelink-prepared-recovery-writer".into())
            .spawn(move || {
                prepared_worker_entry(PreparedWorkerTask {
                    path,
                    scope: thread_scope,
                    receiver,
                    init: init_sender,
                    completions: completion_sender,
                    stopped: stopped_sender,
                    cancel: thread_cancel,
                    #[cfg(test)]
                    test_gate,
                });
            })?;
        Ok(Self {
            scope,
            sender: Some(sender),
            init,
            completions,
            stopped,
            cancel,
            thread: Some(thread),
            sequence: 1,
            inflight: None,
            initialized: false,
            failed: false,
        })
    }

    /// Whether initialization, an I/O error, worker loss, or cancellation latched failure.
    pub fn is_unavailable(&self) -> bool {
        self.failed || self.cancel.load(Ordering::Acquire)
    }

    /// Wait for startup outside the hot path, capped at 250 ms.
    pub fn wait_ready(&mut self, wait: Duration) -> PreparedWorkerPoll {
        if self.initialized {
            return PreparedWorkerPoll::NoCompletion;
        }
        match self
            .init
            .recv_timeout(wait.min(MAX_PREPARED_WORKER_STARTUP_WAIT))
        {
            Ok(Ok(record)) => {
                self.initialized = true;
                PreparedWorkerPoll::Ready(record)
            }
            Ok(Err(())) | Err(RecvTimeoutError::Disconnected) => {
                self.failed = true;
                PreparedWorkerPoll::InitializationFailed
            }
            Err(RecvTimeoutError::Timeout) => PreparedWorkerPoll::Pending,
        }
    }

    /// Try to persist a new explicit choice without blocking or queue growth.
    pub fn submit_prepare(
        &mut self,
        choice_id: &str,
        revision: u64,
        source_bridge_instance: u64,
        host_validation_unix_ms: u64,
    ) -> PreparedWorkerSubmit {
        if !self.initialized || self.failed || self.cancel.load(Ordering::Acquire) {
            return PreparedWorkerSubmit::Unavailable;
        }
        if validate_choice(choice_id).is_err()
            || revision == 0
            || source_bridge_instance == 0
            || host_validation_unix_ms == 0
        {
            return PreparedWorkerSubmit::Invalid;
        }
        if self.inflight.is_some() {
            return PreparedWorkerSubmit::Busy;
        }
        self.submit(PreparedCommand::Prepare {
            token: PreparedWorkerToken {
                target: self.scope.target(),
                sequence: self.sequence,
                revision,
                operation: PreparedWorkerOperation::Prepare,
            },
            choice_id: choice_id.to_owned(),
            source_bridge_instance,
            host_validation_unix_ms,
        })
    }

    /// Try to persist the irreversible spent marker for this exact ticket.
    pub fn submit_spend(
        &mut self,
        ticket: &PreparedResumeTicket,
        revision: u64,
    ) -> PreparedWorkerSubmit {
        if !self.initialized || self.failed || self.cancel.load(Ordering::Acquire) {
            return PreparedWorkerSubmit::Unavailable;
        }
        if revision == 0 {
            return PreparedWorkerSubmit::Invalid;
        }
        if self.inflight.is_some() {
            return PreparedWorkerSubmit::Busy;
        }
        self.submit(PreparedCommand::Spend {
            token: PreparedWorkerToken {
                target: self.scope.target(),
                sequence: self.sequence,
                revision,
                operation: PreparedWorkerOperation::Spend,
            },
            ticket: ticket.clone(),
        })
    }

    fn submit(&mut self, command: PreparedCommand) -> PreparedWorkerSubmit {
        let token = match &command {
            PreparedCommand::Prepare { token, .. } | PreparedCommand::Spend { token, .. } => *token,
        };
        let Some(next) = self.sequence.checked_add(1) else {
            return PreparedWorkerSubmit::SequenceExhausted;
        };
        let Some(sender) = self.sender.as_ref() else {
            return PreparedWorkerSubmit::Unavailable;
        };
        match sender.try_send(command) {
            Ok(()) => {
                self.sequence = next;
                self.inflight = Some(token);
                PreparedWorkerSubmit::Accepted(token)
            }
            Err(TrySendError::Full(_)) => PreparedWorkerSubmit::Busy,
            Err(TrySendError::Disconnected(_)) => {
                self.failed = true;
                PreparedWorkerSubmit::Unavailable
            }
        }
    }

    /// Poll initial load or one completion without waiting.
    pub fn poll(&mut self) -> PreparedWorkerPoll {
        if !self.initialized {
            return match self.try_init() {
                Some(result) => result,
                None => PreparedWorkerPoll::Pending,
            };
        }
        match self.completions.try_recv() {
            Ok(completion) => {
                if self.inflight != Some(completion.token) {
                    self.failed = true;
                    self.inflight = None;
                    return PreparedWorkerPoll::NoCompletion;
                }
                self.inflight = None;
                if completion.result.as_ref().is_some_and(Result::is_err) {
                    self.failed = true;
                }
                PreparedWorkerPoll::Completed(completion)
            }
            Err(TryRecvError::Empty) => PreparedWorkerPoll::NoCompletion,
            Err(TryRecvError::Disconnected) => {
                self.failed = true;
                PreparedWorkerPoll::NoCompletion
            }
        }
    }

    fn try_init(&mut self) -> Option<PreparedWorkerPoll> {
        match self.init.try_recv() {
            Ok(Ok(record)) => {
                self.initialized = true;
                Some(PreparedWorkerPoll::Ready(record))
            }
            Ok(Err(())) | Err(TryRecvError::Disconnected) => {
                self.failed = true;
                Some(PreparedWorkerPoll::InitializationFailed)
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    /// Cancel queued commands and bound the wait for an active storage call.
    pub fn finish(mut self, deadline: Instant) -> PreparedWorkerFinish {
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        self.thread.take();
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(MAX_PREPARED_WORKER_FINISH_WAIT);
        match self.stopped.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => PreparedWorkerFinish::Finished,
            Err(RecvTimeoutError::Timeout) => PreparedWorkerFinish::TimedOut,
        }
    }
}

impl Drop for AsyncPreparedResumeWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        self.thread.take();
    }
}

struct PreparedWorkerTask {
    path: std::path::PathBuf,
    scope: ManagedProviderScope,
    receiver: Receiver<PreparedCommand>,
    init: SyncSender<Result<Option<PreparedResumeRecord>, ()>>,
    completions: SyncSender<PreparedWorkerCompletion>,
    stopped: SyncSender<()>,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    test_gate: Option<(TestGate, TestGatePoint)>,
}

fn prepared_worker_entry(task: PreparedWorkerTask) {
    let PreparedWorkerTask {
        path,
        scope,
        receiver,
        init,
        completions,
        stopped,
        cancel,
        #[cfg(test)]
        test_gate,
    } = task;
    let Ok(mut store) = PreparedResumeStore::open(&path, scope.clone()) else {
        let _ = init.send(Err(()));
        let _ = stopped.try_send(());
        return;
    };
    match store.load() {
        Ok(record) => {
            let _ = init.send(Ok(record));
        }
        Err(_) => {
            drop(store);
            let _ = init.send(Err(()));
            let _ = stopped.try_send(());
            return;
        }
    }
    while !cancel.load(Ordering::Acquire) {
        #[cfg(test)]
        if let Some((gate, TestGatePoint::BeforeReceive)) = &test_gate {
            gate.pause_once();
        }
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let command = match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if cancel.load(Ordering::Acquire) {
            break;
        }
        #[cfg(test)]
        if let Some((gate, TestGatePoint::AfterReceive)) = &test_gate {
            gate.pause_once();
        }
        let completion = match command {
            PreparedCommand::Prepare {
                token,
                choice_id,
                source_bridge_instance,
                host_validation_unix_ms,
            } => PreparedWorkerCompletion {
                token,
                result: Some(
                    store
                        .prepare(
                            &choice_id,
                            token.revision,
                            source_bridge_instance,
                            host_validation_unix_ms,
                        )
                        .map(PreparedWorkerResult::Prepared)
                        .map_err(|_| PreparedWorkerError::StorageFailed),
                ),
            },
            PreparedCommand::Spend { token, ticket } => PreparedWorkerCompletion {
                token,
                result: Some(
                    store
                        .mark_spent(&ticket, token.revision)
                        .map(PreparedWorkerResult::Spent)
                        .map_err(|_| PreparedWorkerError::StorageFailed),
                ),
            },
        };
        let _ = completions.try_send(completion);
    }
    drop(store);
    let _ = stopped.try_send(());
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum TestGatePoint {
    BeforeReceive,
    AfterReceive,
}

#[cfg(test)]
#[derive(Clone)]
struct TestGate {
    state: Arc<(std::sync::Mutex<(bool, bool)>, std::sync::Condvar)>,
    entered_once: Arc<AtomicBool>,
}

#[cfg(test)]
impl TestGate {
    fn new() -> Self {
        Self {
            state: Arc::new((
                std::sync::Mutex::new((false, false)),
                std::sync::Condvar::new(),
            )),
            entered_once: Arc::new(AtomicBool::new(false)),
        }
    }

    fn pause_once(&self) {
        if self.entered_once.swap(true, Ordering::AcqRel) {
            return;
        }
        let (state, changed) = &*self.state;
        let mut current = state.lock().expect("test gate lock");
        current.0 = true;
        changed.notify_all();
        let started = Instant::now();
        while !current.1 {
            let remaining = Duration::from_secs(5).saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let (next, timeout) = changed
                .wait_timeout(current, remaining)
                .expect("test gate wait");
            current = next;
            if timeout.timed_out() {
                break;
            }
        }
    }

    fn wait_until_entered(&self, timeout: Duration) -> bool {
        let (state, changed) = &*self.state;
        let current = state.lock().expect("test gate lock");
        let (current, _) = changed
            .wait_timeout_while(current, timeout, |state| !state.0)
            .expect("test gate wait");
        current.0
    }

    fn release(&self) {
        let (state, changed) = &*self.state;
        state.lock().expect("test gate lock").1 = true;
        changed.notify_all();
    }
}

fn new_ticket() -> io::Result<PreparedResumeTicket> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(io::Error::other(
            "operating system returned an invalid recovery ticket",
        ));
    }
    Ok(PreparedResumeTicket(bytes))
}

fn validate_choice(choice_id: &str) -> io::Result<()> {
    let bytes = choice_id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_PROVIDER_SESSION_ID_BYTES
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.' | b'-'))
    {
        return Err(refused());
    }
    Ok(())
}

fn encode(record: &PreparedResumeRecord) -> io::Result<Vec<u8>> {
    if record.prospective_revision == 0
        || record.source_bridge_instance == 0
        || record.host_validation_unix_ms == 0
    {
        return Err(refused());
    }
    validate_choice(&record.choice_id)?;
    let (target, operation) = (record.scope.target(), record.scope.launch_operation());
    let project = record.scope.project();
    let version = record.scope.provider_version().as_bytes();
    let executable_path = record.scope.canonical_executable_path().unwrap_or_default();
    let choice = record.choice_id.as_bytes();
    let version_len = u16::try_from(version.len()).map_err(|_| refused())?;
    let project_len = u16::try_from(project.len()).map_err(|_| refused())?;
    let path_len = u16::try_from(executable_path.len()).map_err(|_| refused())?;
    let choice_len = u16::try_from(choice.len()).map_err(|_| refused())?;
    let total = HEADER_BYTES
        .checked_add(version.len())
        .and_then(|size| size.checked_add(project.len()))
        .and_then(|size| size.checked_add(executable_path.len()))
        .and_then(|size| size.checked_add(choice.len()))
        .filter(|size| *size <= MAX_PREPARED_RESUME_RECORD_BYTES)
        .ok_or_else(refused)?;

    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(MAGIC);
    for value in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
        operation.generation,
        operation.lease,
        operation.sequence,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(record.scope.executable_fingerprint().as_bytes());
    bytes.extend_from_slice(&record.prospective_revision.to_be_bytes());
    bytes.extend_from_slice(&record.source_bridge_instance.to_be_bytes());
    bytes.extend_from_slice(&record.host_validation_unix_ms.to_be_bytes());
    bytes.extend_from_slice(&record.ticket.0);
    bytes.push(match record.state {
        PreparedResumeState::Prepared => 1,
        PreparedResumeState::Spent => 2,
    });
    bytes.extend_from_slice(&version_len.to_be_bytes());
    bytes.extend_from_slice(&project_len.to_be_bytes());
    bytes.extend_from_slice(&path_len.to_be_bytes());
    bytes.extend_from_slice(&choice_len.to_be_bytes());
    debug_assert_eq!(bytes.len(), HEADER_BYTES);
    bytes.extend_from_slice(version);
    bytes.extend_from_slice(project);
    bytes.extend_from_slice(executable_path);
    bytes.extend_from_slice(choice);
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> io::Result<PreparedResumeRecord> {
    if bytes.len() < HEADER_BYTES
        || bytes.len() > MAX_PREPARED_RESUME_RECORD_BYTES
        || &bytes[..8] != MAGIC
    {
        return Err(invalid_record());
    }
    let target = TargetRef {
        generation: read_u64(bytes, 8)?,
        workspace: read_u64(bytes, 16)?,
        pane: read_u64(bytes, 24)?,
        session: read_u64(bytes, 32)?,
        occupant_revision: read_u64(bytes, 40)?,
    };
    let operation = OperationKey {
        generation: read_u64(bytes, 48)?,
        lease: read_u64(bytes, 56)?,
        sequence: read_u64(bytes, 64)?,
    };
    let fingerprint: [u8; 32] = bytes[72..104].try_into().map_err(|_| invalid_record())?;
    let prospective_revision = read_u64(bytes, 104)?;
    let source_bridge_instance = read_u64(bytes, 112)?;
    let host_validation_unix_ms = read_u64(bytes, 120)?;
    let ticket_bytes: [u8; 16] = bytes[128..144].try_into().map_err(|_| invalid_record())?;
    if ticket_bytes.iter().all(|byte| *byte == 0) {
        return Err(invalid_record());
    }
    let state = match bytes[144] {
        1 => PreparedResumeState::Prepared,
        2 => PreparedResumeState::Spent,
        _ => return Err(invalid_record()),
    };
    let version_len = read_u16(bytes, 145)?;
    let project_len = read_u16(bytes, 147)?;
    let path_len = read_u16(bytes, 149)?;
    let choice_len = read_u16(bytes, 151)?;
    if version_len == 0
        || version_len > MAX_PROVIDER_VERSION_BYTES
        || project_len == 0
        || project_len > MAX_PROJECT_IDENTITY_BYTES
        || path_len > MAX_EXECUTABLE_PATH_BYTES
        || choice_len == 0
        || choice_len > MAX_PROVIDER_SESSION_ID_BYTES
    {
        return Err(invalid_record());
    }
    let expected = HEADER_BYTES
        .checked_add(version_len)
        .and_then(|size| size.checked_add(project_len))
        .and_then(|size| size.checked_add(path_len))
        .and_then(|size| size.checked_add(choice_len))
        .ok_or_else(invalid_record)?;
    if expected != bytes.len() {
        return Err(invalid_record());
    }
    let mut offset = HEADER_BYTES;
    let version =
        std::str::from_utf8(&bytes[offset..offset + version_len]).map_err(|_| invalid_record())?;
    offset += version_len;
    let project = &bytes[offset..offset + project_len];
    offset += project_len;
    let executable_path = &bytes[offset..offset + path_len];
    offset += path_len;
    let choice =
        std::str::from_utf8(&bytes[offset..offset + choice_len]).map_err(|_| invalid_record())?;
    validate_choice(choice).map_err(|_| invalid_record())?;
    let scope = ManagedProviderScope::new(
        target,
        project,
        version,
        operation,
        fingerprint,
        (path_len != 0).then_some(executable_path),
    )
    .map_err(|_| invalid_record())?;
    if prospective_revision == 0 || source_bridge_instance == 0 || host_validation_unix_ms == 0 {
        return Err(invalid_record());
    }
    Ok(PreparedResumeRecord {
        scope,
        choice_id: choice.to_owned(),
        prospective_revision,
        source_bridge_instance,
        host_validation_unix_ms,
        ticket: PreparedResumeTicket(ticket_bytes),
        state,
    })
}

fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let value = bytes.get(offset..offset + 8).ok_or_else(invalid_record)?;
    Ok(u64::from_be_bytes(
        value.try_into().map_err(|_| invalid_record())?,
    ))
}

fn read_u16(bytes: &[u8], offset: usize) -> io::Result<usize> {
    let value = bytes.get(offset..offset + 2).ok_or_else(invalid_record)?;
    Ok(u16::from_be_bytes(value.try_into().map_err(|_| invalid_record())?) as usize)
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "prepared recovery action refused",
    )
}

fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid prepared recovery record",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_provider_store::ManagedProviderScope;
    use crate::private_record_store::FaultPoint;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: std::path::PathBuf,
        scope: ManagedProviderScope,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = Path::new("/private/tmp").join(format!("panelink-prepared-resume-{nonce}"));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let target = TargetRef {
                generation: 3,
                workspace: 4,
                pane: 5,
                session: 6,
                occupant_revision: 7,
            };
            let scope = ManagedProviderScope::new(
                target,
                b"/private/project",
                "2.0.32",
                OperationKey {
                    generation: 3,
                    lease: 8,
                    sequence: 9,
                },
                [0x5a; 32],
                Some(b"/usr/local/bin/opencode"),
            )
            .unwrap();
            Self { root, scope }
        }

        fn store(&self) -> PreparedResumeStore {
            PreparedResumeStore::open(&self.root, self.scope.clone()).unwrap()
        }

        fn prepare(&self, store: &mut PreparedResumeStore) -> PreparedResumeReceipt {
            store
                .prepare("session-A_1", 12, 13, 1_800_000_000_000)
                .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn prepare_is_durable_and_contains_choice_metadata_not_binding() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let receipt = fixture.prepare(&mut store);
        assert_ne!(receipt.ticket().0, [0; 16]);
        assert_eq!(receipt.ticket().as_bytes(), &receipt.ticket().0);
        assert!(PreparedResumeTicket::from_bytes([0; 16]).is_err());
        assert_eq!(
            *PreparedResumeTicket::from_bytes(*receipt.ticket().as_bytes())
                .unwrap()
                .as_bytes(),
            *receipt.ticket().as_bytes()
        );
        assert_eq!(receipt.prospective_revision(), 12);
        drop(store);
        let store = fixture.store();
        let record = store.load().unwrap().unwrap();
        assert_eq!(record.state(), PreparedResumeState::Prepared);
        assert_eq!(record.choice_id(), "session-A_1");
        assert_eq!(record.source_bridge_instance(), 13);
        assert_eq!(record.prospective_revision(), 12);
        assert_eq!(record.scope(), &fixture.scope);
    }

    #[test]
    fn existing_prepared_or_spent_record_is_never_overwritten() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let receipt = fixture.prepare(&mut store);
        assert!(store.prepare("another", 13, 13, 1_800_000_000_001).is_err());
        store
            .mark_spent(receipt.ticket(), receipt.prospective_revision())
            .unwrap();
        assert!(store.prepare("again", 14, 13, 1_800_000_000_002).is_err());
    }

    #[test]
    fn spent_transition_is_exact_one_shot_and_reopen_stays_spent() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let receipt = fixture.prepare(&mut store);
        assert!(store
            .mark_spent(receipt.ticket(), receipt.prospective_revision() + 1)
            .is_err());
        let spent = store
            .mark_spent(receipt.ticket(), receipt.prospective_revision())
            .unwrap();
        assert_eq!(spent.prospective_revision(), 12);
        assert!(store
            .mark_spent(receipt.ticket(), receipt.prospective_revision())
            .is_err());
        drop(store);
        let mut reopened = fixture.store();
        let record = reopened.load().unwrap().unwrap();
        assert_eq!(record.state(), PreparedResumeState::Spent);
        assert!(reopened
            .mark_spent(record.ticket(), record.prospective_revision())
            .is_err());
    }

    #[test]
    fn scope_mismatch_is_rejected_on_load_without_rewriting_record() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.prepare(&mut store);
        drop(store);
        let slot_path = fixture.root.join(CURRENT_NAME);
        let before = fs::read(&slot_path).unwrap();
        let target = fixture.scope.target();
        let operation = fixture.scope.launch_operation();
        let fingerprint = *fixture.scope.executable_fingerprint().as_bytes();
        let mismatches = [
            ManagedProviderScope::new(
                TargetRef { pane: 77, ..target },
                fixture.scope.project(),
                fixture.scope.provider_version(),
                operation,
                fingerprint,
                fixture.scope.canonical_executable_path(),
            ),
            ManagedProviderScope::new(
                target,
                b"/foreign/project",
                fixture.scope.provider_version(),
                operation,
                fingerprint,
                fixture.scope.canonical_executable_path(),
            ),
            ManagedProviderScope::new(
                target,
                fixture.scope.project(),
                "2.0.33",
                operation,
                fingerprint,
                fixture.scope.canonical_executable_path(),
            ),
            ManagedProviderScope::new(
                target,
                fixture.scope.project(),
                fixture.scope.provider_version(),
                OperationKey {
                    sequence: operation.sequence + 1,
                    ..operation
                },
                fingerprint,
                fixture.scope.canonical_executable_path(),
            ),
            ManagedProviderScope::new(
                target,
                fixture.scope.project(),
                fixture.scope.provider_version(),
                operation,
                [0x6b; 32],
                fixture.scope.canonical_executable_path(),
            ),
        ];
        for scope in mismatches {
            let store = PreparedResumeStore::open(&fixture.root, scope.unwrap()).unwrap();
            assert!(store.load().is_err());
        }
        assert_eq!(fs::read(slot_path).unwrap(), before);
    }

    #[test]
    fn corrupt_slot_and_invalid_choices_fail_closed() {
        for invalid in ["", "with space", "slash/id", "../bad", "has\nline"] {
            let fixture = Fixture::new();
            let mut store = fixture.store();
            assert!(store.prepare(invalid, 1, 2, 3).is_err());
        }
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.prepare(&mut store);
        drop(store);
        let path = fixture.root.join(CURRENT_NAME);
        fs::write(&path, b"corrupt").unwrap();
        assert!(fixture.store().load().is_err());
        assert_eq!(fs::read(path).unwrap(), b"corrupt");
    }

    #[test]
    fn post_rename_ambiguity_returns_no_receipt_and_prevents_retry() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store.slot.inject_test_fault(FaultPoint::DirectorySync);
        assert!(store
            .prepare("session-a", 12, 13, 1_800_000_000_000)
            .is_err());
        assert!(store.load().is_err());
        drop(store);
        let mut reopened = fixture.store();
        assert_eq!(
            reopened.load().unwrap().unwrap().state(),
            PreparedResumeState::Prepared
        );
        assert!(reopened
            .prepare("session-a", 12, 13, 1_800_000_000_000)
            .is_err());
    }

    #[test]
    fn spent_sync_ambiguity_reopens_spent_and_cannot_issue_navigation_twice() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let receipt = fixture.prepare(&mut store);
        store.slot.inject_test_fault(FaultPoint::DirectorySync);
        assert!(store
            .mark_spent(receipt.ticket(), receipt.prospective_revision())
            .is_err());
        drop(store);
        let mut reopened = fixture.store();
        let record = reopened.load().unwrap().unwrap();
        assert_eq!(record.state(), PreparedResumeState::Spent);
        assert!(reopened
            .mark_spent(record.ticket(), record.prospective_revision())
            .is_err());
    }

    fn wait_for_completion(worker: &mut AsyncPreparedResumeWorker) -> PreparedWorkerCompletion {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let PreparedWorkerPoll::Completed(completion) = worker.poll() {
                return completion;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("prepared worker did not complete within the test deadline");
    }

    fn wait_for_unlock(fixture: &Fixture) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(store) = PreparedResumeStore::open(&fixture.root, fixture.scope.clone()) {
                drop(store);
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("prepared worker did not release its store lock in time");
    }

    #[test]
    fn async_worker_is_bounded_matches_tokens_and_returns_spent_proof() {
        let fixture = Fixture::new();
        let mut worker =
            AsyncPreparedResumeWorker::start(&fixture.root, fixture.scope.clone()).unwrap();
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            PreparedWorkerPoll::Ready(None)
        ));
        let prepare_token = match worker.submit_prepare("session-async", 22, 23, 1_800_000_000_000)
        {
            PreparedWorkerSubmit::Accepted(token) => token,
            other => panic!("unexpected prepare submission: {other:?}"),
        };
        assert_eq!(prepare_token.target(), fixture.scope.target());
        assert_eq!(prepare_token.operation(), PreparedWorkerOperation::Prepare);
        assert_eq!(
            worker.submit_prepare("session-next", 23, 23, 1_800_000_000_001),
            PreparedWorkerSubmit::Busy
        );
        let mut completion = wait_for_completion(&mut worker);
        assert_eq!(completion.token(), prepare_token);
        let receipt = match completion.take_result().unwrap().unwrap() {
            PreparedWorkerResult::Prepared(receipt) => receipt,
            PreparedWorkerResult::Spent(_) => panic!("prepare returned spent receipt"),
        };
        let spend_token =
            match worker.submit_spend(receipt.ticket(), receipt.prospective_revision()) {
                PreparedWorkerSubmit::Accepted(token) => token,
                other => panic!("unexpected spend submission: {other:?}"),
            };
        assert_eq!(spend_token.operation(), PreparedWorkerOperation::Spend);
        assert!(spend_token.sequence() > prepare_token.sequence());
        let mut completion = wait_for_completion(&mut worker);
        assert_eq!(completion.token(), spend_token);
        assert!(matches!(
            completion.take_result().unwrap().unwrap(),
            PreparedWorkerResult::Spent(_)
        ));
        assert!(!worker.is_unavailable());
        assert_eq!(
            worker.finish(Instant::now() + MAX_PREPARED_WORKER_FINISH_WAIT),
            PreparedWorkerFinish::Finished
        );
    }

    #[test]
    fn drop_cancels_a_queued_command_without_spending_or_preparing() {
        let fixture = Fixture::new();
        let gate = TestGate::new();
        let mut worker = AsyncPreparedResumeWorker::start_with_test_gate(
            &fixture.root,
            fixture.scope.clone(),
            gate.clone(),
            TestGatePoint::BeforeReceive,
        )
        .unwrap();
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            PreparedWorkerPoll::Ready(None)
        ));
        assert!(gate.wait_until_entered(Duration::from_secs(1)));
        assert!(matches!(
            worker.submit_prepare("queued", 1, 2, 3),
            PreparedWorkerSubmit::Accepted(_)
        ));
        drop(worker);
        gate.release();
        wait_for_unlock(&fixture);
        assert!(fixture.store().load().unwrap().is_none());
    }

    #[test]
    fn bounded_finish_times_out_while_running_io_retains_lock_until_completion() {
        let fixture = Fixture::new();
        let gate = TestGate::new();
        let mut worker = AsyncPreparedResumeWorker::start_with_test_gate(
            &fixture.root,
            fixture.scope.clone(),
            gate.clone(),
            TestGatePoint::AfterReceive,
        )
        .unwrap();
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            PreparedWorkerPoll::Ready(None)
        ));
        assert!(matches!(
            worker.submit_prepare("running", 1, 2, 3),
            PreparedWorkerSubmit::Accepted(_)
        ));
        assert!(gate.wait_until_entered(Duration::from_secs(1)));
        assert_eq!(
            worker.finish(Instant::now() + Duration::from_millis(10)),
            PreparedWorkerFinish::TimedOut
        );
        assert!(PreparedResumeStore::open(&fixture.root, fixture.scope.clone()).is_err());
        gate.release();
        wait_for_unlock(&fixture);
        assert_eq!(
            fixture.store().load().unwrap().unwrap().state(),
            PreparedResumeState::Prepared
        );
    }

    #[test]
    fn async_worker_startup_failure_is_reported_without_owner_io() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join(CURRENT_NAME), b"corrupt").unwrap();
        fs::set_permissions(
            fixture.root.join(CURRENT_NAME),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let mut worker =
            AsyncPreparedResumeWorker::start(&fixture.root, fixture.scope.clone()).unwrap();
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            PreparedWorkerPoll::InitializationFailed
        ));
        assert_eq!(
            worker.submit_prepare("x", 1, 1, 1),
            PreparedWorkerSubmit::Unavailable
        );
    }
}
