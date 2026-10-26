#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-child regression for shared multi-server driver shutdown deadlines.

use panelink::managed_launch::LaunchJournalPort;
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use panelink::multi_runtime::MultiServerDriver;
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{Ledger, OperationKey};
use panelink::server::multi_session::MultiSessionOwner;
use panelink::server_pty::PtyEffects;
use panelink::server_storage::{LaunchRecord, LaunchState, ServerStorage};
use panelink::split_protocol::{encode_split_request, SplitRequest};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const LEASE: u64 = 1;
const ROWS: u16 = 24;
const COLUMNS: u16 = 81;
const SPLIT_CONTENT_ROWS: u16 = ROWS - 2;
const SPLIT_CONTENT_COLUMNS: u16 = (COLUMNS - 1) / 2 - 2;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = Path::new("/private/tmp").join(format!(
            "panelink-w2-shutdown-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create private fixture");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("terminfo"))
            .expect("create private terminfo directory");
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Journal(HashSet<u64>);

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _: OperationKey, session: u64) -> io::Result<()> {
        if self.0.contains(&session) {
            Err(io::Error::other("duplicate session"))
        } else {
            Ok(())
        }
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        if record.state == LaunchState::Intent && !self.0.insert(record.session) {
            return Err(io::Error::other("duplicate session"));
        }
        Ok(())
    }
}

fn environment(home: &[u8]) -> [EnvironmentEntry<'_>; 3] {
    [
        EnvironmentEntry {
            name: b"TERM",
            value: b"panelink-probe",
        },
        EnvironmentEntry {
            name: b"PATH",
            value: b"/usr/bin:/bin",
        },
        EnvironmentEntry {
            name: b"HOME",
            value: home,
        },
    ]
}

fn key(generation: u64, sequence: u64) -> OperationKey {
    OperationKey {
        generation,
        lease: LEASE,
        sequence,
    }
}

fn wait_for_files(root: &Path, names: &[&str], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if names.iter().all(|name| root.join(name).is_file()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child markers not ready: {names:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn assert_reaped(pid_path: &Path) {
    let pid = fs::read_to_string(pid_path)
        .expect("child pid marker")
        .trim()
        .parse::<i32>()
        .expect("valid child pid");
    assert!(pid > 0, "child PID must name one process");
    // SAFETY: signal zero only queries this positive child PID.
    let result = unsafe { libc::kill(pid, 0) };
    let error = io::Error::last_os_error();
    assert_eq!(result, -1, "child PID {pid} remains after driver drop");
    assert_eq!(
        error.raw_os_error(),
        Some(libc::ESRCH),
        "only ESRCH proves the direct child is absent"
    );
}

#[test]
fn driver_drop_signals_both_children_before_shared_escalation_deadlines() {
    let fixture = Fixture::new();
    let root = fixture.0.as_os_str().as_encoded_bytes().to_vec();
    let terminfo = fixture.0.join("terminfo");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.0.join("server"))
        .expect("create server directory");
    let storage =
        ServerStorage::initialize(&fixture.0.join("server")).expect("initialize server storage");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).expect("ledger");
    ledger.open_lease(LEASE, 0).expect("open lease");
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true).expect("owner");
    let mut journal = Journal::default();

    let first_script = br#"
        trap 'printf hup >> "$1/hup-a"; n=0; while [ ! -f "$1/hup-b" ] && [ "$n" -lt 40 ]; do read -r -t 0.01 _ || :; n=$((n+1)); done; if [ ! -f "$1/hup-b" ]; then printf missing > "$1/peer-missing-a"; fi' HUP
        printf '%s\n' "$$" > "$1/pid-a"
        printf ready > "$1/ready-a"
        while :; do read -r -t 10 _ || :; done
    "#;
    let first_arguments: [&[u8]; 4] = [b"-c", first_script, b"panelink-w2-shutdown", &root];
    let first_environment = environment(&root);
    let first_request = LaunchRequest {
        key: key(generation, 1),
        rows: ROWS,
        columns: COLUMNS,
        requested_executable: b"/bin/sh",
        requested_working_directory: &root,
        client_working_directory: &root,
        arguments: &first_arguments,
        environment: &first_environment,
    };
    let mut first_payload = Vec::new();
    encode_launch_request(&first_request, &mut first_payload).expect("encode first launch");
    let first = owner
        .execute_launch(
            &first_payload,
            0,
            &mut journal,
            |request, target| prepare_launch(request, target, &terminfo).map_err(|_| ()),
            |prepared: PreparedLaunch| prepared.spawn().map(PtyEffects::new).map_err(|_| ()),
        )
        .expect("launch first child")
        .target
        .expect("first target");

    let second_script = br#"
        trap 'printf hup >> "$1/hup-b"; n=0; while [ ! -f "$1/hup-a" ] && [ "$n" -lt 40 ]; do read -r -t 0.01 _ || :; n=$((n+1)); done; if [ ! -f "$1/hup-a" ]; then printf missing > "$1/peer-missing-b"; fi' HUP
        printf '%s\n' "$$" > "$1/pid-b"
        printf ready > "$1/ready-b"
        while :; do read -r -t 10 _ || :; done
    "#;
    let second_arguments: [&[u8]; 4] = [b"-c", second_script, b"panelink-w2-shutdown", &root];
    let second_environment = environment(&root);
    let second_request = LaunchRequest {
        key: key(generation, 2),
        rows: SPLIT_CONTENT_ROWS,
        columns: SPLIT_CONTENT_COLUMNS,
        requested_executable: b"/bin/sh",
        requested_working_directory: &root,
        client_working_directory: &root,
        arguments: &second_arguments,
        environment: &second_environment,
    };
    let split = SplitRequest {
        key: key(generation, 2),
        target: first,
        expected_layout_revision: 1,
        direction: SplitDirection::Right,
        launch: &second_request,
    };
    let mut split_bytes = Vec::new();
    encode_split_request(&split, &mut split_bytes).expect("encode split");
    owner
        .execute_split(
            &split_bytes,
            0,
            &mut journal,
            |request, target| prepare_launch(request, target, &terminfo).map_err(|_| ()),
            |prepared: PreparedLaunch| prepared.spawn().map(PtyEffects::new).map_err(|_| ()),
            |effects: &mut PtyEffects, rows, columns| effects.resize(rows, columns),
        )
        .expect("launch second child");
    drop(journal);

    let driver = MultiServerDriver::new(owner, storage, terminfo);
    wait_for_files(&fixture.0, &["ready-a", "ready-b"], Duration::from_secs(5));
    let started = Instant::now();
    drop(driver);
    let elapsed = started.elapsed();

    assert!(
        fixture.0.join("hup-a").is_file(),
        "first child did not receive HUP"
    );
    assert!(
        fixture.0.join("hup-b").is_file(),
        "second child did not receive HUP"
    );
    assert!(
        !fixture.0.join("peer-missing-a").exists() && !fixture.0.join("peer-missing-b").exists(),
        "one HUP arrived only after the other child's grace interval"
    );
    assert_reaped(&fixture.0.join("pid-a"));
    assert_reaped(&fixture.0.join("pid-b"));
    // The two shared one-second grace windows should finish well below two
    // serialized two-second child shutdowns; markers above are the primary
    // ordering proof, this bound is only a broad regression guard.
    assert!(
        elapsed >= Duration::from_millis(800),
        "children did not remain alive through most of the HUP grace: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "driver shutdown took {elapsed:?}"
    );
}
