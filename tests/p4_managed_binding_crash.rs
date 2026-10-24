#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Process-kill evidence for atomic managed/provider and one-shot recovery rows.
//! These cases do not model machine power loss or filesystem-controller failure.

use panelink::control_protocol::TargetRef;
use panelink::managed_provider_store::{
    ManagedLaunchOutcome, ManagedLifecycleSnapshot, ManagedProviderRecord, ManagedProviderScope,
    ManagedProviderStore,
};
use panelink::protocol::OperationKey;
use panelink::provider_binding::{
    Binding, BindingState, LastConfirmedRecord, ProviderBinding, VerificationSource,
};
use panelink::provider_recovery_prepared::{PreparedResumeState, PreparedResumeStore};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);
const PROJECT: &[u8] = b"/private/panelink-crash-fixture-project";
const VERSION: &str = "2.0.32";
const MARKER: &str = "boundary";
const CHILD_MODE: &str = "PANELINK_MANAGED_CRASH_CHILD";
const CHILD_DIR: &str = "PANELINK_MANAGED_CRASH_DIRECTORY";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "panelink-managed-crash-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(fs::canonicalize(path).expect("canonical fixture")),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => panic!("create private crash fixture"),
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
        generation: 301,
        workspace: 307,
        pane: 311,
        session: 313,
        occupant_revision: 317,
    }
}

fn scope() -> ManagedProviderScope {
    ManagedProviderScope::new(
        target(),
        PROJECT,
        VERSION,
        OperationKey {
            generation: target().generation,
            lease: 331,
            sequence: 337,
        },
        [0x5a; 32],
        Some(b"/usr/local/bin/opencode"),
    )
    .expect("bounded complete managed scope")
}

fn binding(revision: u64) -> Binding {
    Binding::new(
        target(),
        &panelink::provider_binding::ProjectIdentity::new(PROJECT).unwrap(),
        &panelink::provider_binding::SupportedVersion::new(VERSION).unwrap(),
        347,
        revision,
        if revision == 1 {
            "session_A"
        } else {
            "session_B"
        },
        1_800_000_000_000 + revision,
        VerificationSource::TuiRouteSnapshot,
    )
    .unwrap()
}

fn combined(revision: u64) -> ManagedProviderRecord {
    ManagedProviderRecord::new(
        scope(),
        ManagedLifecycleSnapshot::Running,
        ManagedLaunchOutcome::Applied,
        binding(revision),
    )
    .unwrap()
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_at_boundary(directory: &Path, marker: &str, label: &[u8]) -> ! {
    fs::write(directory.join(marker), label).expect("fixed child boundary marker");
    loop {
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

fn managed_store(path: &Path) -> ManagedProviderStore {
    ManagedProviderStore::open(path, scope()).expect("open scoped combined store")
}

fn prepared_store(path: &Path) -> PreparedResumeStore {
    PreparedResumeStore::open(path, scope()).expect("open scoped prepared store")
}

/// Child entrypoint used only by parent cases in this integration binary.
#[test]
fn managed_and_prepared_crash_child() {
    let Ok(stage) = std::env::var(CHILD_MODE) else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os(CHILD_DIR).expect("fixture path"));
    match stage.as_str() {
        "managed_before_save" => {
            let store = managed_store(&directory);
            assert_eq!(store.load().unwrap().unwrap().binding().revision(), 1);
            wait_at_boundary(&directory, MARKER, b"managed-before-save")
        }
        "managed_after_publish" => {
            let mut store = managed_store(&directory);
            store
                .publish(&combined(2))
                .expect("publish rev2 combined record");
            // This marker proves publication returned; no owner completion/ack
            // is emitted in this stage.
            wait_at_boundary(&directory, MARKER, b"published-no-owner-ack")
        }
        "managed_after_ack" => {
            let mut store = managed_store(&directory);
            store
                .publish(&combined(2))
                .expect("publish rev2 combined record");
            wait_at_boundary(&directory, "owner-ack", b"owner-ack-received")
        }
        "prepared_create" => {
            let mut store = prepared_store(&directory);
            store
                .prepare("chosen_session", 19, 23, 1_800_000_000_019)
                .expect("publish prepared choice");
            wait_at_boundary(&directory, MARKER, b"prepared-no-selection")
        }
        "spent_before_action" | "spent_after_action_ack" => {
            let mut store = prepared_store(&directory);
            let record = store.load().unwrap().expect("prepared fixture row");
            assert_eq!(record.state(), PreparedResumeState::Prepared);
            let ticket = record.ticket().clone();
            store
                .mark_spent(&ticket, 19)
                .expect("persist Spent before action");
            if stage == "spent_before_action" {
                wait_at_boundary(&directory, MARKER, b"spent-no-external-action")
            } else {
                // A fixture-owned marker models one external action and its
                // acknowledgment; it is not a real provider navigation.
                fs::write(directory.join("fixture-action"), b"one").unwrap();
                wait_at_boundary(&directory, "owner-ack", b"fixture-action-acked")
            }
        }
        _ => panic!("unknown child stage"),
    }
}

fn run_child(directory: &Path, stage: &str, marker: &str) {
    let executable = std::env::current_exe().expect("test executable");
    let mut command = Command::new(executable);
    command
        .args(["--exact", "managed_and_prepared_crash_child", "--nocapture"])
        .env(CHILD_MODE, stage)
        .env(CHILD_DIR, directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = OwnedChild(command.spawn().expect("spawn owned interruption fixture"));
    let ready = directory.join(marker);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.is_file() && Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before marker"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.is_file(), "child marker deadline");
    child.0.kill().expect("kill child");
    assert_eq!(child.0.wait().unwrap().signal(), Some(9));
}

#[test]
fn managed_record_kill_boundaries_publish_scope_and_binding_together() {
    for (stage, expected_revision, acknowledged) in [
        ("managed_before_save", 1, false),
        ("managed_after_publish", 2, false),
        ("managed_after_ack", 2, true),
    ] {
        let fixture = Fixture::new();
        {
            let mut store = managed_store(fixture.path());
            store.publish(&combined(1)).unwrap();
        }
        run_child(
            fixture.path(),
            stage,
            if acknowledged { "owner-ack" } else { MARKER },
        );
        let store = managed_store(fixture.path());
        let record = store
            .load()
            .unwrap()
            .expect("one combined historical record");
        assert_eq!(record.binding().revision(), expected_revision);
        assert_eq!(
            record.binding().session_id(),
            if expected_revision == 1 {
                "session_A"
            } else {
                "session_B"
            }
        );
        assert_eq!(record.scope().target(), target());
        assert_eq!(record.scope().project(), PROJECT);
        assert_eq!(record.scope().provider_version(), VERSION);
        assert_eq!(
            record.scope().executable_fingerprint().as_bytes(),
            &[0x5a; 32]
        );
        assert_eq!(record.scope().launch_operation().generation, 301);
        assert_eq!(record.scope().launch_operation().lease, 331);
        assert_eq!(record.scope().launch_operation().sequence, 337);
        assert_eq!(record.lifecycle(), ManagedLifecycleSnapshot::Running);
        assert_eq!(record.outcome(), ManagedLaunchOutcome::Applied);
        assert_eq!(fixture.path().join("owner-ack").is_file(), acknowledged);

        let last = LastConfirmedRecord::from_validated_binding(record.into_binding());
        let restored = ProviderBinding::restore(target(), PROJECT, VERSION, 353, last).unwrap();
        assert_eq!(restored.state(), BindingState::Stale);
        assert!(restored.verified_current().is_none());
        assert!(restored.pending().is_none());
    }
}

#[test]
fn prepared_and_spent_kills_never_become_a_selected_binding_or_reissue_action() {
    let prepared_fixture = Fixture::new();
    run_child(prepared_fixture.path(), "prepared_create", MARKER);
    let record = prepared_store(prepared_fixture.path())
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(record.state(), PreparedResumeState::Prepared);
    assert_eq!(record.choice_id(), "chosen_session");
    assert_eq!(record.scope().target(), target());
    assert_eq!(record.scope().project(), PROJECT);
    assert!(!prepared_fixture.path().join("fixture-action").exists());
    assert!(!prepared_fixture
        .path()
        .join("managed-provider.record")
        .exists());

    for (stage, action_acked) in [
        ("spent_before_action", false),
        ("spent_after_action_ack", true),
    ] {
        let fixture = Fixture::new();
        {
            let mut store = prepared_store(fixture.path());
            store
                .prepare("chosen_session", 19, 23, 1_800_000_000_019)
                .unwrap();
        }
        run_child(
            fixture.path(),
            stage,
            if action_acked { "owner-ack" } else { MARKER },
        );
        let record = prepared_store(fixture.path()).load().unwrap().unwrap();
        assert_eq!(record.state(), PreparedResumeState::Spent);
        assert_eq!(record.choice_id(), "chosen_session");
        assert_eq!(record.scope().target(), target());
        assert_eq!(record.scope().project(), PROJECT);
        assert_eq!(
            fixture.path().join("fixture-action").is_file(),
            action_acked
        );
        assert_eq!(fixture.path().join("owner-ack").is_file(), action_acked);
        assert!(!fixture.path().join("managed-provider.record").exists());
        let mut reopened = prepared_store(fixture.path());
        assert!(reopened
            .mark_spent(record.ticket(), record.prospective_revision())
            .is_err());
        assert_eq!(
            reopened.load().unwrap().unwrap().state(),
            PreparedResumeState::Spent
        );
    }
}
