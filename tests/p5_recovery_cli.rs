#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::layout_store::LayoutSlot;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::saved_layout::{encode, LayoutMetadata, SavedLayout};
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-recovery-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
            .unwrap();
        let marker = root.join("marker-ran");
        let shell = root.join("marker-shell");
        fs::write(
            &shell,
            format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn server(&self) -> PathBuf {
        self.root.join("server")
    }

    fn write_layout(&self, slot: LayoutSlot, layout: &SavedLayout) {
        let directory = self.server().join("layout");
        if !directory.exists() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
        }
        let name = match slot {
            LayoutSlot::Current => "current.layout",
            LayoutSlot::Previous => "previous.layout",
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(directory.join(name))
            .unwrap();
        file.write_all(&encode(layout)).unwrap();
        file.sync_all().unwrap();
    }

    fn history(&self) -> HistoryStore {
        let directory = self.server().join("history");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        HistoryStore::open(&fs::canonicalize(directory).unwrap()).unwrap()
    }

    fn command(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(arguments)
            .env_clear()
            .env("PANELINK_HOME", self.root())
            .env("HOME", self.root())
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", self.root.join("marker-shell"))
            .current_dir(self.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    }

    fn assert_no_runtime_effects(&self) {
        assert!(!self.server().join("server.lock").exists());
        assert!(!self.server().join("control.sock").exists());
        assert!(!self.server().join("server.pid").exists());
        assert!(!self.root.join("marker-ran").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

type TreeEntry = (PathBuf, bool, u32, u64, i64, i64, Vec<u8>);

fn snapshot(root: &Path) -> Vec<TreeEntry> {
    fn visit(root: &Path, path: &Path, output: &mut Vec<TreeEntry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        output.push((
            path.strip_prefix(root).unwrap().to_path_buf(),
            metadata.is_dir(),
            metadata.mode(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            if metadata.is_file() {
                fs::read(path).unwrap()
            } else {
                Vec::new()
            },
        ));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), output);
            }
        }
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn target(generation: u64, pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation,
        workspace: 2,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn metadata(generation: u64, sequence: u64, revision: u64) -> LayoutMetadata {
    LayoutMetadata::new(generation, 2, sequence, revision, 1_000_000 + sequence).unwrap()
}

fn one_pane(generation: u64, sequence: u64) -> SavedLayout {
    SavedLayout::from_snapshot(
        metadata(generation, sequence, 1),
        &LayoutSnapshot {
            revision: 1,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: target(generation, 1, 10),
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
            dividers: [None; panelink::pane_layout::MAX_PANES - 1],
        },
    )
    .unwrap()
}

fn two_pane(generation: u64, sequence: u64) -> SavedLayout {
    let rows = 10;
    let columns = 82;
    SavedLayout::from_snapshot(
        metadata(generation, sequence, 1),
        &LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes: [
                Some(PaneGeometry {
                    target: target(generation, 1, 10),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns: 40,
                    },
                }),
                Some(PaneGeometry {
                    target: target(generation, 2, 11),
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows,
                        columns: 41,
                    },
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 40,
                y: 0,
                rows,
                columns: 1,
            }),
            dividers: [
                Some(Rect {
                    x: 40,
                    y: 0,
                    rows,
                    columns: 1,
                }),
                None,
                None,
            ],
        },
    )
    .unwrap()
}

fn empty(generation: u64, sequence: u64) -> SavedLayout {
    SavedLayout::empty(metadata(generation, sequence, 1))
}

fn checkpoint(target: TargetRef, buffer: ScreenBuffer) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(10, 30).unwrap()).unwrap();
    let mut record = screen_checkpoint::encode(
        &snapshot,
        &CheckpointMetadata::new(target, 3, 4, 5).unwrap(),
    )
    .unwrap();
    record[80] = u8::from(buffer == ScreenBuffer::Alternate);
    record
}

fn history_row(target: TargetRef) -> Vec<u8> {
    let mut parser = vt100::Parser::new(2, 30, 1024);
    let cursor = parser.screen().history_cursor().unwrap();
    parser.process(b"saved-line\r\nnext-line\r\n");
    let row = parser
        .screen()
        .history_after(cursor)
        .unwrap()
        .next()
        .expect("one synthetic history row");
    history_rows::encode(target, 1, row, 1234).unwrap()
}

#[test]
fn absent_layout_fails_without_mutating_the_tree_or_starting_a_shell() {
    let fixture = Fixture::new();
    let before = snapshot(fixture.root());
    let output = fixture.command(&["recovery", "view"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"panelink: no saved layout\n");
    assert_eq!(snapshot(fixture.root()), before);
    fixture.assert_no_runtime_effects();
}

#[test]
fn current_previous_two_pane_one_pane_and_empty_summaries_are_explicit() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Previous, &one_pane(7, 12));
    fixture.write_layout(LayoutSlot::Current, &two_pane(8, 15));

    let current = fixture.command(&["recovery", "view"]);
    assert_eq!(current.status.code(), Some(0));
    let current = String::from_utf8(current.stdout).unwrap();
    assert!(current.contains("slot: current\ngeneration: 8\nsequence: 15"));
    assert!(current.contains("size: 10x82\norientation: SideBySide"));
    assert!(current
        .contains("pane 1: target 8:2:1:10:1; rows missing; normal missing; alternate missing"));
    assert!(current
        .contains("pane 2: target 8:2:2:11:1; rows missing; normal missing; alternate missing"));

    let previous = fixture.command(&["recovery", "view", "--previous"]);
    assert_eq!(previous.status.code(), Some(0));
    let previous = String::from_utf8(previous.stdout).unwrap();
    assert!(previous.contains("slot: previous\ngeneration: 7\nsequence: 12"));
    assert!(previous.contains("pane 1: target 7:2:1:10:1"));
    assert!(!previous.contains("pane 2:"));

    fixture.write_layout(LayoutSlot::Previous, &empty(9, 18));
    let empty = fixture.command(&["recovery", "view", "--previous"]);
    assert_eq!(empty.status.code(), Some(0));
    assert!(String::from_utf8(empty.stdout)
        .unwrap()
        .contains("workspace: empty"));
    fixture.assert_no_runtime_effects();
}

#[test]
fn invalid_current_does_not_fall_back_and_previous_remains_readable() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Previous, &one_pane(7, 10));
    fixture.write_layout(LayoutSlot::Current, &one_pane(8, 11));
    fs::write(
        fixture.server().join("layout/current.layout"),
        b"invalid-current",
    )
    .unwrap();

    let current = fixture.command(&["recovery", "view"]);
    assert_eq!(current.status.code(), Some(1));
    assert_eq!(current.stderr, b"panelink: saved layout invalid\n");
    let previous = fixture.command(&["recovery", "view", "--previous"]);
    assert_eq!(previous.status.code(), Some(0));
    assert!(String::from_utf8(previous.stdout)
        .unwrap()
        .contains("slot: previous\ngeneration: 7"));
    fixture.assert_no_runtime_effects();
}

#[test]
fn history_availability_is_per_pane_and_invalid_committed_rows_are_reported() {
    let fixture = Fixture::new();
    let layout = two_pane(8, 15);
    fixture.write_layout(LayoutSlot::Current, &layout);
    let store = fixture.history();
    let first = target(8, 1, 10);
    let second = target(8, 2, 11);
    store
        .append_rows(first, &[history_row(first)], false)
        .unwrap();
    store
        .save(
            first,
            ScreenBuffer::Normal,
            &checkpoint(first, ScreenBuffer::Normal),
        )
        .unwrap();
    store
        .save(
            second,
            ScreenBuffer::Alternate,
            &checkpoint(second, ScreenBuffer::Alternate),
        )
        .unwrap();

    let output = fixture.command(&["recovery", "view"]);
    assert_eq!(output.status.code(), Some(0));
    let summary = String::from_utf8(output.stdout).unwrap();
    assert!(summary.contains(
        "pane 1: target 8:2:1:10:1; rows available; normal available; alternate missing"
    ));
    assert!(summary
        .contains("pane 2: target 8:2:2:11:1; rows missing; normal missing; alternate available"));

    let rows_path = fs::read_dir(fixture.server().join("history"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "rows")
        })
        .unwrap();
    fs::write(rows_path, b"invalid-row-record").unwrap();
    let output = fixture.command(&["recovery", "view"]);
    assert_eq!(output.status.code(), Some(0));
    let summary = String::from_utf8(output.stdout).unwrap();
    assert!(summary
        .contains("pane 1: target 8:2:1:10:1; rows invalid; normal available; alternate missing"));
    assert!(summary
        .contains("pane 2: target 8:2:2:11:1; rows missing; normal missing; alternate available"));
    fixture.assert_no_runtime_effects();
}

#[test]
fn recovery_usage_errors_exit_two_without_touching_the_state_root() {
    let fixture = Fixture::new();
    let before = snapshot(fixture.root());
    for arguments in [
        &["recovery"][..],
        &["recovery", "view", "x"][..],
        &["recovery", "view", "--previous", "--previous"][..],
        // `recovery relaunch` became a real command; extra args stay usage errors.
        &["recovery", "relaunch", "x"][..],
        &["recovery", "relaunch", "--previous", "--previous"][..],
    ] {
        let output = fixture.command(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert_eq!(output.stdout, b"", "{arguments:?}");
        assert_eq!(
            output.stderr,
            b"error: invalid command syntax; try 'panelink --help'\n"
        );
        assert_eq!(snapshot(fixture.root()), before);
        fixture.assert_no_runtime_effects();
    }
}
