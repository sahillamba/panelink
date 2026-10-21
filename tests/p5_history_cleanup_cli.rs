#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-history-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
            .unwrap();
        Self(root)
    }

    fn history(&self) -> PathBuf {
        self.0.join("server/history")
    }

    fn create_history(&self) {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.history())
            .unwrap();
    }

    fn command(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PANELINK_HOME", &self.0)
            .env("HOME", &self.0)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/false")
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = i32::try_from(child.id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                // SAFETY: this is the isolated group created for this child.
                unsafe { libc::kill(-group, libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                panic!("history CLI exceeded its deadline");
            }
            thread::sleep(Duration::from_millis(5));
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane: 2,
        session: 3,
        occupant_revision: 1,
    }
}

fn temporary_name(counter: u64) -> String {
    format!(
        ".{:016x}-{:016x}-{:016x}-{:016x}-{:016x}.rows-123-{counter}.tmp",
        7, 1, 2, 3, 1
    )
}

fn write_private(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn cleanup_cli_removes_only_uncommitted_files_and_readers_leave_them_alone() {
    let fixture = Fixture::new();
    let storage = ServerStorage::initialize(&fixture.0.join("server")).unwrap();
    fixture.create_history();
    let store = HistoryStore::open(&fixture.history()).unwrap();
    store.begin_history(target(), 1, false).unwrap();
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
    let checkpoint = screen_checkpoint::encode(
        &snapshot,
        &CheckpointMetadata::new(target(), 1, 123, 1).unwrap(),
    )
    .unwrap();
    store
        .save(target(), ScreenBuffer::Normal, &checkpoint)
        .unwrap();
    let committed = fs::read_dir(fixture.history())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let temporary = fixture.history().join(temporary_name(1));
    write_private(&temporary, b"incomplete");
    let unknown = fixture.history().join(".unknown.tmp");
    write_private(&unknown, b"keep");
    drop(store);
    drop(storage);

    let shown = fixture.command(&["history", "show", "7:1:2:3:1"]);
    assert!(shown.status.success(), "{shown:?}");
    assert!(temporary.exists(), "history reads must not clean up files");
    let invalid = fixture.command(&["history", "cleanup", "extra"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(temporary.exists());

    let cleaned = fixture.command(&["history", "cleanup"]);
    assert!(cleaned.status.success(), "{cleaned:?}");
    assert_eq!(
        String::from_utf8(cleaned.stdout).unwrap().trim(),
        "panelink: uncommitted history files removed: 1"
    );
    assert!(!temporary.exists());
    assert_eq!(fs::read(unknown).unwrap(), b"keep");
    for (path, bytes) in committed {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    let repeated = fixture.command(&["history", "cleanup"]);
    assert!(repeated.status.success(), "{repeated:?}");
    assert!(String::from_utf8(repeated.stdout)
        .unwrap()
        .contains("removed: 0"));
    assert!(!fixture.0.join("server/control.sock").exists());
    assert!(!fixture.0.join("server.pid").exists());
}

#[test]
fn cleanup_cli_requires_the_server_lock_and_existing_identity_metadata() {
    let fixture = Fixture::new();
    let storage = ServerStorage::initialize(&fixture.0.join("server")).unwrap();
    fixture.create_history();
    let temporary = fixture.history().join(temporary_name(1));
    write_private(&temporary, b"partial");
    let denied = fixture.command(&["history", "cleanup"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(String::from_utf8(denied.stderr)
        .unwrap()
        .contains("stop the workspace"));
    assert!(temporary.exists());
    drop(storage);
    let maintenance = ServerStorage::lock_for_maintenance(&fixture.0.join("server")).unwrap();
    assert_eq!(
        fixture.command(&["history", "cleanup"]).status.code(),
        Some(1)
    );
    assert!(temporary.exists());
    drop(maintenance);
    fs::remove_file(fixture.0.join("server/generation")).unwrap();
    assert_eq!(
        fixture.command(&["history", "cleanup"]).status.code(),
        Some(1)
    );
    assert!(temporary.exists(), "missing metadata must fail closed");
}

#[test]
fn cleanup_cli_rejects_unsafe_candidates_without_deleting_other_files() {
    let fixture = Fixture::new();
    drop(ServerStorage::initialize(&fixture.0.join("server")).unwrap());
    fixture.create_history();
    let safe = fixture.history().join(temporary_name(1));
    write_private(&safe, b"partial");
    let sentinel = fixture.0.join("sentinel");
    write_private(&sentinel, b"untouched");
    let unsafe_path = fixture.history().join(temporary_name(2));
    std::os::unix::fs::symlink(&sentinel, &unsafe_path).unwrap();
    let denied = fixture.command(&["history", "cleanup"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(safe.exists());
    assert!(fs::symlink_metadata(unsafe_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
}
