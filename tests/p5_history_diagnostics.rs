#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Public observers survive both successful teardown and rejected setup.
use panelink::history_diagnostics::{HistoryDrainOutcome, HistoryReason};
use panelink::history_store::HistoryStore;
use panelink::launch::LaunchContext;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::server_storage::ServerStorage;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-diag-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn observe_setup(wrong_domain: bool) {
    let fixture = Fixture::new();
    let server = fixture.0.join("server");
    let terminfo = fixture.0.join("terminfo");
    fs::DirBuilder::new().mode(0o700).create(&server).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();
    let storage = ServerStorage::initialize(&server).unwrap();
    let context = LaunchContext {
        working_directory: fixture.0.clone(),
        environment: [
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), fixture.0.as_os_str().to_owned()),
        ]
        .into_iter()
        .collect(),
    };
    let executable = OsString::from("/bin/sh");
    let arguments = vec![
        OsString::from("-c"),
        OsString::from("printf 'DIAGNOSTIC-READY\\r\\n'; exit 0"),
    ];
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
        false,
    )
    .unwrap();
    let target = started.startup().target.unwrap();
    let mut driver = started.into_driver();
    let observer = driver.history_diagnostics();
    let before = observer.snapshot();
    assert_eq!(before.reasons().len(), 0);
    assert_eq!(
        before.writer_drain_outcome(),
        HistoryDrainOutcome::NotRequested
    );
    let path = if wrong_domain {
        fixture.0.join("wrong-history")
    } else {
        server.join("history")
    };
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    driver.enable_history(HistoryStore::open(&path).unwrap());
    let failed = driver.history_failure_signal();
    if !wrong_domain {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let _ = driver.tick(|| 1);
            driver.capture_now();
            if HistoryStore::open(&path)
                .unwrap()
                .load_rows(target)
                .unwrap()
                .is_some_and(|saved| saved.ended_at_unix_ms != 0)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "natural exit capture did not settle"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    } else {
        assert!(!driver.history_available());
        let _ = driver.tick(|| 1);
    }
    drop(driver);
    // Settlement is the boundary after which detached-worker events are stable.
    drop(ServerStorage::lock_for_maintenance(&server).unwrap());
    let after = observer.snapshot();
    assert_eq!(driverless_snapshot(&observer), after);
    assert_eq!(failed.load(Ordering::Acquire), wrong_domain);
    if wrong_domain {
        assert!(after.contains(HistoryReason::CaptureSetupFailed));
        assert_eq!(after.reasons().len(), 1);
        assert_eq!(
            after.writer_drain_outcome(),
            HistoryDrainOutcome::NotRequested
        );
    } else {
        assert_eq!(after.writer_drain_outcome(), HistoryDrainOutcome::Drained);
        assert!(after.reasons().all(|reason| matches!(
            reason,
            HistoryReason::AdmissionContended
                | HistoryReason::AdmissionCountFull
                | HistoryReason::AdmissionBytesFull
        )));
    }
}

fn driverless_snapshot(
    observer: &panelink::history_diagnostics::HistoryDiagnostics,
) -> panelink::history_diagnostics::HistoryDiagnosticSnapshot {
    observer.clone().snapshot()
}

#[test]
fn pre_enable_observer_survives_clean_driver_shutdown() {
    observe_setup(false);
}

#[test]
fn pre_enable_observer_sees_rejected_store_domain() {
    observe_setup(true);
}
