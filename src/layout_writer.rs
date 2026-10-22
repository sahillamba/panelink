//! Bounded saved-layout async writer with coalescing, retry and timed shutdown.
//!
//! One worker thread serializes storage calls. The owner may observe changed
//! layouts without waiting for disk; intermediate snapshots are coalesced.
//! A timed-out shutdown detaches but the worker retains its singleton guard.

use crate::layout_store::{LayoutStore, SaveOutcome};
use crate::saved_layout::SavedLayout;
use crate::server_storage::{LayoutWriterGuard, ServerStorage};
use std::fmt;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Maximum time an explicit shutdown waits for the worker.
pub const MAX_SHUTDOWN_WAIT: Duration = Duration::from_millis(250);

const RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(10);

fn invalid_layout() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid layout candidate")
}

/// Result of a bounded explicit writer shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutShutdownOutcome {
    /// Every admitted candidate reached its terminal status before the deadline.
    Drained,
    /// The wait budget expired; cancellation was requested between calls.
    TimedOut,
    /// At least one candidate failed or the worker panicked.
    Failed,
}

/// Why an observed layout was not durably submitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutSubmit {
    /// Accepted into the worker queue.
    Accepted,
    /// Coalesced into the owner-deferred slot; a previous candidate is queued.
    Deferred,
    /// The writer is stopped, failed, or the candidate scope mismatch was refused.
    Unavailable,
}

/// Fate of the deferred candidate at shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutFinalStatus {
    /// No deferred candidate existed at shutdown.
    NothingPending,
    /// The shutdown try-submit succeeded.
    Submitted,
    /// The shutdown try-submit failed or was not attempted.
    Unsubmitted,
}

/// Immutable identity of one observed layout snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutKey {
    /// Server generation bound to this writer.
    pub generation: u64,
    /// Snapshot sequence ordering within the generation.
    pub snapshot_sequence: u64,
}

impl fmt::Debug for LayoutKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LayoutKey { <redacted> }")
    }
}

/// Read-only snapshot of writer progress.
#[derive(Clone, Copy)]
pub struct LayoutStatus {
    /// Last candidate observed by the owner.
    pub last_observed: Option<LayoutKey>,
    /// Last candidate submitted to the worker queue.
    pub last_submitted: Option<LayoutKey>,
    /// Last candidate acknowledged durable by the store.
    pub last_durable: Option<LayoutKey>,
    /// Sticky terminal failure or scope refusal.
    pub unavailable: bool,
    /// Final deferred candidate fate after shutdown.
    pub final_status: LayoutFinalStatus,
}

impl fmt::Debug for LayoutStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutStatus")
            .field("unavailable", &self.unavailable)
            .field("final_status", &self.final_status)
            .finish_non_exhaustive()
    }
}

/// A cloneable handle that can read status after the writer or driver is dropped.
#[derive(Clone)]
pub struct LayoutStatusHandle {
    inner: Arc<Mutex<StatusInner>>,
}

impl LayoutStatusHandle {
    /// Read the current status without waiting for storage work.
    pub fn status(&self) -> LayoutStatus {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        LayoutStatus {
            last_observed: inner.last_observed,
            last_submitted: inner.last_submitted,
            last_durable: inner.last_durable,
            unavailable: inner.unavailable,
            final_status: inner.final_status,
        }
    }
}

#[derive(Clone, Copy)]
struct StatusInner {
    last_observed: Option<LayoutKey>,
    last_submitted: Option<LayoutKey>,
    last_durable: Option<LayoutKey>,
    unavailable: bool,
    final_status: LayoutFinalStatus,
}

/// One-producer, one-worker bounded saved-layout writer.
pub struct LayoutWriter {
    sender: Option<SyncSender<SavedLayout>>,
    done: Receiver<LayoutShutdownOutcome>,
    worker: Option<JoinHandle<()>>,
    status: Arc<Mutex<StatusInner>>,
    deferred: Option<SavedLayout>,
    cancel: Arc<AtomicBool>,
    bound_generation: u64,
    bound_workspace: u64,
    finalized: bool,
}

impl fmt::Debug for LayoutWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LayoutWriter { <redacted> }")
    }
}

impl LayoutWriter {
    /// Start one worker after verifying the store and claiming its server.
    ///
    /// `workspace` must be nonzero. The caller must stop any other direct
    /// mutation of that store while the writer lives.
    pub fn start(store: LayoutStore, storage: &ServerStorage, workspace: u64) -> io::Result<Self> {
        store.verify_server_directory(storage.directory_path())?;
        let writer_guard = storage.claim_layout_writer()?;
        let generation = storage.generation();
        if workspace == 0 {
            return Err(invalid_layout());
        }
        Self::spawn(
            Box::new(StoreBackend { store }),
            writer_guard,
            generation,
            workspace,
        )
    }

    /// Observe one candidate without waiting for disk.
    ///
    /// A scope mismatch sets the sticky `unavailable` flag and refuses.
    /// A full queue coalesces the candidate into the owner-deferred slot.
    pub fn observe(&mut self, layout: SavedLayout) -> LayoutSubmit {
        let meta = layout.metadata();
        let key = LayoutKey {
            generation: meta.generation(),
            snapshot_sequence: meta.snapshot_sequence(),
        };

        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        status.last_observed = Some(key);

        if meta.generation() != self.bound_generation || meta.workspace() != self.bound_workspace {
            status.unavailable = true;
            return LayoutSubmit::Unavailable;
        }
        if status.unavailable {
            return LayoutSubmit::Unavailable;
        }
        drop(status);

        // Always route through the single deferred slot: a newer observation
        // replaces any older unsubmitted one, so submission order stays
        // strictly increasing and the store never sees a stale sequence.
        self.deferred = Some(layout);
        self.flush()
    }

    /// Retry the owner-deferred candidate once.
    ///
    /// Returns `Accepted` when there is nothing pending to flush.
    pub fn flush(&mut self) -> LayoutSubmit {
        let status = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if status.unavailable {
            return LayoutSubmit::Unavailable;
        }
        drop(status);

        let Some(layout) = self.deferred.take() else {
            return LayoutSubmit::Accepted;
        };

        let key = LayoutKey {
            generation: layout.metadata().generation(),
            snapshot_sequence: layout.metadata().snapshot_sequence(),
        };

        let Some(sender) = self.sender.as_ref() else {
            let mut status = self
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            status.unavailable = true;
            return LayoutSubmit::Unavailable;
        };

        match sender.try_send(layout) {
            Ok(()) => {
                let mut status = self
                    .status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                status.last_submitted = Some(key);
                LayoutSubmit::Accepted
            }
            Err(TrySendError::Full(l)) => {
                self.deferred = Some(l);
                LayoutSubmit::Deferred
            }
            Err(TrySendError::Disconnected(_)) => {
                let mut status = self
                    .status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                status.unavailable = true;
                LayoutSubmit::Unavailable
            }
        }
    }

    /// Read current progress without waiting.
    pub fn status(&self) -> LayoutStatus {
        let inner = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        LayoutStatus {
            last_observed: inner.last_observed,
            last_submitted: inner.last_submitted,
            last_durable: inner.last_durable,
            unavailable: inner.unavailable,
            final_status: inner.final_status,
        }
    }

    /// Return a handle that may be retained and observed after the writer is
    /// dropped.
    pub fn status_handle(&self) -> LayoutStatusHandle {
        LayoutStatusHandle {
            inner: Arc::clone(&self.status),
        }
    }

    /// Stop admission and drain for at most `min(wait, 250 ms)`.
    ///
    /// A timeout detaches from the worker but does not release the singleton
    /// guard until the thread exits.
    pub fn shutdown(self, wait: Duration) -> LayoutShutdownOutcome {
        let budget = wait.min(MAX_SHUTDOWN_WAIT);
        let deadline = Instant::now() + budget;
        self.begin_shutdown().wait_until(deadline)
    }

    /// Shutdown using a shared deadline so the caller can pool wait budgets.
    pub fn shutdown_with_deadline(self, deadline: Instant) -> LayoutShutdownOutcome {
        self.begin_shutdown().wait_until(deadline)
    }

    /// Make the one final nonblocking try-submit and close admission now,
    /// without waiting. The caller later waits on the returned handle, so a
    /// driver can close every metadata stream before one shared wait.
    pub fn begin_shutdown(mut self) -> PendingLayoutShutdown {
        let final_status = match self.deferred.take() {
            None => LayoutFinalStatus::NothingPending,
            Some(layout) => {
                let key = key_of(&layout);
                let sent = match self.sender.as_ref() {
                    Some(sender) => match sender.try_send(layout) {
                        Ok(()) => Some(true),
                        Err(TrySendError::Full(_)) => Some(false),
                        Err(TrySendError::Disconnected(_)) => None,
                    },
                    None => None,
                };
                match sent {
                    Some(true) => {
                        self.lock_status().last_submitted = Some(key);
                        LayoutFinalStatus::Submitted
                    }
                    Some(false) => LayoutFinalStatus::Unsubmitted,
                    None => {
                        self.lock_status().unavailable = true;
                        LayoutFinalStatus::Unsubmitted
                    }
                }
            }
        };
        self.lock_status().final_status = final_status;
        self.sender.take();
        self.finalized = true;
        // Dropping the join handle detaches; we never join a blocked worker.
        self.worker.take();
        let (_, done) = mpsc::sync_channel(0);
        PendingLayoutShutdown {
            done: std::mem::replace(&mut self.done, done),
            cancel: Arc::clone(&self.cancel),
        }
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, StatusInner> {
        self.status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn start_with_backend(
        backend: Box<dyn LayoutBackend>,
        storage: &ServerStorage,
        workspace: u64,
    ) -> io::Result<Self> {
        let writer_guard = storage.claim_layout_writer()?;
        let generation = storage.generation();
        if workspace == 0 {
            return Err(invalid_layout());
        }
        Self::spawn(backend, writer_guard, generation, workspace)
    }

    fn spawn(
        backend: Box<dyn LayoutBackend>,
        writer_guard: LayoutWriterGuard,
        generation: u64,
        workspace: u64,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (done_sender, done) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(StatusInner {
            last_observed: None,
            last_submitted: None,
            last_durable: None,
            unavailable: false,
            final_status: LayoutFinalStatus::NothingPending,
        }));
        let worker_status = Arc::clone(&status);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::Builder::new()
            .name("panelink-layout-writer".to_owned())
            .spawn(move || {
                worker_entry(
                    backend,
                    writer_guard,
                    receiver,
                    (worker_status, worker_cancel),
                    done_sender,
                    generation,
                    workspace,
                );
            })
            .map_err(|_| io::Error::other("unable to start layout writer"))?;

        Ok(Self {
            sender: Some(sender),
            done,
            worker: Some(worker),
            status,
            deferred: None,
            cancel,
            bound_generation: generation,
            bound_workspace: workspace,
            finalized: false,
        })
    }
}

/// Admission is closed; waiting is deferred to a caller-owned deadline.
/// Dropping this handle cancels queued saves and retries without waiting.
pub struct PendingLayoutShutdown {
    done: Receiver<LayoutShutdownOutcome>,
    cancel: Arc<AtomicBool>,
}

impl fmt::Debug for PendingLayoutShutdown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingLayoutShutdown { .. }")
    }
}

impl Drop for PendingLayoutShutdown {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl PendingLayoutShutdown {
    /// Wait until `deadline`, capped at 250 ms from now. On timeout no new
    /// save stage starts; a running store call may still finish later, but
    /// the result stays `TimedOut`.
    pub fn wait_until(self, deadline: Instant) -> LayoutShutdownOutcome {
        let budget = deadline
            .saturating_duration_since(Instant::now())
            .min(MAX_SHUTDOWN_WAIT);
        match self.done.recv_timeout(budget) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.cancel.store(true, Ordering::Release);
                LayoutShutdownOutcome::TimedOut
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => LayoutShutdownOutcome::Failed,
        }
    }
}

fn key_of(layout: &SavedLayout) -> LayoutKey {
    LayoutKey {
        generation: layout.metadata().generation(),
        snapshot_sequence: layout.metadata().snapshot_sequence(),
    }
}

impl Drop for LayoutWriter {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        self.worker.take(); // Plain Drop never joins or waits for disk work.
    }
}

pub(crate) trait LayoutBackend: Send + 'static {
    fn save(
        &mut self,
        bound_generation: u64,
        bound_workspace: u64,
        candidate: &SavedLayout,
    ) -> io::Result<SaveOutcome>;
}

struct StoreBackend {
    store: LayoutStore,
}

impl LayoutBackend for StoreBackend {
    fn save(
        &mut self,
        bound_generation: u64,
        bound_workspace: u64,
        candidate: &SavedLayout,
    ) -> io::Result<SaveOutcome> {
        self.store
            .save(bound_generation, bound_workspace, candidate)
    }
}

fn worker_entry(
    mut backend: Box<dyn LayoutBackend>,
    writer_guard: LayoutWriterGuard,
    receiver: Receiver<SavedLayout>,
    (shared, cancel): (Arc<Mutex<StatusInner>>, Arc<AtomicBool>),
    done: SyncSender<LayoutShutdownOutcome>,
    bound_generation: u64,
    bound_workspace: u64,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        worker_loop(
            &mut *backend,
            &receiver,
            &shared,
            &cancel,
            bound_generation,
            bound_workspace,
        )
    }));
    let mut failed = match result {
        Ok(worker_failed) => worker_failed,
        Err(_) => {
            let mut status = shared.lock().unwrap_or_else(|error| error.into_inner());
            status.unavailable = true;
            true
        }
    };
    if catch_unwind(AssertUnwindSafe(|| drop(backend))).is_err() {
        let mut status = shared.lock().unwrap_or_else(|error| error.into_inner());
        status.unavailable = true;
        failed = true;
    }
    drop(writer_guard);
    let outcome = if failed {
        LayoutShutdownOutcome::Failed
    } else {
        LayoutShutdownOutcome::Drained
    };
    let _ = done.send(outcome);
}

fn worker_loop(
    backend: &mut dyn LayoutBackend,
    receiver: &Receiver<SavedLayout>,
    shared: &Arc<Mutex<StatusInner>>,
    cancel: &AtomicBool,
    bound_generation: u64,
    bound_workspace: u64,
) -> bool {
    let mut failed = false;
    loop {
        if failed {
            // Drain remaining queue without processing.
            match receiver.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return true,
            }
        }
        match receiver.recv() {
            Ok(layout) => {
                if failed {
                    continue;
                }
                let mut saved = false;
                for attempt in 0..RETRIES {
                    if failed {
                        break;
                    }
                    if cancel.load(Ordering::Acquire) {
                        // Canceled work is neither saved nor a failure.
                        return failed;
                    }
                    match backend.save(bound_generation, bound_workspace, &layout) {
                        Ok(_) => {
                            let mut status =
                                shared.lock().unwrap_or_else(|error| error.into_inner());
                            status.last_durable = Some(LayoutKey {
                                generation: layout.metadata().generation(),
                                snapshot_sequence: layout.metadata().snapshot_sequence(),
                            });
                            saved = true;
                            break;
                        }
                        Err(_) if attempt + 1 < RETRIES => {
                            thread::sleep(RETRY_DELAY);
                        }
                        Err(_) => {
                            failed = true;
                            let mut status =
                                shared.lock().unwrap_or_else(|error| error.into_inner());
                            status.unavailable = true;
                        }
                    }
                }
                if !saved && !failed {
                    // Should not happen, but treat as failure to be safe.
                    failed = true;
                }
            }
            Err(_) => return failed,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::TargetRef;
    use crate::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
    use crate::saved_layout::LayoutMetadata;
    use crate::server_storage::ServerStorage;
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = PathBuf::from(format!(
                "/private/tmp/panelink-layout-writer-{}-{suffix}",
                std::process::id()
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("create private writer fixture");
            Self { root }
        }

        fn prepare(&self) -> (ServerStorage, LayoutStore) {
            let storage =
                ServerStorage::initialize_for_test(&self.root).expect("initialize storage");
            let store = LayoutStore::prepare(&self.root).expect("prepare layout store");
            (storage, store)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn target_for(generation: u64, workspace: u64, pane: u64, session: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace,
            pane,
            session,
            occupant_revision: 1,
        }
    }

    fn make_meta(generation: u64, workspace: u64, sequence: u64, revision: u64) -> LayoutMetadata {
        LayoutMetadata::new(generation, workspace, sequence, revision, 1_000_001)
            .expect("valid test metadata")
    }

    fn saved_layout_one(
        generation: u64,
        workspace: u64,
        sequence: u64,
        revision: u64,
    ) -> SavedLayout {
        let meta = make_meta(generation, workspace, sequence, revision);
        let snapshot = LayoutSnapshot {
            revision,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: target_for(generation, workspace, 2, 3),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 10,
                        columns: 30,
                    },
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; crate::pane_layout::MAX_PANES - 1],
        };
        SavedLayout::from_snapshot(meta, &snapshot).expect("valid one-pane saved layout")
    }

    fn wait_for_durable(handle: &LayoutStatusHandle, key: LayoutKey, deadline: Instant) {
        loop {
            let status = handle.status();
            if status.last_durable == Some(key) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "candidate did not become durable"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_lock(path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match ServerStorage::lock_for_maintenance(path) {
                Ok(lock) => {
                    drop(lock);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("maintenance lock failed: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "retained server lock did not release"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Save,
    }

    enum Behavior {
        Fail,
        Block(Arc<Gate>),
        Panic,
    }

    struct Gate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                state: Mutex::new((false, false)),
                changed: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }

        fn wait_until_entered(&self) {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            while !state.0 {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "backend did not reach blocking stage");
                let (next, timeout) = self
                    .changed
                    .wait_timeout(state, left)
                    .unwrap_or_else(|error| error.into_inner());
                state = next;
                assert!(!timeout.timed_out() || state.0, "backend stage timed out");
            }
        }

        fn release(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.1 = true;
            self.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct FakeState {
        calls: Vec<Call>,
        saved_keys: Vec<LayoutKey>,
    }

    #[derive(Clone, Default)]
    struct FakeControl {
        state: Arc<Mutex<FakeState>>,
        behaviors: Arc<Mutex<VecDeque<(Call, Behavior)>>>,
    }

    impl FakeControl {
        fn schedule(&self, call: Call, behavior: Behavior) {
            self.behaviors
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_back((call, behavior));
        }

        fn calls(&self) -> Vec<Call> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .calls
                .clone()
        }

        fn saved_keys(&self) -> Vec<LayoutKey> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .saved_keys
                .clone()
        }

        fn before(&self, call: Call) -> io::Result<()> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .calls
                .push(call);
            let behavior = {
                let mut queue = self
                    .behaviors
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if queue
                    .front()
                    .is_some_and(|(scheduled, _)| *scheduled == call)
                {
                    queue.pop_front().map(|(_, behavior)| behavior)
                } else {
                    None
                }
            };
            match behavior {
                Some(Behavior::Fail) => Err(io::Error::other("injected storage failure")),
                Some(Behavior::Block(gate)) => {
                    gate.wait();
                    Ok(())
                }
                Some(Behavior::Panic) => std::panic::resume_unwind(Box::new("test panic")),
                None => Ok(()),
            }
        }

        fn record_save(&self, key: LayoutKey) {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .saved_keys
                .push(key);
        }
    }

    struct FakeBackend(FakeControl);

    impl LayoutBackend for FakeBackend {
        fn save(
            &mut self,
            _bound_generation: u64,
            _bound_workspace: u64,
            candidate: &SavedLayout,
        ) -> io::Result<SaveOutcome> {
            self.0.before(Call::Save)?;
            let key = LayoutKey {
                generation: candidate.metadata().generation(),
                snapshot_sequence: candidate.metadata().snapshot_sequence(),
            };
            self.0.record_save(key);
            Ok(SaveOutcome::Written)
        }
    }

    fn fake_writer(fixture: &Fixture, backend: FakeControl) -> (ServerStorage, LayoutWriter) {
        let (storage, _store) = fixture.prepare();
        let writer = LayoutWriter::start_with_backend(Box::new(FakeBackend(backend)), &storage, 1)
            .expect("start fake writer");
        (storage, writer)
    }

    // -- unit tests ----------------------------------------------------------

    #[test]
    fn accept_and_durable() {
        let fixture = Fixture::new();
        let (_storage, mut writer) = fake_writer(&fixture, FakeControl::default());
        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(layout), LayoutSubmit::Accepted);
        let handle = writer.status_handle();
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &handle,
            LayoutKey {
                generation: 1,
                snapshot_sequence: 1,
            },
            deadline,
        );
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn coalescing_to_latest_with_blocked_worker() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer) = fake_writer(&fixture, control.clone());

        let first = saved_layout_one(1, 1, 1, 1);
        let second = saved_layout_one(1, 1, 2, 2);
        let third = saved_layout_one(1, 1, 3, 3);
        let fourth = saved_layout_one(1, 1, 4, 4);

        assert_eq!(writer.observe(first), LayoutSubmit::Accepted);
        gate.wait_until_entered();
        // sync_channel(1): first is in-flight, second fills the queue.
        assert_eq!(writer.observe(second), LayoutSubmit::Accepted);
        // Third is deferred and later coalesced by fourth.
        assert_eq!(writer.observe(third), LayoutSubmit::Deferred);
        assert_eq!(writer.observe(fourth), LayoutSubmit::Deferred);
        gate.release();

        // After unblocking: first and second are saved. Fourth is still deferred.
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 2,
            },
            deadline,
        );

        // Flush the deferred fourth.
        assert_eq!(writer.flush(), LayoutSubmit::Accepted);
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 4,
            },
            deadline,
        );

        // Only first, second and fourth should have been saved (third was coalesced away).
        let keys = control.saved_keys();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].snapshot_sequence, 1);
        assert_eq!(keys[1].snapshot_sequence, 2);
        assert_eq!(keys[2].snapshot_sequence, 4);

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn queue_never_exceeds_three_candidates() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer) = fake_writer(&fixture, control);

        let first = saved_layout_one(1, 1, 1, 1);
        let second = saved_layout_one(1, 1, 2, 2);
        let third = saved_layout_one(1, 1, 3, 3);
        let fourth = saved_layout_one(1, 1, 4, 4);

        // in-flight fills channel capacity, queued fills the one slot.
        assert_eq!(writer.observe(first), LayoutSubmit::Accepted);
        gate.wait_until_entered();
        assert_eq!(writer.observe(second), LayoutSubmit::Accepted);
        // Queue is now full (in-flight + queued). Further observes are deferred.
        assert_eq!(writer.observe(third), LayoutSubmit::Deferred);
        assert_eq!(writer.observe(fourth), LayoutSubmit::Deferred);
        gate.release();

        // After worker drains: first and second are saved, fourth remains deferred.
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 2,
            },
            deadline,
        );

        // Flush the deferred fourth.
        assert_eq!(writer.flush(), LayoutSubmit::Accepted);
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 4,
            },
            deadline,
        );

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn retry_identical_bytes_then_success() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        control.schedule(Call::Save, Behavior::Fail);
        control.schedule(Call::Save, Behavior::Fail);
        let (_storage, mut writer) = fake_writer(&fixture, control.clone());

        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(layout), LayoutSubmit::Accepted);

        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 1,
            },
            deadline,
        );

        // Should have retried the identical layout 3 times (2 failures + 1 success).
        assert_eq!(control.calls().len(), 3);
        assert!(control.calls().iter().all(|c| *c == Call::Save));

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn terminal_failure_is_sticky_and_later_observe_unavailable() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        for _ in 0..RETRIES {
            control.schedule(Call::Save, Behavior::Fail);
        }
        let (_storage, mut writer) = fake_writer(&fixture, control.clone());

        let first = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(first), LayoutSubmit::Accepted);

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = writer.status();
            if status.unavailable {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "writer did not become unavailable"
            );
            thread::sleep(Duration::from_millis(1));
        }

        let second = saved_layout_one(1, 1, 2, 2);
        assert_eq!(writer.observe(second), LayoutSubmit::Unavailable);

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Failed
        );
    }

    #[test]
    fn panic_fails_and_sets_unavailable() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        control.schedule(Call::Save, Behavior::Panic);
        let (_storage, mut writer) = fake_writer(&fixture, control);

        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(layout), LayoutSubmit::Accepted);

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = writer.status();
            if status.unavailable {
                break;
            }
            assert!(Instant::now() < deadline, "panic did not set unavailable");
            thread::sleep(Duration::from_millis(1));
        }

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Failed
        );
    }

    #[test]
    fn scope_mismatch_refused_and_sticky() {
        let fixture = Fixture::new();
        let (_storage, mut writer) = fake_writer(&fixture, FakeControl::default());

        let wrong_gen = saved_layout_one(2, 1, 1, 1);
        assert_eq!(writer.observe(wrong_gen), LayoutSubmit::Unavailable);
        let status = writer.status();
        assert!(status.unavailable);

        let wrong_ws = saved_layout_one(1, 2, 2, 2);
        assert_eq!(writer.observe(wrong_ws), LayoutSubmit::Unavailable);

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn shutdown_drained_with_no_pending() {
        let fixture = Fixture::new();
        let (_storage, writer) = fake_writer(&fixture, FakeControl::default());
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }

    #[test]
    fn shutdown_timed_out_with_blocked_backend() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer) = fake_writer(&fixture, control);

        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(layout), LayoutSubmit::Accepted);
        gate.wait_until_entered();

        let started = Instant::now();
        let outcome = writer.shutdown(Duration::from_secs(5));
        assert_eq!(outcome, LayoutShutdownOutcome::TimedOut);
        assert!(
            started.elapsed() <= MAX_SHUTDOWN_WAIT + Duration::from_millis(100),
            "shutdown exceeded capped budget"
        );

        // The worker still retains the singleton until unblocked.
        drop(storage);
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        gate.release();
        wait_for_lock(&fixture.root);
    }

    #[test]
    fn timed_out_shutdown_starts_no_new_save_stage() {
        // Parent review regression: after a timeout, queued work is canceled
        // rather than written by a detached worker.
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer) = fake_writer(&fixture, control.clone());
        assert_eq!(
            writer.observe(saved_layout_one(1, 1, 1, 1)),
            LayoutSubmit::Accepted
        );
        gate.wait_until_entered();
        assert_eq!(
            writer.observe(saved_layout_one(1, 1, 2, 2)),
            LayoutSubmit::Accepted
        );
        let handle = writer.status_handle();
        assert_eq!(
            writer.shutdown(Duration::from_millis(20)),
            LayoutShutdownOutcome::TimedOut
        );
        drop(storage);
        gate.release();
        wait_for_lock(&fixture.root);
        // The blocked first save completes; the queued second never starts.
        assert_eq!(control.calls().len(), 1);
        assert_eq!(
            handle
                .status()
                .last_durable
                .map(|key| key.snapshot_sequence),
            Some(1)
        );
    }

    #[test]
    fn abandoned_shutdown_starts_no_new_save_stage() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer) = fake_writer(&fixture, control.clone());
        assert_eq!(
            writer.observe(saved_layout_one(1, 1, 1, 1)),
            LayoutSubmit::Accepted
        );
        gate.wait_until_entered();
        assert_eq!(
            writer.observe(saved_layout_one(1, 1, 2, 2)),
            LayoutSubmit::Accepted
        );
        let handle = writer.status_handle();
        let pending = writer.begin_shutdown();
        drop(pending);
        drop(storage);
        // Abandoning shutdown never releases exclusion while a save is active.
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        gate.release();
        wait_for_lock(&fixture.root);
        assert_eq!(control.calls().len(), 1);
        assert_eq!(
            handle
                .status()
                .last_durable
                .map(|key| key.snapshot_sequence),
            Some(1)
        );
    }

    #[test]
    fn shutdown_final_deferred_submitted_vs_unsubmitted() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer) = fake_writer(&fixture, control);

        let first = saved_layout_one(1, 1, 1, 1);
        let second = saved_layout_one(1, 1, 2, 2);
        assert_eq!(writer.observe(first), LayoutSubmit::Accepted);
        gate.wait_until_entered();
        // Queue slot is available; second goes to queue (not deferred).
        assert_eq!(writer.observe(second), LayoutSubmit::Accepted);

        // Shutdown with the worker blocked should time out.
        let handle = writer.status_handle();
        let outcome = writer.shutdown(Duration::from_millis(10));
        assert_eq!(outcome, LayoutShutdownOutcome::TimedOut);

        // No deferred candidate existed, so final_status reflects nothing pending.
        let status = handle.status();
        assert_eq!(status.final_status, LayoutFinalStatus::NothingPending);
        assert_eq!(
            status.last_observed,
            Some(LayoutKey {
                generation: 1,
                snapshot_sequence: 2,
            })
        );

        gate.release();
    }

    #[test]
    fn status_handle_readable_after_writer_dropped() {
        let fixture = Fixture::new();
        let (_storage, mut writer) = fake_writer(&fixture, FakeControl::default());
        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(writer.observe(layout), LayoutSubmit::Accepted);
        let handle = writer.status_handle();
        let outcome = writer.shutdown(Duration::from_millis(250));
        assert_eq!(outcome, LayoutShutdownOutcome::Drained);
        // Writer is consumed by shutdown; handle must still be readable.
        let status = handle.status();
        assert_eq!(
            status.last_durable,
            Some(LayoutKey {
                generation: 1,
                snapshot_sequence: 1,
            })
        );
    }

    #[test]
    fn second_start_on_same_storage_would_block() {
        let fixture = Fixture::new();
        let (storage, store) = fixture.prepare();
        let store2 = LayoutStore::prepare(&fixture.root).expect("second open");
        let first = LayoutWriter::start(store, &storage, 1).expect("first start");
        assert_eq!(
            LayoutWriter::start(store2, &storage, 1).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        first.shutdown(Duration::from_millis(250));
    }

    #[test]
    fn history_claim_coexists_with_layout_claim() {
        let fixture = Fixture::new();
        let (storage, store) = fixture.prepare();
        let history_guard = storage.claim_history_writer().expect("claim history");
        let mut layout_writer = LayoutWriter::start(store, &storage, 1)
            .expect("layout writer starts concurrently with history claim");
        let layout = saved_layout_one(1, 1, 1, 1);
        assert_eq!(layout_writer.observe(layout), LayoutSubmit::Accepted);
        assert_eq!(
            layout_writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
        drop(history_guard);
        // Lock release timing is nondeterministic because shutdown drops the
        // JoinHandle without joining; the worker thread drops the guard
        // asynchronously. This is covered by timeout/retention tests.
        thread::sleep(Duration::from_millis(200));
    }

    #[test]
    fn flush_retries_deferred_once() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Save, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer) = fake_writer(&fixture, control);

        let first = saved_layout_one(1, 1, 1, 1);
        let second = saved_layout_one(1, 1, 2, 2);
        let third = saved_layout_one(1, 1, 3, 3);
        assert_eq!(writer.observe(first), LayoutSubmit::Accepted);
        gate.wait_until_entered();
        // Queue slot is available; second fills it.
        assert_eq!(writer.observe(second), LayoutSubmit::Accepted);
        // Now both in-flight and queue are occupied; third becomes deferred.
        assert_eq!(writer.observe(third), LayoutSubmit::Deferred);

        // flush while still blocked should re-defer.
        assert_eq!(writer.flush(), LayoutSubmit::Deferred);

        gate.release();
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 2,
            },
            deadline,
        );

        // Now flush should succeed for the deferred third.
        assert_eq!(writer.flush(), LayoutSubmit::Accepted);
        let deadline = Instant::now() + Duration::from_secs(3);
        wait_for_durable(
            &writer.status_handle(),
            LayoutKey {
                generation: 1,
                snapshot_sequence: 3,
            },
            deadline,
        );

        assert_eq!(
            writer.shutdown(Duration::from_millis(250)),
            LayoutShutdownOutcome::Drained
        );
    }
}
