//! One-in-flight asynchronous adapter for durable provider-binding writes.
//!
//! Disk open/load/save work runs on a dedicated thread that owns the store's
//! lifetime lock. The owner submits by nonblocking try-send and can confirm a
//! binding only by consuming the worker-created successful completion.

use crate::control_protocol::TargetRef;
use crate::managed_provider_store::{
    ManagedLaunchOutcome, ManagedLifecycleSnapshot, ManagedProviderRecord, ManagedProviderScope,
    ManagedProviderStore, ManagedPublishOutcome,
};
use crate::provider_binding::{Binding, LastConfirmedRecord};
use crate::provider_binding_codec;
use crate::provider_binding_store::{ProviderBindingStore, SaveOutcome};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Maximum startup wait callers may request for the worker's initial load.
pub const MAX_BINDING_STARTUP_WAIT: Duration = Duration::from_millis(250);
/// Maximum explicit finish wait; a timeout detaches while the worker retains its lock.
pub const MAX_BINDING_FINISH_WAIT: Duration = Duration::from_millis(250);

/// Complete configured identity for a binding store. Debug output is redacted.
pub struct BindingWorkerScope {
    path: PathBuf,
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    managed: Option<(ManagedProviderScope, ManagedLaunchOutcome)>,
}

impl BindingWorkerScope {
    /// Configure one exact target, project, and provider-version store.
    pub fn new(path: impl AsRef<Path>, target: TargetRef, project: &[u8], version: &str) -> Self {
        Self {
            path: path.as_ref().to_owned(),
            target,
            project: project.to_vec(),
            version: version.to_owned(),
            managed: None,
        }
    }

    /// Couple every saved binding to the exact durable managed launch metadata.
    pub fn with_managed(
        mut self,
        scope: ManagedProviderScope,
        outcome: ManagedLaunchOutcome,
    ) -> io::Result<Self> {
        if scope.target() != self.target
            || scope.project() != self.project
            || scope.provider_version() != self.version
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "managed binding scope mismatch",
            ));
        }
        self.managed = Some((scope, outcome));
        Ok(self)
    }
}

impl fmt::Debug for BindingWorkerScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingWorkerScope { <redacted> }")
    }
}

/// Nonzero identity of one accepted storage request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BindingJobToken {
    target: TargetRef,
    bridge_instance: u64,
    revision: u64,
    issuance_sequence: u64,
}

impl BindingJobToken {
    /// Configured target, including generation and occupant revision.
    pub const fn target(self) -> TargetRef {
        self.target
    }
    /// Bridge instance bound into the submitted binding.
    pub const fn bridge_instance(self) -> u64 {
        self.bridge_instance
    }
    /// Provider-observation revision bound into the submitted binding.
    pub const fn revision(self) -> u64 {
        self.revision
    }
    /// Monotonic nonzero request number assigned by this worker handle.
    pub const fn issuance_sequence(self) -> u64 {
        self.issuance_sequence
    }
}

impl fmt::Debug for BindingJobToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingJobToken { <redacted> }")
    }
}

/// Result of a nonblocking request attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingSubmit {
    /// The worker accepted the one available request.
    Accepted(BindingJobToken),
    /// A previous request has not yet been consumed by the owner.
    Busy,
    /// The binding does not match the configured target and project scope.
    WrongScope,
    /// The bounded codec refused to copy the binding for worker ownership.
    InvalidBinding,
    /// Startup failed, the worker stopped, or a prior storage error latched it.
    Unavailable,
    /// The issuance counter cannot advance without wrapping.
    SequenceExhausted,
}

/// Fixed, redacted worker error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingWorkerError {
    /// Opening the store or loading its initial record failed.
    InitializationFailed,
    /// The durable save returned an error; commit state may be uncertain.
    SaveFailed,
    /// The worker thread ended before returning a result.
    WorkerStopped,
}

/// Initial store state, returned only after open and load run on the worker.
pub struct BindingWorkerReady {
    pub last_confirmed: Option<LastConfirmedRecord>,
}

impl fmt::Debug for BindingWorkerReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingWorkerReady { <redacted> }")
    }
}

/// Completion proof created only after the worker's save call returns.
pub struct BindingCompletion {
    token: BindingJobToken,
    result: Result<SaveOutcome, BindingWorkerError>,
    durable_binding: Option<Binding>,
}

impl BindingCompletion {
    /// Exact accepted request identity.
    pub const fn token(&self) -> BindingJobToken {
        self.token
    }
    /// Save result. Success is emitted only after store sync completion.
    pub const fn result(&self) -> Result<SaveOutcome, BindingWorkerError> {
        self.result
    }
    /// Take the exact submitted value for owner adoption after successful save.
    pub fn take_durable_binding(&mut self) -> Option<Binding> {
        self.durable_binding.take()
    }
}

impl fmt::Debug for BindingCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingCompletion { <redacted> }")
    }
}

/// Nonblocking worker poll result.
pub enum BindingWorkerPoll {
    /// No initialization response is ready yet.
    Pending,
    /// Store opened and loaded successfully.
    Ready(BindingWorkerReady),
    /// Store open or initial load failed; the worker is fail-closed.
    InitializationFailed,
    /// No write completion is ready yet.
    NoCompletion,
    /// One accepted save has completed and must be matched to its token.
    Completed(BindingCompletion),
}

impl fmt::Debug for BindingWorkerPoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "Pending",
            Self::Ready(_) => "Ready { <redacted> }",
            Self::InitializationFailed => "InitializationFailed",
            Self::NoCompletion => "NoCompletion",
            Self::Completed(_) => "Completed { <redacted> }",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingFinishOutcome {
    Finished,
    TimedOut,
}

/// Owner-side handle. No storage operation runs in `submit`, `poll`, or `drop`.
pub struct ProviderBindingWorker {
    scope: BindingWorkerScope,
    sender: Option<SyncSender<Command>>,
    init: Receiver<InitResult>,
    completions: Receiver<BindingCompletion>,
    stopped: Receiver<()>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    next_sequence: u64,
    inflight: Option<BindingJobToken>,
    initialized: bool,
    failed: bool,
}

impl fmt::Debug for ProviderBindingWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderBindingWorker { <redacted> }")
    }
}

impl ProviderBindingWorker {
    /// Whether storage failure, worker loss, or cancellation has latched this handle.
    pub fn is_unavailable(&self) -> bool {
        self.failed || self.cancel.load(Ordering::Acquire)
    }

    /// Start the worker; `open` and initial `load` happen on its thread.
    pub fn start(scope: BindingWorkerScope) -> io::Result<Self> {
        Self::start_with_factory(scope, |scope| {
            if let Some((managed, outcome)) = scope.managed.as_ref() {
                return ManagedProviderStore::open(&scope.path, managed.clone())
                    .map(|store| {
                        Box::new(ManagedStoreBackend {
                            store,
                            scope: managed.clone(),
                            outcome: *outcome,
                        }) as Box<dyn BindingStorageBackend>
                    })
                    .map_err(|_| ());
            }
            ProviderBindingStore::open(&scope.path, scope.target, &scope.project, &scope.version)
                .map(|store| Box::new(StoreBackend(store)) as Box<dyn BindingStorageBackend>)
                .map_err(|_| ())
        })
    }

    fn start_with_factory<F>(scope: BindingWorkerScope, factory: F) -> io::Result<Self>
    where
        F: FnOnce(&BindingWorkerScope) -> Result<Box<dyn BindingStorageBackend>, ()>
            + Send
            + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (init_sender, init) = mpsc::sync_channel(1);
        let (completion_sender, completions) = mpsc::sync_channel(1);
        let (stopped_sender, stopped) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let mut worker_scope =
            BindingWorkerScope::new(&scope.path, scope.target, &scope.project, &scope.version);
        worker_scope.managed = scope.managed.clone();
        let worker = thread::Builder::new()
            .name("panelink-binding-writer".into())
            .spawn(move || {
                worker_entry(
                    worker_scope,
                    receiver,
                    init_sender,
                    completion_sender,
                    stopped_sender,
                    thread_cancel,
                    factory,
                );
            })?;
        Ok(Self {
            scope,
            sender: Some(sender),
            init,
            completions,
            stopped,
            cancel,
            worker: Some(worker),
            next_sequence: 1,
            inflight: None,
            initialized: false,
            failed: false,
        })
    }

    /// Wait for initial open/load outside the owner hot path, capped at 250 ms.
    pub fn wait_ready(&mut self, wait: Duration) -> BindingWorkerPoll {
        if self.initialized {
            return BindingWorkerPoll::NoCompletion;
        }
        match self.init.recv_timeout(wait.min(MAX_BINDING_STARTUP_WAIT)) {
            Ok(Ok(record)) => {
                self.initialized = true;
                BindingWorkerPoll::Ready(BindingWorkerReady {
                    last_confirmed: record,
                })
            }
            Ok(Err(())) | Err(RecvTimeoutError::Disconnected) => {
                self.failed = true;
                BindingWorkerPoll::InitializationFailed
            }
            Err(RecvTimeoutError::Timeout) => BindingWorkerPoll::Pending,
        }
    }

    /// Try to admit one immutable binding without waiting or growing a queue.
    pub fn submit(&mut self, binding: &Binding) -> BindingSubmit {
        if !self.initialized || self.failed || self.cancel.load(Ordering::Acquire) {
            return BindingSubmit::Unavailable;
        }
        if binding.target() != self.scope.target
            || binding.project_identity() != self.scope.project
            || binding.supported_version() != self.scope.version
        {
            return BindingSubmit::WrongScope;
        }
        if self.inflight.is_some() {
            return BindingSubmit::Busy;
        }
        // Copy across the thread boundary through the existing bounded codec.
        // The owner keeps its pending value for reconciliation after completion.
        let owned = match provider_binding_codec::encode(binding)
            .and_then(|bytes| provider_binding_codec::decode(&bytes))
        {
            Ok(owned) => owned,
            Err(_) => return BindingSubmit::InvalidBinding,
        };
        let sequence = self.next_sequence;
        let Some(next) = sequence.checked_add(1) else {
            return BindingSubmit::SequenceExhausted;
        };
        if sequence == 0 {
            return BindingSubmit::SequenceExhausted;
        }
        let token = BindingJobToken {
            target: owned.target(),
            bridge_instance: owned.bridge_instance(),
            revision: owned.revision(),
            issuance_sequence: sequence,
        };
        let Some(sender) = self.sender.as_ref() else {
            return BindingSubmit::Unavailable;
        };
        match sender.try_send(Command::Save {
            token,
            binding: owned,
        }) {
            Ok(()) => {
                self.next_sequence = next;
                self.inflight = Some(token);
                BindingSubmit::Accepted(token)
            }
            Err(TrySendError::Full(_)) => BindingSubmit::Busy,
            Err(TrySendError::Disconnected(_)) => {
                self.failed = true;
                BindingSubmit::Unavailable
            }
        }
    }

    /// Poll for one completion. The caller must compare its token to the pending observation.
    pub fn poll(&mut self) -> BindingWorkerPoll {
        if !self.initialized {
            return match self.try_init() {
                Some(result) => result,
                None => BindingWorkerPoll::Pending,
            };
        }
        match self.completions.try_recv() {
            Ok(completion) => {
                if self.inflight != Some(completion.token) {
                    self.failed = true;
                    self.inflight = None;
                    return BindingWorkerPoll::NoCompletion;
                }
                self.inflight = None;
                if completion.result.is_err() {
                    self.failed = true;
                }
                BindingWorkerPoll::Completed(completion)
            }
            Err(TryRecvError::Empty) => BindingWorkerPoll::NoCompletion,
            Err(TryRecvError::Disconnected) => {
                self.failed = true;
                BindingWorkerPoll::NoCompletion
            }
        }
    }

    fn try_init(&mut self) -> Option<BindingWorkerPoll> {
        match self.init.try_recv() {
            Ok(Ok(record)) => {
                self.initialized = true;
                Some(BindingWorkerPoll::Ready(BindingWorkerReady {
                    last_confirmed: record,
                }))
            }
            Ok(Err(())) | Err(TryRecvError::Disconnected) => {
                self.failed = true;
                Some(BindingWorkerPoll::InitializationFailed)
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    /// Cancel queued work and wait for thread exit only until the supplied deadline.
    pub fn finish(mut self, deadline: Instant) -> BindingFinishOutcome {
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        self.worker.take();
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(MAX_BINDING_FINISH_WAIT);
        match self.stopped.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => BindingFinishOutcome::Finished,
            Err(RecvTimeoutError::Timeout) => BindingFinishOutcome::TimedOut,
        }
    }
}

impl Drop for ProviderBindingWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        self.worker.take();
    }
}

enum Command {
    Save {
        token: BindingJobToken,
        binding: Binding,
    },
}
type InitResult = Result<Option<LastConfirmedRecord>, ()>;

trait BindingStorageBackend: Send {
    fn load(&mut self) -> io::Result<Option<LastConfirmedRecord>>;
    fn save(&mut self, binding: &Binding) -> io::Result<SaveOutcome>;
}

struct StoreBackend(ProviderBindingStore);

struct ManagedStoreBackend {
    store: ManagedProviderStore,
    scope: ManagedProviderScope,
    outcome: ManagedLaunchOutcome,
}

impl BindingStorageBackend for ManagedStoreBackend {
    fn load(&mut self) -> io::Result<Option<LastConfirmedRecord>> {
        self.store.load().map(|record| {
            record.map(|record| LastConfirmedRecord::from_validated_binding(record.into_binding()))
        })
    }

    fn save(&mut self, binding: &Binding) -> io::Result<SaveOutcome> {
        let binding = provider_binding_codec::encode(binding)
            .and_then(|bytes| provider_binding_codec::decode(&bytes))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "managed binding invalid"))?;
        let record = ManagedProviderRecord::new(
            self.scope.clone(),
            ManagedLifecycleSnapshot::Running,
            self.outcome,
            binding,
        )?;
        self.store.publish(&record).map(|outcome| match outcome {
            ManagedPublishOutcome::Written => SaveOutcome::Written,
            ManagedPublishOutcome::AlreadyDurable => SaveOutcome::AlreadyDurable,
        })
    }
}

impl BindingStorageBackend for StoreBackend {
    fn load(&mut self) -> io::Result<Option<LastConfirmedRecord>> {
        self.0.load()
    }

    fn save(&mut self, binding: &Binding) -> io::Result<SaveOutcome> {
        self.0.save(binding)
    }
}

fn worker_entry<F>(
    scope: BindingWorkerScope,
    receiver: Receiver<Command>,
    init: SyncSender<InitResult>,
    completions: SyncSender<BindingCompletion>,
    stopped: SyncSender<()>,
    cancel: Arc<AtomicBool>,
    factory: F,
) where
    F: FnOnce(&BindingWorkerScope) -> Result<Box<dyn BindingStorageBackend>, ()>,
{
    let Ok(mut store) = factory(&scope) else {
        let _ = init.send(Err(()));
        let _ = stopped.try_send(());
        return;
    };
    match store.load() {
        Ok(record) => {
            let _ = init.send(Ok(record));
        }
        Err(_) => {
            let _ = init.send(Err(()));
            drop(store);
            let _ = stopped.try_send(());
            return;
        }
    }
    while let Ok(command) = receiver.recv() {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        match command {
            Command::Save { token, binding } => {
                let (result, durable_binding) = match store.save(&binding) {
                    Ok(outcome) => (Ok(outcome), Some(binding)),
                    Err(_) => (Err(BindingWorkerError::SaveFailed), None),
                };
                let completion = BindingCompletion {
                    token,
                    result: match (result, durable_binding.as_ref()) {
                        (Ok(outcome), Some(_)) => Ok(outcome),
                        _ => Err(BindingWorkerError::SaveFailed),
                    },
                    durable_binding,
                };
                if completions.send(completion).is_err() {
                    break;
                }
                // A failed save may have committed before a sync error; never retry automatically.
                // The owner sees the failure and this worker refuses further requests.
                if result.is_err() {
                    break;
                }
            }
        }
    }
    drop(store);
    let _ = stopped.try_send(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::{ProjectIdentity, SupportedVersion, VerificationSource};
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
        target: TargetRef,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = Path::new("/private/tmp").join(format!("panelink-binding-worker-{nonce}"));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("private fixture");
            Self {
                root,
                target: TargetRef {
                    generation: 7,
                    workspace: 11,
                    pane: 13,
                    session: 17,
                    occupant_revision: 19,
                },
            }
        }
        fn scope(&self) -> BindingWorkerScope {
            BindingWorkerScope::new(&self.root, self.target, b"/private/project", "2.0.32")
        }
        fn binding(&self, revision: u64) -> Binding {
            let project = ProjectIdentity::new(b"/private/project").expect("project");
            let version = SupportedVersion::new("2.0.32").expect("version");
            Binding::new(
                self.target,
                &project,
                &version,
                23,
                revision,
                "session_abc",
                29,
                VerificationSource::TuiRouteSnapshot,
            )
            .expect("binding")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn ready(worker: &mut ProviderBindingWorker) {
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            BindingWorkerPoll::Ready(_)
        ));
    }

    #[test]
    fn async_save_only_returns_worker_created_durable_completion() {
        let fixture = Fixture::new();
        let mut worker = ProviderBindingWorker::start(fixture.scope()).expect("worker");
        ready(&mut worker);
        let binding = fixture.binding(1);
        let token = match worker.submit(&binding) {
            BindingSubmit::Accepted(token) => token,
            other => panic!("unexpected submit: {other:?}"),
        };
        assert_eq!(worker.submit(&fixture.binding(2)), BindingSubmit::Busy);
        let mut completion = loop {
            if let BindingWorkerPoll::Completed(done) = worker.poll() {
                break done;
            }
            thread::yield_now();
        };
        assert_eq!(completion.token(), token);
        assert_eq!(completion.result(), Ok(SaveOutcome::Written));
        let durable = completion
            .take_durable_binding()
            .expect("successful save carries exact binding");
        assert_eq!(durable.revision(), 1);
        assert_eq!(
            worker.finish(Instant::now() + Duration::from_secs(1)),
            BindingFinishOutcome::Finished
        );
    }

    #[test]
    fn startup_failure_is_fail_closed() {
        let fixture = Fixture::new();
        fs::remove_dir(&fixture.root).expect("remove fixture");
        let mut worker = ProviderBindingWorker::start(fixture.scope())
            .expect("thread starts independently of disk");
        assert!(matches!(
            worker.wait_ready(Duration::from_secs(1)),
            BindingWorkerPoll::InitializationFailed
        ));
        assert_eq!(
            worker.submit(&fixture.binding(1)),
            BindingSubmit::Unavailable
        );
    }

    #[test]
    fn mismatched_scope_is_rejected_before_queue_admission() {
        let fixture = Fixture::new();
        let mut worker = ProviderBindingWorker::start(fixture.scope()).expect("worker");
        ready(&mut worker);
        let other_project = ProjectIdentity::new(b"/another/project").expect("project");
        let version = SupportedVersion::new("2.0.32").expect("version");
        let binding = Binding::new(
            fixture.target,
            &other_project,
            &version,
            23,
            1,
            "session_abc",
            29,
            VerificationSource::TuiRouteSnapshot,
        )
        .expect("binding");
        assert_eq!(worker.submit(&binding), BindingSubmit::WrongScope);
        assert_eq!(
            worker.finish(Instant::now() + Duration::from_secs(1)),
            BindingFinishOutcome::Finished
        );
    }

    #[test]
    fn save_error_cannot_become_a_successful_completion() {
        let fixture = Fixture::new();
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release_sender, release) = mpsc::channel();
        let scope = fixture.scope();
        let mut worker = ProviderBindingWorker::start_with_factory(scope, move |scope| {
            let store = ProviderBindingStore::open(
                &scope.path,
                scope.target,
                &scope.project,
                &scope.version,
            )
            .map_err(|_| ())?;
            Ok(Box::new(BlockingBackend {
                store,
                entered: entered_sender,
                release,
            }) as Box<dyn BindingStorageBackend>)
        })
        .expect("worker");
        ready(&mut worker);
        let binding = fixture.binding(1);
        let token = match worker.submit(&binding) {
            BindingSubmit::Accepted(token) => token,
            other => panic!("unexpected submit: {other:?}"),
        };
        entered
            .recv_timeout(Duration::from_secs(1))
            .expect("save reached backend");
        let displaced = fixture.root.with_extension("displaced");
        fs::rename(&fixture.root, &displaced).expect("replace checked directory name");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.root)
            .expect("replacement directory");
        release_sender.send(()).expect("release save");
        let completion = loop {
            if let BindingWorkerPoll::Completed(done) = worker.poll() {
                break done;
            }
            thread::yield_now();
        };
        assert_eq!(completion.token(), token);
        assert_eq!(completion.result(), Err(BindingWorkerError::SaveFailed));
        assert!(completion.durable_binding.is_none());
        let _ = fs::remove_dir_all(displaced);
        let _ = worker.finish(Instant::now() + Duration::from_secs(1));
    }

    struct BlockingBackend {
        store: ProviderBindingStore,
        entered: mpsc::SyncSender<()>,
        release: Receiver<()>,
    }

    impl BindingStorageBackend for BlockingBackend {
        fn load(&mut self) -> io::Result<Option<LastConfirmedRecord>> {
            self.store.load()
        }
        fn save(&mut self, binding: &Binding) -> io::Result<SaveOutcome> {
            let _ = self.entered.send(());
            let _ = self.release.recv();
            self.store.save(binding)
        }
    }

    #[test]
    fn finish_timeout_keeps_store_lock_until_running_io_exits() {
        let fixture = Fixture::new();
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release_sender, release) = mpsc::channel();
        let scope = fixture.scope();
        let mut worker = ProviderBindingWorker::start_with_factory(scope, move |scope| {
            let store = ProviderBindingStore::open(
                &scope.path,
                scope.target,
                &scope.project,
                &scope.version,
            )
            .map_err(|_| ())?;
            Ok(Box::new(BlockingBackend {
                store,
                entered: entered_sender,
                release,
            }) as Box<dyn BindingStorageBackend>)
        })
        .expect("worker");
        ready(&mut worker);
        assert!(matches!(
            worker.submit(&fixture.binding(1)),
            BindingSubmit::Accepted(_)
        ));
        entered
            .recv_timeout(Duration::from_secs(1))
            .expect("save entered backend");
        assert!(matches!(worker.poll(), BindingWorkerPoll::NoCompletion));
        assert_eq!(worker.submit(&fixture.binding(2)), BindingSubmit::Busy);
        assert_eq!(
            worker.finish(Instant::now() + Duration::from_millis(10)),
            BindingFinishOutcome::TimedOut
        );
        assert_eq!(
            ProviderBindingStore::open(
                &fixture.root,
                fixture.target,
                b"/private/project",
                "2.0.32"
            )
            .err()
            .map(|error| error.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        release_sender.send(()).expect("release running save");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match ProviderBindingStore::open(
                &fixture.root,
                fixture.target,
                b"/private/project",
                "2.0.32",
            ) {
                Ok(store) => {
                    drop(store);
                    break;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::yield_now()
                }
                Err(error) => panic!("worker did not release lock: {error}"),
            }
        }
    }

    #[test]
    fn dropped_worker_does_not_wait_for_storage_thread() {
        let fixture = Fixture::new();
        let mut worker = ProviderBindingWorker::start(fixture.scope()).expect("worker");
        ready(&mut worker);
        let binding = fixture.binding(1);
        let _ = worker.submit(&binding);
        drop(worker);
        // Drop is owner-side cancellation only; it never joins the storage thread.
    }
}
