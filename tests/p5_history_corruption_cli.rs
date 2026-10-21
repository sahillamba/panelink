#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
const TARGET: &str = "7:1:2:3:1";
const HEALTHY: &str = "7:1:2:4:1";
const CANARY: &[u8] = b"\x1b]52;c;CORRUPTION-CANARY\x07";

struct Fixture(PathBuf);

impl Fixture {
    fn new(with_identity: bool) -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-corruption-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
            .unwrap();
        if with_identity {
            drop(ServerStorage::initialize(&root.join("server")).unwrap());
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server/history"))
            .unwrap();
        let shell = root.join("forbidden-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf started > '{}/child-started'\nexit 0\n",
                root.display()
            ),
        )
        .unwrap();
        fs::set_permissions(shell, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self(root);
        let store = HistoryStore::open(&fixture.history()).unwrap();
        for session in [3, 4] {
            let target = target(session);
            let mut parser = vt100::Parser::new(2, 8, 8);
            let cursor = parser.screen().history_cursor().unwrap();
            parser.process(b"SAFE-0\r\nSAFE-1\r\nSAFE-2\r\nSAFE-3\r\n");
            let rows = parser
                .screen()
                .history_after(cursor)
                .unwrap()
                .map(|row| history_rows::encode(target, 1, row, 123).unwrap())
                .collect::<Vec<_>>();
            assert!(!rows.is_empty());
            store.append_rows(target, &rows, false).unwrap();
            let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
            let normal = screen_checkpoint::encode(
                &snapshot,
                &CheckpointMetadata::new(target, 1, 123, 1).unwrap(),
            )
            .unwrap();
            store.save(target, ScreenBuffer::Normal, &normal).unwrap();
            // Fixture-only buffer tag change, validated by both codec and store.
            let mut alternate = normal;
            alternate[80] = 1;
            assert_eq!(
                screen_checkpoint::decode(&alternate).unwrap().buffer(),
                ScreenBuffer::Alternate
            );
            store
                .save(target, ScreenBuffer::Alternate, &alternate)
                .unwrap();
        }
        fixture
    }

    fn history(&self) -> PathBuf {
        self.0.join("server/history")
    }

    fn file(&self, suffix: &str) -> PathBuf {
        self.history().join(format!(
            "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}{suffix}",
            7, 1, 2, 3, 1
        ))
    }

    fn command(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PANELINK_HOME", &self.0)
            .env("HOME", &self.0)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", self.0.join("forbidden-shell"))
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = i32::try_from(child.id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let timed_out = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if Instant::now() >= deadline {
                break true;
            }
            thread::sleep(Duration::from_millis(5));
        };
        // SAFETY: negative PID names only this test-owned isolated group.
        unsafe { libc::kill(-group, libc::SIGKILL) };
        let output = child.wait_with_output().unwrap();
        assert!(!timed_out, "history CLI exceeded deadline: {args:?}");
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target(session: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane: 2,
        session,
        occupant_revision: 1,
    }
}

// Ignore atime: opening a read-only file can legitimately update it.
type Entry = (bool, u32, i64, i64, Vec<u8>);
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        entries.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            (
                metadata.is_dir(),
                metadata.mode(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                if metadata.is_file() {
                    fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            ),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn assert_refused(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "no partial prefix or healthy component output"
    );
    assert_eq!(output.stderr, b"panelink: saved history invalid\n");
}

#[test]
fn fresh_readers_refuse_damaged_components_without_partial_output_or_mutation() {
    for (suffix, damage) in [
        (".rows", 0),
        (".rows", 1),
        (".rows", 2),
        ("-n.screen", 0),
        ("-a.screen", 0),
        ("-a.screen", 2),
    ] {
        let fixture = Fixture::new(false);
        let path = fixture.file(suffix);
        let mut bytes = fs::read(&path).unwrap();
        match damage {
            0 => {
                bytes.pop();
            }
            1 => bytes.clear(),
            _ => bytes.extend_from_slice(CANARY),
        }
        fs::write(&path, &bytes).unwrap();
        let before = snapshot(&fixture.0);
        for args in [
            vec!["history", "show", TARGET],
            vec!["history", "search", TARGET, "SAFE"],
        ] {
            assert_refused(&fixture.command(&args));
            assert_eq!(snapshot(&fixture.0), before);
        }
        for args in [
            vec!["history", "show", HEALTHY],
            vec!["history", "search", HEALTHY, "SAFE"],
        ] {
            let output = fixture.command(&args);
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("SAFE"));
            assert!(!output.stdout.contains(&0x1b));
            assert!(output.stderr.is_empty());
            assert_eq!(snapshot(&fixture.0), before);
        }
        assert!(!fixture.0.join("child-started").exists());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn valid_orphan_never_repairs_invalid_committed_destination() {
    let fixture = Fixture::new(true);
    let path = fixture.file(".rows");
    let valid = fs::read(&path).unwrap();
    let temporary = fixture.history().join(format!(
        ".{}-123-1.tmp",
        path.file_name().unwrap().to_str().unwrap()
    ));
    fs::write(&temporary, &valid).unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, &valid[..valid.len() - 1]).unwrap();
    let before = snapshot(&fixture.0);
    assert_refused(&fixture.command(&["history", "show", TARGET]));
    assert_eq!(snapshot(&fixture.0), before);
    let guard = ServerStorage::lock_for_maintenance(&fixture.0.join("server")).unwrap();
    let store = HistoryStore::open(&fixture.history()).unwrap();
    assert_eq!(store.cleanup_temporary_files().unwrap(), 1);
    assert!(!temporary.exists());
    assert_eq!(fs::read(&path).unwrap(), &valid[..valid.len() - 1]);
    assert!(store.load_rows(target(3)).is_err());
    assert!(store.load_rows(target(4)).unwrap().is_some());
    drop(store);
    drop(guard);
    assert_refused(&fixture.command(&["history", "search", TARGET, "SAFE"]));
}
