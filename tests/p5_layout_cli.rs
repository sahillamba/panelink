#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::layout_store::LayoutStore;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::saved_layout::{LayoutMetadata, SavedLayout};
use panelink::server_storage::ServerStorage;
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

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-layout-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
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
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn server(&self) -> PathBuf {
        self.root.join("server")
    }

    fn layout(&self) -> PathBuf {
        self.server().join("layout")
    }

    /// Initialize server storage and return the store.
    fn with_storage(&self) -> ServerStorage {
        ServerStorage::initialize(&self.server()).unwrap()
    }

    /// Prepare the layout store (creates layout directory if absent).
    fn with_layout_store(&self) -> LayoutStore {
        LayoutStore::prepare(&self.server()).unwrap()
    }

    fn command(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", self.root.join("forbidden-shell"))
            .current_dir(&self.root)
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
        assert!(!timed_out, "layout CLI exceeded deadline: {args:?}");
        output
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

fn make_metadata(generation: u64, workspace: u64, sequence: u64, revision: u64) -> LayoutMetadata {
    LayoutMetadata::new(generation, workspace, sequence, revision, 1_000_001)
        .expect("valid test metadata")
}

fn saved_layout_one(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
    let meta = make_metadata(generation, workspace, sequence, revision);
    let snapshot = LayoutSnapshot {
        revision,
        rows: 10,
        columns: 30,
        panes: [
            Some(PaneGeometry {
                target: target_for(generation, workspace, 1, 1),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: 10,
                    columns: 30,
                },
            }),
            None,
        ],
        divider: None,
    };
    SavedLayout::from_snapshot(meta, &snapshot).expect("valid one-pane saved layout")
}

fn saved_layout_side_by_side(
    generation: u64,
    workspace: u64,
    sequence: u64,
    revision: u64,
) -> SavedLayout {
    let meta = make_metadata(generation, workspace, sequence, revision);
    let (rows, columns) = (7, 82);
    let usable = columns - 1;
    let first_cols = usable / 2;
    let second_cols = usable - first_cols;
    let divider_x = first_cols;
    let second_x = divider_x + 1;
    let snapshot = LayoutSnapshot {
        revision,
        rows,
        columns,
        panes: [
            Some(PaneGeometry {
                target: target_for(generation, workspace, 1, 1),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows,
                    columns: first_cols,
                },
            }),
            Some(PaneGeometry {
                target: target_for(generation, workspace, 2, 2),
                rect: Rect {
                    x: second_x,
                    y: 0,
                    rows,
                    columns: second_cols,
                },
            }),
        ],
        divider: Some(Rect {
            x: divider_x,
            y: 0,
            rows,
            columns: 1,
        }),
    };
    SavedLayout::from_snapshot(meta, &snapshot).expect("valid side-by-side saved layout")
}

fn saved_layout_stacked(
    generation: u64,
    workspace: u64,
    sequence: u64,
    revision: u64,
) -> SavedLayout {
    let meta = make_metadata(generation, workspace, sequence, revision);
    let (rows, columns) = (11, 82);
    let usable = rows - 1;
    let first_rows = usable / 2;
    let second_rows = usable - first_rows;
    let divider_y = first_rows;
    let second_y = divider_y + 1;
    let snapshot = LayoutSnapshot {
        revision,
        rows,
        columns,
        panes: [
            Some(PaneGeometry {
                target: target_for(generation, workspace, 1, 1),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: first_rows,
                    columns,
                },
            }),
            Some(PaneGeometry {
                target: target_for(generation, workspace, 2, 2),
                rect: Rect {
                    x: 0,
                    y: second_y,
                    rows: second_rows,
                    columns,
                },
            }),
        ],
        divider: Some(Rect {
            x: 0,
            y: divider_y,
            rows: 1,
            columns,
        }),
    };
    SavedLayout::from_snapshot(meta, &snapshot).expect("valid stacked saved layout")
}

fn saved_layout_empty(
    generation: u64,
    workspace: u64,
    sequence: u64,
    revision: u64,
) -> SavedLayout {
    let meta = make_metadata(generation, workspace, sequence, revision);
    SavedLayout::empty(meta)
}

/// Ignore atime: opening a read-only file can legitimately update it.
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

fn assert_exit1_no_saved_layout(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "no partial stdout: {output:?}");
    assert_eq!(output.stderr, b"panelink: no saved layout\n");
}

fn assert_exit1_invalid(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "no partial stdout: {output:?}");
    assert_eq!(
        output.stderr, b"panelink: saved layout invalid\n",
        "{output:?}"
    );
}

fn assert_usage_error(output: &Output) {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "no stdout on usage: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("error:"),
        "expected usage error: {output:?}"
    );
}

#[test]
fn absent_root_server_or_layout_reports_no_saved_layout_without_mutation() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.root);
    assert_exit1_no_saved_layout(&fixture.command(&["layout", "show"]));
    assert_eq!(snapshot(&fixture.root), before);
    assert!(!fixture.root.join("child-started").exists());
}

#[test]
fn absent_layout_with_pre_initialized_storage_leaves_identity_unchanged() {
    let fixture = Fixture::new();
    let _storage = fixture.with_storage();
    let before = snapshot(&fixture.root);
    assert_exit1_no_saved_layout(&fixture.command(&["layout", "show"]));
    assert_eq!(snapshot(&fixture.root), before);
    assert!(!fixture.root.join("child-started").exists());
}

#[test]
fn valid_one_pane_current_exact_output() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let layout = saved_layout_one(7, 1, 1, 1);
    store.save(7, 1, &layout).unwrap();

    let output = fixture.command(&["layout", "show"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        concat!(
            "saved layout: current\n",
            "generation: 7\n",
            "workspace: 1\n",
            "snapshot sequence: 1\n",
            "layout revision: 1\n",
            "observed at unix ms: 1000001\n",
            "size: 10x30\n",
            "orientation: single\n",
            "pane 1: 7:1:1:1:1\n",
            "history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n",
        )
    );

    // --previous absent despite current present
    assert_exit1_no_saved_layout(&fixture.command(&["layout", "show", "--previous"]));
    assert!(!fixture.root.join("child-started").exists());
}

#[test]
fn valid_two_pane_side_by_side_exact_output() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let layout = saved_layout_side_by_side(7, 1, 2, 3);
    store.save(7, 1, &layout).unwrap();

    let output = fixture.command(&["layout", "show"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        concat!(
            "saved layout: current\n",
            "generation: 7\n",
            "workspace: 1\n",
            "snapshot sequence: 2\n",
            "layout revision: 3\n",
            "observed at unix ms: 1000001\n",
            "size: 7x82\n",
            "orientation: side-by-side\n",
            "pane 1: 7:1:1:1:1\n",
            "pane 2: 7:1:2:2:1\n",
            "history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n",
        )
    );
}

#[test]
fn valid_two_pane_stacked_exact_output() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let layout = saved_layout_stacked(8, 2, 1, 1);
    store.save(8, 2, &layout).unwrap();

    let output = fixture.command(&["layout", "show"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        concat!(
            "saved layout: current\n",
            "generation: 8\n",
            "workspace: 2\n",
            "snapshot sequence: 1\n",
            "layout revision: 1\n",
            "observed at unix ms: 1000001\n",
            "size: 11x82\n",
            "orientation: stacked\n",
            "pane 1: 8:2:1:1:1\n",
            "pane 2: 8:2:2:2:1\n",
            "history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n",
        )
    );
}

#[test]
fn empty_workspace_snapshot_exact_output() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let layout = saved_layout_empty(7, 1, 5, 10);
    store.save(7, 1, &layout).unwrap();

    let output = fixture.command(&["layout", "show"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        concat!(
            "saved layout: current\n",
            "generation: 7\n",
            "workspace: 1\n",
            "snapshot sequence: 5\n",
            "layout revision: 10\n",
            "observed at unix ms: 1000001\n",
            "panes: none (empty workspace snapshot)\n",
            "history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n",
        )
    );
}

#[test]
fn rotation_previous_shows_older_generation() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let gen1 = saved_layout_one(7, 1, 1, 1);
    store.save(7, 1, &gen1).unwrap();
    let gen2 = saved_layout_one(8, 1, 1, 1);
    store.save(8, 1, &gen2).unwrap();

    let current = fixture.command(&["layout", "show"]);
    assert!(current.status.success(), "{current:?}");
    let stdout = String::from_utf8_lossy(&current.stdout);
    assert!(stdout.contains("generation: 8\n"), "{stdout}");

    let previous = fixture.command(&["layout", "show", "--previous"]);
    assert!(previous.status.success(), "{previous:?}");
    let prev_stdout = String::from_utf8_lossy(&previous.stdout);
    assert_eq!(
        prev_stdout,
        concat!(
            "saved layout: previous\n",
            "generation: 7\n",
            "workspace: 1\n",
            "snapshot sequence: 1\n",
            "layout revision: 1\n",
            "observed at unix ms: 1000001\n",
            "size: 10x30\n",
            "orientation: single\n",
            "pane 1: 7:1:1:1:1\n",
            "history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n",
        )
    );
}

#[test]
fn invalid_current_reports_invalid_but_previous_still_readable() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let gen1 = saved_layout_one(7, 1, 1, 1);
    store.save(7, 1, &gen1).unwrap();
    let gen2 = saved_layout_one(8, 1, 1, 1);
    store.save(8, 1, &gen2).unwrap();

    // Corrupt current.layout directly (previous remains valid gen1)
    let current_path = fixture.layout().join("current.layout");
    fs::write(&current_path, b"not-valid-layout-record-here").unwrap();
    fs::set_permissions(&current_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert_exit1_invalid(&fixture.command(&["layout", "show"]));

    let previous = fixture.command(&["layout", "show", "--previous"]);
    assert!(previous.status.success(), "{previous:?}");
    let prev_stdout = String::from_utf8_lossy(&previous.stdout);
    assert!(prev_stdout.contains("generation: 7\n"), "{prev_stdout}");
}

#[test]
fn read_preserves_bytes_mode_mtime_and_directory_listing() {
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    let layout = saved_layout_side_by_side(7, 1, 1, 1);
    store.save(7, 1, &layout).unwrap();

    let before = snapshot(&fixture.root);
    let output = fixture.command(&["layout", "show"]);
    assert!(output.status.success(), "{output:?}");
    let after = snapshot(&fixture.root);
    assert_eq!(before, after);
}

#[test]
fn usage_errors_return_exit_two() {
    let fixture = Fixture::new();
    for args in [
        vec!["layout"],
        vec!["layout", "show", "extra"],
        vec!["layout", "show", "--previous", "--previous"],
        vec!["layout", "list"],
    ] {
        let output = fixture.command(&args);
        assert_usage_error(&output);
        assert!(!fixture.root.join("child-started").exists());
    }
}

#[test]
fn unsafe_layout_directory_is_invalid_not_absent() {
    // Parent review regression: a public (0755) layout directory must be
    // reported as invalid, not silently treated as "no saved layout".
    let fixture = Fixture::new();
    let store = fixture.with_layout_store();
    store.save(7, 1, &saved_layout_one(7, 1, 1, 1)).unwrap();
    drop(store);
    fs::set_permissions(fixture.layout(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_exit1_invalid(&fixture.command(&["layout", "show"]));
    fs::set_permissions(fixture.layout(), fs::Permissions::from_mode(0o700)).unwrap();
}
