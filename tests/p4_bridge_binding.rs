#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Component integration and process-interruption evidence, not provider resume.

use panelink::control_protocol::TargetRef;
use panelink::provider_binding::{
    Binding, BindingObservation, BindingState, ObservationScope, ProjectIdentity, ProviderBinding,
    SupportedVersion, VerificationSource,
};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);
const VERSION: &str = "2.0.32";
const PROJECT: &[u8] = b"canonical-private-fixture-project";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "panelink-binding-component-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(fs::canonicalize(path).expect("canonical fixture")),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => panic!("create private binding fixture"),
            }
        }
        panic!("private fixture allocation exhausted")
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 2,
        pane: 3,
        session: 4,
        occupant_revision: 5,
    }
}

fn binding(revision: u64, session: &str) -> Binding {
    Binding::new(
        target(),
        &ProjectIdentity::new(PROJECT).unwrap(),
        &SupportedVersion::new(VERSION).unwrap(),
        7,
        revision,
        session,
        1000 + revision,
        VerificationSource::TuiRouteSnapshot,
    )
    .unwrap()
}

fn observation(revision: u64, session: &str) -> BindingObservation {
    BindingObservation::new(
        ObservationScope::SelectedTopLevel,
        target(),
        &ProjectIdentity::new(PROJECT).unwrap(),
        &SupportedVersion::new(VERSION).unwrap(),
        7,
        revision,
        session,
        1000 + revision,
        VerificationSource::TuiRouteSnapshot,
    )
    .unwrap()
}

/// Reap on every parent assertion failure, including a failed ready handshake.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn kill_after_ready(directory: &Path, stage: &str) {
    let executable = std::env::current_exe().unwrap();
    let mut child = OwnedChild(
        Command::new(executable)
            .args(["--exact", "crash_child", "--nocapture"])
            .env("PANELINK_BINDING_TEST_CHILD", stage)
            .env("PANELINK_BINDING_TEST_DIRECTORY", directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated interruption fixture"),
    );
    let ready = directory.join("ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.is_file(), "fixture ready deadline");
    child.0.kill().expect("kill isolated fixture");
    let status = child.0.wait().expect("reap isolated fixture");
    assert_eq!(
        status.signal(),
        Some(9),
        "fixture must be killed with SIGKILL"
    );
}

fn ready_and_wait(directory: &Path) -> ! {
    fs::write(directory.join("ready"), b"ready").expect("ready handshake");
    loop {
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

fn store(directory: &Path) -> panelink::provider_binding_store::ProviderBindingStore {
    panelink::provider_binding_store::ProviderBindingStore::open(
        directory,
        target(),
        PROJECT,
        VERSION,
    )
    .expect("open private binding store")
}

#[test]
fn core_acknowledges_only_a_successful_store_and_restores_as_stale() {
    let fixture = Fixture::new();
    let mut storage = store(fixture.path());
    let mut core = ProviderBinding::new(target(), PROJECT, VERSION, 7).unwrap();
    core.observe(observation(1, "conversation_A")).unwrap();
    core.confirm_pending(1, |candidate| storage.save(candidate).map(|_| ()))
        .unwrap();
    assert_eq!(core.state(), BindingState::Verified);
    core.observe(observation(2, "conversation_B")).unwrap();
    std::os::unix::fs::symlink(
        "untrusted",
        fixture.path().join(".last-confirmed.binding.tmp"),
    )
    .expect("plant refused temporary name");
    assert!(core
        .confirm_pending(2, |candidate| storage.save(candidate).map(|_| ()))
        .is_err());
    assert_eq!(core.state(), BindingState::Pending);
    assert_eq!(core.pending().unwrap().session_id(), "conversation_B");
    assert_eq!(
        core.last_confirmed().unwrap().session_id(),
        "conversation_A"
    );
    let record = storage.load().unwrap().unwrap();
    assert_eq!(record.binding().revision(), 1);
    let restored = ProviderBinding::restore(target(), PROJECT, VERSION, 8, record).unwrap();
    assert_eq!(restored.state(), BindingState::Stale);
    assert!(restored.verified_current().is_none());
}

/// Executed in a separate process by the bounded parent handshake. A normal
/// discovery invocation has no child mode and performs no writes.
#[test]
fn crash_child() {
    let Ok(stage) = std::env::var("PANELINK_BINDING_TEST_CHILD") else {
        return;
    };
    let directory = PathBuf::from(
        std::env::var_os("PANELINK_BINDING_TEST_DIRECTORY").expect("child fixture directory"),
    );
    let mut storage = store(&directory);
    let record = storage.load().unwrap().unwrap();
    let mut core = ProviderBinding::restore(target(), PROJECT, VERSION, 7, record).unwrap();
    core.observe(observation(2, "conversation_B")).unwrap();
    match stage.as_str() {
        "before_write" => ready_and_wait(&directory),
        "after_commit_before_ack" => {
            let _: Result<(), _> = core.confirm_pending(2, |candidate| -> io::Result<()> {
                storage.save(candidate)?;
                ready_and_wait(&directory)
            });
            unreachable!("parent must interrupt before acknowledgment");
        }
        "after_ack" => {
            core.confirm_pending(2, |candidate| storage.save(candidate).map(|_| ()))
                .unwrap();
            assert_eq!(core.state(), BindingState::Verified);
            ready_and_wait(&directory)
        }
        _ => panic!("invalid interruption stage"),
    }
}

#[test]
fn process_kill_boundaries_reopen_only_last_record_and_always_restore_stale() {
    for (stage, expected_revision) in [
        ("before_write", 1),
        ("after_commit_before_ack", 2),
        ("after_ack", 2),
    ] {
        let fixture = Fixture::new();
        {
            let mut storage = store(fixture.path());
            storage.save(&binding(1, "conversation_A")).unwrap();
        }
        kill_after_ready(fixture.path(), stage);
        // Successful open also proves that kernel-held writer exclusion was
        // released when the interrupted process was reaped.
        let storage = store(fixture.path());
        let record = storage.load().unwrap().unwrap();
        assert_eq!(record.binding().revision(), expected_revision);
        let restored = ProviderBinding::restore(target(), PROJECT, VERSION, 8, record).unwrap();
        assert_eq!(restored.state(), BindingState::Stale);
        assert!(restored.verified_current().is_none());
        assert!(restored.pending().is_none());
    }
}

#[test]
fn bridge_storage_ack_expiry_and_reconciliation_share_one_confirmation_boundary() {
    use panelink::provider_bridge::{
        Admission, BridgeReceiver, Frame, FrameScope, LeasePolicy, Selection,
        BRIDGE_PROTOCOL_VERSION,
    };
    fn frame(sequence: u64, session: &str) -> Frame {
        Frame::new(
            BRIDGE_PROTOCOL_VERSION,
            target(),
            PROJECT,
            VERSION,
            7,
            sequence,
            FrameScope::TopLevel,
            Selection::session(session, VerificationSource::TuiRouteSnapshot).unwrap(),
        )
        .unwrap()
    }
    let fixture = Fixture::new();
    let mut storage = store(fixture.path());
    let mut bridge =
        BridgeReceiver::new(target(), PROJECT, 7, LeasePolicy::new(100).unwrap()).unwrap();
    bridge.request_reconciliation();
    assert_eq!(
        bridge
            .receive(frame(1, "conversation_A"), 10, 1_700_000_000_000)
            .unwrap(),
        Admission::Pending { revision: 1 }
    );
    bridge
        .confirm_pending(1, 11, |candidate| storage.save(candidate).map(|_| ()))
        .unwrap();
    assert_eq!(bridge.state(), BindingState::Verified);
    assert_eq!(
        storage
            .load()
            .unwrap()
            .unwrap()
            .binding()
            .verified_at_unix_ms(),
        1_700_000_000_000
    );

    assert_eq!(
        bridge
            .receive(frame(2, "conversation_B"), 20, 1_700_000_000_010)
            .unwrap(),
        Admission::Pending { revision: 2 }
    );
    bridge.tick(120).unwrap();
    let mut writer_called = false;
    assert!(bridge
        .confirm_pending(2, 120, |_| {
            writer_called = true;
            Ok::<(), ()>(())
        })
        .is_err());
    assert!(!writer_called);
    assert_eq!(storage.load().unwrap().unwrap().binding().revision(), 1);
    assert_eq!(bridge.state(), BindingState::Stale);
    assert_eq!(
        bridge
            .receive(frame(3, "conversation_B"), 121, 1_700_000_000_111)
            .unwrap(),
        Admission::Revoked
    );
    bridge.request_reconciliation();
    assert_eq!(
        bridge
            .receive(frame(4, "conversation_B"), 122, 1_700_000_000_112)
            .unwrap(),
        Admission::Pending { revision: 3 }
    );
    bridge
        .confirm_pending(3, 123, |candidate| storage.save(candidate).map(|_| ()))
        .unwrap();
    let record = storage.load().unwrap().unwrap();
    assert_eq!(record.binding().revision(), 3);
    let restored = BridgeReceiver::restore(
        target(),
        PROJECT,
        8,
        LeasePolicy::new(100).unwrap(),
        record,
        bridge.revision_floor(),
    )
    .unwrap();
    assert_eq!(restored.state(), BindingState::Stale);
    assert!(restored.pending().is_none());
    assert_eq!(
        restored.last_confirmed().unwrap().session_id(),
        "conversation_B"
    );
}
