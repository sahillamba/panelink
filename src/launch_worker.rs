//! Serialized launch preparation, journal IO and PTY creation.
//!
//! This worker deliberately owns no runtime ledger, layout or event effects.
//! Its single command slot and completion slot keep one launch transaction in
//! flight, while the duplicated storage claim keeps journal work off the owner.

use crate::control_protocol::TargetRef;
use crate::managed_launch_prepare::prepare_launch;
use crate::protocol::{OperationKey, Outcome};
use crate::server_storage::{LaunchRecord, LaunchState, LaunchWriterGuard, ServerStorage};
use crate::split_protocol::decode_split_request;
use crate::terminal::TerminalSession;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

enum Command {
    Start {
        payload: Vec<u8>,
        target: TargetRef,
    },
    Finalize {
        key: OperationKey,
        target: TargetRef,
    },
}

/// One worker completion. A spawned session is transferred exactly once to
/// the server owner, which must attach it before requesting finalization.
pub(crate) enum LaunchWorkResult {
    Spawned(Box<TerminalSession>),
    Finished { outcome: Outcome, unhealthy: bool },
}

type Completion = Result<LaunchWorkResult, ()>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchStage {
    #[cfg(test)]
    BeforePrepare,
    BeforeIntent,
    AfterIntent,
    BeforeSpawn,
    AfterSpawn,
    SpawnPublished,
    BeforeFinalize,
}

#[cfg(test)]
#[derive(Default)]
struct TestControl {
    hook: Option<Arc<dyn Fn(LaunchStage) + Send + Sync>>,
    fail_at: Option<LaunchStage>,
}

/// Driver for the one serialized launch worker.
pub(crate) struct LaunchWorker {
    sender: SyncSender<Command>,
    completion: Arc<Mutex<Option<Completion>>>,
    cancelled: Arc<AtomicBool>,
    busy: AtomicBool,
    unavailable: AtomicBool,
    #[cfg(test)]
    test_control: Arc<Mutex<TestControl>>,
    thread: Option<JoinHandle<()>>,
}

impl LaunchWorker {
    pub(crate) fn start(storage: &ServerStorage, terminfo: PathBuf) -> io::Result<Self> {
        fn assert_send<T: Send>() {}
        assert_send::<TerminalSession>();

        let guard = storage.claim_launch_writer()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = Arc::new(Mutex::new(None));
        let worker_completion = Arc::clone(&completion);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        #[cfg(test)]
        let test_control = Arc::new(Mutex::new(TestControl::default()));
        #[cfg(test)]
        let worker_test_control = Arc::clone(&test_control);
        let thread = thread::Builder::new()
            .name("panelink-launch".to_owned())
            .spawn(move || {
                #[cfg(test)]
                run_worker(
                    receiver,
                    guard,
                    terminfo,
                    worker_completion,
                    worker_cancelled,
                    worker_test_control,
                );
                #[cfg(not(test))]
                run_worker(
                    receiver,
                    guard,
                    terminfo,
                    worker_completion,
                    worker_cancelled,
                );
            })?;
        Ok(Self {
            sender,
            completion,
            cancelled,
            busy: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            #[cfg(test)]
            test_control,
            thread: Some(thread),
        })
    }

    #[cfg(test)]
    pub(crate) fn set_test_hook(&self, hook: Arc<dyn Fn(LaunchStage) + Send + Sync>) {
        self.test_control.lock().unwrap().hook = Some(hook);
    }

    #[cfg(test)]
    pub(crate) fn fail_at_stage(&self, stage: LaunchStage) {
        self.test_control.lock().unwrap().fail_at = Some(stage);
    }

    pub(crate) fn submit_start(&self, payload: Vec<u8>, target: TargetRef) -> io::Result<()> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(io::Error::other("launch worker is unavailable"));
        }
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "launch worker is busy"))?;
        self.sender
            .try_send(Command::Start { payload, target })
            .map_err(|_| {
                self.busy.store(false, Ordering::Release);
                io::Error::new(io::ErrorKind::WouldBlock, "launch worker is unavailable")
            })
    }

    /// Ask the worker to stop at its next safe boundary. This never waits for
    /// filesystem or spawn work already in progress.
    pub(crate) fn request_shutdown(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Request shutdown and wait only until the caller's shared deadline.
    ///
    /// true means the worker thread has ended and its storage/child cleanup
    /// is complete. false means it remains detached and retains its singleton
    /// claim until in-flight work and cleanup finish.
    pub(crate) fn shutdown_until(mut self, deadline: std::time::Instant) -> bool {
        self.request_shutdown();
        loop {
            let finished = self.thread.as_ref().is_none_or(JoinHandle::is_finished);
            if finished {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(Duration::from_millis(1)),
            );
        }
    }

    pub(crate) fn submit_finalize(&self, key: OperationKey, target: TargetRef) -> io::Result<()> {
        if !self.busy.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no launch is active",
            ));
        }
        self.sender
            .try_send(Command::Finalize { key, target })
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "launch worker is unavailable"))
    }

    pub(crate) fn poll(&mut self) -> io::Result<Option<LaunchWorkResult>> {
        let mut slot = self
            .completion
            .lock()
            .map_err(|_| io::Error::other("launch worker failed"))?;
        match slot.take() {
            Some(Ok(result @ LaunchWorkResult::Finished { .. })) => {
                if matches!(
                    result,
                    LaunchWorkResult::Finished {
                        unhealthy: true,
                        ..
                    }
                ) {
                    self.unavailable.store(true, Ordering::Release);
                }
                self.busy.store(false, Ordering::Release);
                Ok(Some(result))
            }
            Some(Ok(result)) => Ok(Some(result)),
            Some(Err(())) => {
                self.unavailable.store(true, Ordering::Release);
                Err(io::Error::other("launch worker failed"))
            }
            None if self.thread.as_ref().is_none_or(JoinHandle::is_finished) => {
                self.unavailable.store(true, Ordering::Release);
                Err(io::Error::other("launch worker stopped"))
            }
            None => Ok(None),
        }
    }
}

impl Drop for LaunchWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

fn publish(slot: &Mutex<Option<Completion>>, result: LaunchWorkResult) -> bool {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_some() {
        return false;
    }
    *slot = Some(Ok(result));
    true
}

#[cfg(test)]
fn stage(control: &Mutex<TestControl>, stage: LaunchStage) -> bool {
    let (hook, fail) = match control.lock() {
        Ok(control) => (control.hook.clone(), control.fail_at == Some(stage)),
        Err(poisoned) => {
            let control = poisoned.into_inner();
            (control.hook.clone(), control.fail_at == Some(stage))
        }
    };
    if let Some(hook) = hook {
        hook(stage);
    }
    fail
}

#[cfg(test)]
macro_rules! at_stage {
    ($control:expr, $stage:expr) => {
        stage($control, $stage)
    };
}

#[cfg(not(test))]
macro_rules! at_stage {
    ($control:expr, $stage:expr) => {{
        let _ = $stage;
        false
    }};
}

fn publish_failure(slot: &Mutex<Option<Completion>>) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let abandoned = slot.replace(Err(()));
    drop(slot);
    // An unclaimed process must be cleaned up on this worker without holding
    // the completion mutex needed by the live owner's nonblocking poll.
    drop(abandoned);
}

fn take_unclaimed_child(slot: &Mutex<Option<Completion>>) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(slot.as_ref(), Some(Ok(LaunchWorkResult::Spawned(_)))) {
        let child = slot.take();
        drop(slot);
        drop(child);
    }
}

fn check_cancelled(cancelled: &AtomicBool, slot: &Mutex<Option<Completion>>) -> bool {
    let is_cancelled = cancelled.load(Ordering::Acquire);
    if is_cancelled {
        take_unclaimed_child(slot);
    }
    is_cancelled
}

fn run_worker(
    receiver: Receiver<Command>,
    mut storage: LaunchWriterGuard,
    terminfo: PathBuf,
    completion: Arc<Mutex<Option<Completion>>>,
    cancelled: Arc<AtomicBool>,
    #[cfg(test)] test_control: Arc<Mutex<TestControl>>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        worker_loop(
            &receiver,
            &mut storage,
            &terminfo,
            &completion,
            &cancelled,
            &test_control,
        );
        #[cfg(not(test))]
        worker_loop(&receiver, &mut storage, &terminfo, &completion, &cancelled);
    }));
    if result.is_err() {
        publish_failure(&completion);
    }
}

fn worker_loop(
    receiver: &Receiver<Command>,
    storage: &mut LaunchWriterGuard,
    terminfo: &std::path::Path,
    completion: &Mutex<Option<Completion>>,
    cancelled: &AtomicBool,
    #[cfg(test)] test_control: &Mutex<TestControl>,
) {
    loop {
        if check_cancelled(cancelled, completion) {
            return;
        }
        let command = match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Command::Start { payload, target } = command else {
            publish_failure(completion);
            return;
        };
        if check_cancelled(cancelled, completion) {
            return;
        }
        #[cfg(test)]
        if at_stage!(test_control, LaunchStage::BeforePrepare) {
            if !publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: false,
                },
            ) {
                return;
            }
            continue;
        }

        let split = match decode_split_request(&payload) {
            Ok(request)
                if request.key().generation == target.generation
                    && request.launch().key() == request.key()
                    && [
                        target.generation,
                        target.workspace,
                        target.pane,
                        target.session,
                        target.occupant_revision,
                    ]
                    .iter()
                    .all(|field| *field != 0) =>
            {
                request
            }
            _ => {
                if !publish(
                    completion,
                    LaunchWorkResult::Finished {
                        outcome: Outcome::Failed,
                        unhealthy: false,
                    },
                ) {
                    return;
                }
                continue;
            }
        };
        let key = split.key();
        let prepared = match prepare_launch(split.launch(), target, terminfo) {
            Ok(prepared) => prepared,
            Err(_) => {
                if !publish(
                    completion,
                    LaunchWorkResult::Finished {
                        outcome: Outcome::Failed,
                        unhealthy: false,
                    },
                ) {
                    return;
                }
                continue;
            }
        };
        if check_cancelled(cancelled, completion) {
            return;
        }
        if at_stage!(test_control, LaunchStage::BeforeIntent) {
            let _ = publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: true,
                },
            );
            continue;
        }
        if check_cancelled(cancelled, completion) {
            return;
        }

        let journal_result = (|| {
            let mut journal = storage.journal()?;
            journal.preflight_launch(key, target.session)?;
            journal.record(LaunchRecord {
                key,
                session: target.session,
                state: LaunchState::Intent,
            })?;
            Ok::<_, io::Error>(())
        })();
        if journal_result.is_err() {
            let _ = publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: true,
                },
            );
            continue;
        }
        if check_cancelled(cancelled, completion) {
            // The child was never started; do not turn shutdown into a replay.
            let _ = publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: true,
                },
            );
            return;
        }
        if at_stage!(test_control, LaunchStage::AfterIntent)
            || at_stage!(test_control, LaunchStage::BeforeSpawn)
        {
            let healthy = record_state(storage, key, target, LaunchState::Failed).is_ok();
            if !publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: !healthy,
                },
            ) {
                return;
            }
            continue;
        }
        if check_cancelled(cancelled, completion) {
            let _ = publish(
                completion,
                LaunchWorkResult::Finished {
                    outcome: Outcome::Failed,
                    unhealthy: true,
                },
            );
            return;
        }

        match prepared.spawn() {
            Err(_) => {
                let healthy = record_state(storage, key, target, LaunchState::Failed).is_ok();
                if !publish(
                    completion,
                    LaunchWorkResult::Finished {
                        outcome: Outcome::Failed,
                        unhealthy: !healthy,
                    },
                ) {
                    return;
                }
            }
            Ok(session) => {
                let failed_after_spawn = at_stage!(test_control, LaunchStage::AfterSpawn);
                if check_cancelled(cancelled, completion) {
                    drop(session);
                    return;
                }
                if !publish(completion, LaunchWorkResult::Spawned(Box::new(session))) {
                    return;
                }
                let _ = at_stage!(test_control, LaunchStage::SpawnPublished);
                if check_cancelled(cancelled, completion) {
                    return;
                }
                loop {
                    if check_cancelled(cancelled, completion) {
                        return;
                    }
                    match receiver.recv_timeout(Duration::from_millis(10)) {
                        Ok(Command::Finalize {
                            key: final_key,
                            target: final_target,
                        }) if final_key == key && final_target == target => {
                            let failed_before_finalize =
                                at_stage!(test_control, LaunchStage::BeforeFinalize);
                            if check_cancelled(cancelled, completion) {
                                return;
                            }
                            let healthy = !failed_after_spawn
                                && !failed_before_finalize
                                && finalize(storage, key, target).is_ok();
                            if !publish(
                                completion,
                                LaunchWorkResult::Finished {
                                    outcome: if healthy {
                                        Outcome::Applied
                                    } else {
                                        Outcome::Unknown
                                    },
                                    unhealthy: !healthy,
                                },
                            ) {
                                return;
                            }
                            break;
                        }
                        Ok(_) => {
                            let _ = publish(
                                completion,
                                LaunchWorkResult::Finished {
                                    outcome: Outcome::Unknown,
                                    unhealthy: true,
                                },
                            );
                            return;
                        }
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
        }
    }
}

fn record_state(
    storage: &mut LaunchWriterGuard,
    key: OperationKey,
    target: TargetRef,
    state: LaunchState,
) -> io::Result<()> {
    storage.journal()?.record(LaunchRecord {
        key,
        session: target.session,
        state,
    })
}

fn finalize(
    storage: &mut LaunchWriterGuard,
    key: OperationKey,
    target: TargetRef,
) -> io::Result<()> {
    let mut journal = storage.journal()?;
    journal.record(LaunchRecord {
        key,
        session: target.session,
        state: LaunchState::Spawned,
    })?;
    journal.record(LaunchRecord {
        key,
        session: target.session,
        state: LaunchState::Applied,
    })
}

#[cfg(test)]
mod tests {
    use super::{LaunchStage, LaunchWorker};
    use crate::control_protocol::TargetRef;
    use crate::server_storage::ServerStorage;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::path::PathBuf::from(format!(
                "/private/tmp/pl-launch-worker-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cancellation_keeps_singleton_until_delayed_worker_exits() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = std::sync::Mutex::new(release_rx);
        worker.set_test_hook(Arc::new(move |stage| {
            if stage == LaunchStage::BeforePrepare {
                let _ = entered_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }
        }));
        let target = TargetRef {
            generation: storage.generation(),
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        worker.submit_start(vec![0], target).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(worker.submit_start(vec![0], target).is_err());
        assert_eq!(
            storage.journal().err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!worker.shutdown_until(Instant::now() + Duration::from_millis(5)));
        assert_eq!(
            storage.claim_launch_writer().err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match storage.claim_launch_writer() {
                Ok(guard) => {
                    drop(guard);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "launch worker did not stop");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("unexpected launch claim error: {error}"),
            }
        }
    }

    fn split_payload(
        generation: u64,
        marker: &std::path::Path,
    ) -> (Vec<u8>, TargetRef, TargetRef, crate::protocol::OperationKey) {
        use crate::managed_launch_protocol::LaunchRequest;
        use crate::pane_layout::SplitDirection;
        use crate::protocol::OperationKey;
        use crate::split_protocol::{encode_split_request, SplitRequest};
        use std::os::unix::ffi::OsStrExt;

        let key = OperationKey {
            generation,
            lease: 1,
            sequence: 1,
        };
        let existing = TargetRef {
            generation,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let created = TargetRef {
            generation,
            workspace: 1,
            pane: 2,
            session: 2,
            occupant_revision: 1,
        };
        let arguments: [&[u8]; 4] = [
            b"-c",
            b"printf '%s\\n' \"$$\" > \"$1\"; exec /bin/sleep 60",
            b"panelink-test",
            marker.as_os_str().as_bytes(),
        ];
        let launch = LaunchRequest {
            key,
            rows: 24,
            columns: 80,
            requested_executable: b"/bin/sh",
            requested_working_directory: b"/",
            client_working_directory: b"/",
            arguments: &arguments,
            environment: &[],
        };
        let mut payload = Vec::new();
        encode_split_request(
            &SplitRequest {
                key,
                target: existing,
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
                launch: &launch,
            },
            &mut payload,
        )
        .unwrap();
        (payload, existing, created, key)
    }

    fn wait_for_marker(path: &std::path::Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(value) = fs::read_to_string(path) {
                if let Ok(pid) = value.trim().parse() {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "test child did not start");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_result(worker: &mut LaunchWorker) -> super::LaunchWorkResult {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = worker.poll().unwrap() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "launch worker completion timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_child_exit(pid: i32) {
        use rustix::process::{test_kill_process, Pid};
        let pid = Pid::from_raw(pid).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while test_kill_process(pid).is_ok() {
            assert!(Instant::now() < deadline, "test child was not reaped");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn real_split_transfers_one_child_then_finalizes_and_rejects_another_start() {
        let fixture = Fixture::new();
        let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let marker = fixture.0.join("started");
        let (payload, _existing, created, key) = split_payload(storage.generation(), &marker);
        let mut worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        worker.submit_start(payload.clone(), created).unwrap();
        let adopted = match wait_for_result(&mut worker) {
            super::LaunchWorkResult::Spawned(session) => session,
            super::LaunchWorkResult::Finished { .. } => panic!("expected spawned child"),
        };
        let pid = wait_for_marker(&marker);
        assert!(worker.submit_start(payload, created).is_err());
        worker.submit_finalize(key, created).unwrap();
        assert!(matches!(
            wait_for_result(&mut worker),
            super::LaunchWorkResult::Finished {
                outcome: crate::protocol::Outcome::Applied,
                unhealthy: false,
            }
        ));
        assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
        drop(adopted);
        wait_for_child_exit(pid);
        drop(worker);
    }

    #[test]
    fn intent_refusal_reports_no_spawn_and_latches_worker_unavailable() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let marker = fixture.0.join("must-not-start");
        let (payload, _existing, created, key) = split_payload(storage.generation(), &marker);
        let mut worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        worker.fail_at_stage(LaunchStage::BeforeIntent);
        worker.submit_start(payload, created).unwrap();
        assert!(matches!(
            wait_for_result(&mut worker),
            super::LaunchWorkResult::Finished {
                outcome: crate::protocol::Outcome::Failed,
                unhealthy: true,
            }
        ));
        assert!(!marker.exists());
        assert!(worker.submit_start(vec![0], created).is_err());
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match storage.journal() {
                Ok(journal) => {
                    assert!(journal.recovered(key).is_none());
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "worker claim did not release");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("unexpected journal error: {error}"),
            }
        }
    }

    #[test]
    fn finalize_uncertainty_keeps_transferred_child_owned_and_never_replays() {
        let fixture = Fixture::new();
        let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let marker = fixture.0.join("started-once");
        let (payload, _existing, created, key) = split_payload(storage.generation(), &marker);
        let mut worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        worker.fail_at_stage(LaunchStage::BeforeFinalize);
        worker.submit_start(payload.clone(), created).unwrap();
        let adopted = match wait_for_result(&mut worker) {
            super::LaunchWorkResult::Spawned(session) => session,
            super::LaunchWorkResult::Finished { .. } => panic!("expected spawned child"),
        };
        let pid = wait_for_marker(&marker);
        worker.submit_finalize(key, created).unwrap();
        assert!(matches!(
            wait_for_result(&mut worker),
            super::LaunchWorkResult::Finished {
                outcome: crate::protocol::Outcome::Unknown,
                unhealthy: true,
            }
        ));
        assert!(worker.submit_start(payload, created).is_err());
        assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
        drop(worker);
        assert!(
            rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap())
                .is_ok()
        );
        drop(adopted);
        wait_for_child_exit(pid);
    }

    #[test]
    fn cancellation_cleans_unconsumed_spawn_result_on_worker_before_unlock() {
        let fixture = Fixture::new();
        let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let marker = fixture.0.join("unconsumed");
        let (payload, _existing, created, _key) = split_payload(storage.generation(), &marker);
        let worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = std::sync::Mutex::new(release_rx);
        worker.set_test_hook(Arc::new(move |stage| {
            if stage == LaunchStage::SpawnPublished {
                let _ = entered_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }
        }));
        worker.submit_start(payload, created).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let pid = wait_for_marker(&marker);
        drop(worker);
        assert_eq!(
            storage.claim_launch_writer().err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        release_tx.send(()).unwrap();
        wait_for_child_exit(pid);

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match storage.claim_launch_writer() {
                Ok(guard) => {
                    drop(guard);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "worker kept singleton after cleanup"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("unexpected launch claim error: {error}"),
            }
        }
    }

    #[test]
    fn injected_preparation_failure_is_reported_without_journal_work() {
        let fixture = Fixture::new();
        let mut storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let mut worker = LaunchWorker::start(&storage, fixture.0.clone()).unwrap();
        worker.fail_at_stage(LaunchStage::BeforePrepare);
        let target = TargetRef {
            generation: storage.generation(),
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        worker.submit_start(vec![0], target).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(result) = worker.poll().unwrap() {
                assert!(matches!(
                    result,
                    super::LaunchWorkResult::Finished {
                        outcome: crate::protocol::Outcome::Failed,
                        unhealthy: false,
                    }
                ));
                break;
            }
            assert!(Instant::now() < deadline, "worker completion timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match storage.journal() {
                Ok(journal) => {
                    assert_eq!(
                        journal.recovered(crate::protocol::OperationKey {
                            generation: target.generation,
                            lease: 1,
                            sequence: 1,
                        }),
                        None
                    );
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "worker claim did not release");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("unexpected journal error: {error}"),
            }
        }
    }
}
