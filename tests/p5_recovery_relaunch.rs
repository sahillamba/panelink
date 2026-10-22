#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::layout_store::LayoutSlot;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, PaneLayout, Rect, SplitDirection};
use panelink::recovery_links::{LinkOutcome, LinkStore};
use panelink::saved_layout::LayoutAxis;
use panelink::saved_layout::{encode, LayoutMetadata, SavedLayout};
use panelink::server_storage::ServerStorage;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);
const CANARY: &str = "RELAUNCH-SECRET-CANARY-7f04a2";

struct Fixture {
    root: PathBuf,
    shell: PathBuf,
    marker: PathBuf,
    client_cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-recovery-relaunch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let server = root.join("server");
        fs::DirBuilder::new().mode(0o700).create(&server).unwrap();
        let runtime = ServerStorage::initialize(&fs::canonicalize(&server).unwrap())
            .expect("initialize synthetic server storage");
        drop(runtime);
        let client_cwd = root.join("client-cwd");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&client_cwd)
            .unwrap();
        let marker = root.join("shell-starts");
        let shell = root.join("marker-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$PWD\" \"$RELAUNCH_TEST_CANARY\" \"$PANELINK_GENERATION\" >> '{}'\nexec /bin/sleep 300\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            shell,
            marker,
            client_cwd,
        }
    }

    fn server(&self) -> PathBuf {
        self.root.join("server")
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .args(arguments)
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", &self.shell)
            .env("RELAUNCH_TEST_CANARY", CANARY)
            .current_dir(&self.client_cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        run_limited(self.command(arguments), Duration::from_secs(20))
            .expect("panelink command completes within its fixture deadline")
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
        let mut record = encode(layout);
        if layout.pane_count() > 2 {
            record[53] = 3;
        }
        assert_eq!(
            panelink::saved_layout::decode(&record),
            Ok(*layout),
            "fixture must be a valid current-format PLLAY record"
        );
        file.write_all(&record).unwrap();
        file.sync_all().unwrap();
    }

    fn wait_for_shells(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let contents = fs::read_to_string(&self.marker).unwrap_or_default();
            if contents.lines().count() >= count {
                return;
            }
            if let Ok(log) = fs::read_to_string(self.root.join("server.log")) {
                if log.contains("listener_permission_denied") {
                    panic!("BLOCKED: pathname socket bind denied by environment");
                }
            }
            assert!(
                Instant::now() < deadline,
                "shell start marker count {count}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn assert_secret_absent(&self, output: &Output) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(CANARY), "secret leaked to stdout");
        assert!(!stderr.contains(CANARY), "secret leaked to stderr");
        let links = self.server().join("links/links.bin");
        if links.exists() {
            let bytes = fs::read(links).unwrap();
            assert!(!bytes
                .windows(CANARY.len())
                .any(|window| window == CANARY.as_bytes()));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = run_limited(self.command(&["stop"]), Duration::from_secs(8));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_limited(mut command: Command, timeout: Duration) -> Option<Output> {
    let mut child = command
        .spawn()
        .expect("spawn bounded panelink fixture command");
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll bounded child").is_some() {
            return Some(
                child
                    .wait_with_output()
                    .expect("collect bounded child output"),
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait_with_output();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn target(generation: u64, workspace: u64, pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation,
        workspace,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn metadata(generation: u64, sequence: u64) -> LayoutMetadata {
    LayoutMetadata::new(generation, 1, sequence, 1, 10_000 + sequence).unwrap()
}

fn one_pane(generation: u64, sequence: u64) -> SavedLayout {
    let rows = 24;
    let columns = 80;
    SavedLayout::from_snapshot(
        metadata(generation, sequence),
        &LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes: [
                Some(PaneGeometry {
                    target: target(generation, 1, 7, 9),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns,
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

fn two_pane(generation: u64, sequence: u64, axis: LayoutAxis) -> SavedLayout {
    let rows = 24;
    let columns = 80;
    let panes = match axis {
        LayoutAxis::SideBySide => [
            Some(PaneGeometry {
                target: target(generation, 1, 7, 9),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows,
                    columns: 39,
                },
            }),
            Some(PaneGeometry {
                target: target(generation, 1, 8, 10),
                rect: Rect {
                    x: 40,
                    y: 0,
                    rows,
                    columns: 40,
                },
            }),
            None,
            None,
        ],
        LayoutAxis::Stacked => [
            Some(PaneGeometry {
                target: target(generation, 1, 7, 9),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: 11,
                    columns,
                },
            }),
            Some(PaneGeometry {
                target: target(generation, 1, 8, 10),
                rect: Rect {
                    x: 0,
                    y: 12,
                    rows: 12,
                    columns,
                },
            }),
            None,
            None,
        ],
        LayoutAxis::Single => unreachable!(),
    };
    let divider = match axis {
        LayoutAxis::SideBySide => Rect {
            x: 39,
            y: 0,
            rows,
            columns: 1,
        },
        LayoutAxis::Stacked => Rect {
            x: 0,
            y: 11,
            rows: 1,
            columns,
        },
        LayoutAxis::Single => unreachable!(),
    };
    SavedLayout::from_snapshot(
        metadata(generation, sequence),
        &LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes,
            divider: Some(divider),
            dividers: [Some(divider), None, None],
        },
    )
    .unwrap()
}

/// Build a saved layout by replaying real splits through the production
/// `PaneLayout` model so the fixture is always a valid tree snapshot.
fn tree_layout(
    generation: u64,
    sequence: u64,
    splits: &[(usize, SplitDirection, u64, u64)],
) -> SavedLayout {
    let mut layout = PaneLayout::new(target(generation, 1, 7, 9), 24, 80).unwrap();
    for (source, direction, pane, session) in splits {
        let snapshot = layout.snapshot();
        let source = snapshot.panes[*source].unwrap().target;
        let preview = layout
            .preview_split(
                source,
                snapshot.revision,
                *direction,
                target(generation, 1, *pane, *session),
            )
            .unwrap();
        layout.commit_split(preview).unwrap();
    }
    let snapshot = layout.snapshot();
    let revision = snapshot.revision;
    let metadata =
        LayoutMetadata::new(generation, 1, sequence, revision, 10_000 + sequence).unwrap();
    let saved = SavedLayout::from_snapshot(metadata, &snapshot).unwrap();
    assert_eq!(panelink::saved_layout::decode(&encode(&saved)), Ok(saved));
    saved
}

/// Right split, then the left half split down: left column stacked, right full.
fn three_pane(generation: u64, sequence: u64) -> SavedLayout {
    tree_layout(
        generation,
        sequence,
        &[
            (0, SplitDirection::Right, 8, 10),
            (0, SplitDirection::Down, 9, 11),
        ],
    )
}

/// Right split, then each half split down: a 2x2 grid with T-junctions.
fn four_pane(generation: u64, sequence: u64) -> SavedLayout {
    let three = [
        (0, SplitDirection::Right, 8, 10),
        (0, SplitDirection::Down, 9, 11),
    ];
    // After the second split spatial order is [7 (top-left), 9 (bottom-left), 8 (right)].
    let mut splits = three.to_vec();
    splits.push((2, SplitDirection::Down, 10, 12));
    tree_layout(generation, sequence, &splits)
}

fn empty(generation: u64, sequence: u64) -> SavedLayout {
    SavedLayout::empty(metadata(generation, sequence))
}

type TreeEntry = (PathBuf, bool, u32, u64, i64, i64, Vec<u8>);

fn snapshot(root: &Path) -> Vec<TreeEntry> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<TreeEntry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        entries.push((
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
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn assert_no_server_start(fixture: &Fixture) {
    assert!(!fixture.server().join("control.sock").exists());
    assert!(!fixture.server().join("server.pid").exists());
    assert!(!fixture.marker.exists());
}

fn relaunch_and_assert_running(fixture: &Fixture, args: &[&str], panes: usize) -> Output {
    let output = match run_limited(fixture.command(args), Duration::from_secs(18)) {
        Some(output) => output,
        None => panic!("BLOCKED: relaunch command exceeded its strict fixture deadline"),
    };
    if let Ok(log) = fs::read_to_string(fixture.root.join("server.log")) {
        if log.contains("listener_permission_denied") {
            panic!("BLOCKED: pathname socket bind denied by environment");
        }
    }
    assert_eq!(
        output.status.code(),
        Some(0),
        "{output:?}; server log: {}",
        fs::read_to_string(fixture.root.join("server.log")).unwrap_or_default()
    );
    fixture.wait_for_shells(panes);
    fixture.assert_secret_absent(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("attach with: panelink"));
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("provider conversations are not resumed; pick them manually"));
    output
}

#[test]
fn missing_invalid_and_empty_slots_refuse_before_any_runtime_effect() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.root);
    let missing = fixture.run(&["recovery", "relaunch"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(missing.stderr, b"panelink: no saved layout\n");
    assert_eq!(snapshot(&fixture.root), before);
    assert_no_server_start(&fixture);

    fixture.write_layout(LayoutSlot::Current, &one_pane(4, 1));
    fs::write(fixture.server().join("layout/current.layout"), b"invalid").unwrap();
    let before = snapshot(&fixture.root);
    let invalid = fixture.run(&["recovery", "relaunch"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(invalid.stderr, b"panelink: saved layout invalid\n");
    assert_eq!(snapshot(&fixture.root), before);
    assert_no_server_start(&fixture);

    fs::write(
        fixture.server().join("layout/current.layout"),
        encode(&empty(5, 2)),
    )
    .unwrap();
    let before = snapshot(&fixture.root);
    let empty = fixture.run(&["recovery", "relaunch"]);
    assert_eq!(empty.status.code(), Some(1));
    assert_eq!(empty.stderr, b"panelink: saved layout has no panes\n");
    assert_eq!(snapshot(&fixture.root), before);
    assert_no_server_start(&fixture);
}

#[test]
fn usage_errors_exit_two_without_state_mutation() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.root);
    for args in [
        &["recovery", "relaunch", "extra"][..],
        &["recovery", "relaunch", "--previous", "--previous"][..],
        &["recovery", "relaunch", "--unknown"][..],
    ] {
        let output = fixture.run(args);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(snapshot(&fixture.root), before);
        assert_no_server_start(&fixture);
    }
}

#[test]
fn one_pane_relaunch_uses_fresh_context_and_records_a_new_generation_link() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &one_pane(41, 7));
    let output = relaunch_and_assert_running(&fixture, &["recovery", "relaunch"], 1);
    let summary = String::from_utf8_lossy(&output.stdout);
    assert!(summary.contains("pane 1: old 41:1:7:9:1 -> new "));
    assert!(summary.contains("(applied)"));
    let started = fs::read_to_string(&fixture.marker).unwrap();
    let [line] = started.lines().collect::<Vec<_>>()[..] else {
        panic!("one shell start expected: {started:?}");
    };
    assert!(line.starts_with(&format!("{}|{}|", fixture.client_cwd.display(), CANARY)));
    assert!(!line.contains("41|"));
    let pane_output = fixture.run(&["panes"]);
    assert_eq!(pane_output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&pane_output.stdout).starts_with("1 pane ("));
    let links = LinkStore::open(&fixture.server().join("links")).unwrap();
    let recorded = links.links_for_old(target(41, 1, 7, 9)).unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].source_generation, 41);
    assert_eq!(recorded[0].source_sequence, 7);
    assert_eq!(recorded[0].outcome, LinkOutcome::Applied);
    assert_ne!(recorded[0].new_target.generation, 41);
    // Parent review: the links file holds identities only, never env/cwd.
    for entry in fs::read_dir(fixture.server().join("links")).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(CANARY));
        assert!(!text.contains(&*fixture.client_cwd.to_string_lossy()));
    }
}

#[test]
fn two_pane_relaunch_preserves_both_saved_orientations_and_links_in_spatial_order() {
    for axis in [LayoutAxis::SideBySide, LayoutAxis::Stacked] {
        let fixture = Fixture::new();
        fixture.write_layout(LayoutSlot::Current, &two_pane(50, 9, axis));
        let output = relaunch_and_assert_running(&fixture, &["recovery", "relaunch"], 2);
        let summary = String::from_utf8_lossy(&output.stdout);
        assert!(summary.contains("pane 1: old 50:1:7:9:1 -> new "));
        assert!(summary.contains("pane 2: old 50:1:8:10:1 -> new "));
        assert_eq!(summary.matches("(applied)").count(), 2, "{summary}");
        let starts = fs::read_to_string(&fixture.marker).unwrap();
        assert_eq!(starts.lines().count(), 2, "{starts:?}");
        assert!(starts.lines().all(|line| {
            line.starts_with(&format!("{}|{}|", fixture.client_cwd.display(), CANARY))
        }));

        let pane_output = fixture.run(&["panes"]);
        assert_eq!(pane_output.status.code(), Some(0));
        let panes = String::from_utf8_lossy(&pane_output.stdout);
        assert_eq!(panes.matches("target ").count(), 2, "{panes}");
        match axis {
            LayoutAxis::SideBySide => assert!(panes.contains(" at col 40, row 0")),
            LayoutAxis::Stacked => assert!(panes.contains(" at col 0, row 12")),
            LayoutAxis::Single => unreachable!(),
        }

        let links = LinkStore::open(&fixture.server().join("links")).unwrap();
        let first = links.links_for_old(target(50, 1, 7, 9)).unwrap();
        let second = links.links_for_old(target(50, 1, 8, 10)).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].outcome, LinkOutcome::Applied);
        assert_eq!(second[0].outcome, LinkOutcome::Applied);
        assert_eq!(
            first[0].new_target.generation,
            second[0].new_target.generation
        );
    }
}

#[test]
fn three_pane_nested_layout_relaunches_and_links_in_spatial_order() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &three_pane(70, 13));
    let output = relaunch_and_assert_running(&fixture, &["recovery", "relaunch"], 3);
    let summary = String::from_utf8_lossy(&output.stdout);
    assert_eq!(summary.matches("(applied)").count(), 3, "{summary}");
    for (index, pane, session) in [(1, 7, 9), (2, 9, 11), (3, 8, 10)] {
        assert!(summary.contains(&format!(
            "pane {index}: old 70:1:{pane}:{session}:1 -> new "
        )));
    }
    let starts = fs::read_to_string(&fixture.marker).unwrap();
    assert_eq!(starts.lines().count(), 3, "{starts:?}");
    let pane_output = fixture.run(&["panes"]);
    assert_eq!(pane_output.status.code(), Some(0));
    let panes = String::from_utf8_lossy(&pane_output.stdout);
    assert_eq!(panes.matches("target ").count(), 3, "{panes}");

    let links = LinkStore::open(&fixture.server().join("links")).unwrap();
    let mut generation = None;
    for (pane, session) in [(7, 9), (8, 10), (9, 11)] {
        let records = links.links_for_old(target(70, 1, pane, session)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, LinkOutcome::Applied);
        match generation {
            Some(expected) => assert_eq!(records[0].new_target.generation, expected),
            None => generation = Some(records[0].new_target.generation),
        }
    }
}

#[test]
fn four_pane_nested_layout_relaunches_all_shells_and_links_in_spatial_order() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &four_pane(72, 14));
    let output = relaunch_and_assert_running(&fixture, &["recovery", "relaunch"], 4);
    let summary = String::from_utf8_lossy(&output.stdout);
    assert_eq!(summary.matches("(applied)").count(), 4, "{summary}");
    // Spatial order of the 2x2 grid: top-left, bottom-left, top-right, bottom-right.
    for (index, pane, session) in [(1, 7, 9), (2, 9, 11), (3, 8, 10), (4, 10, 12)] {
        assert!(
            summary.contains(&format!(
                "pane {index}: old 72:1:{pane}:{session}:1 -> new "
            )),
            "missing spatial pane {index}: {summary}"
        );
    }
    let starts = fs::read_to_string(&fixture.marker).unwrap();
    assert_eq!(starts.lines().count(), 4, "{starts:?}");
    assert!(starts.lines().all(|line| {
        line.starts_with(&format!("{}|{}|", fixture.client_cwd.display(), CANARY))
    }));

    let pane_output = fixture.run(&["panes"]);
    assert_eq!(pane_output.status.code(), Some(0));
    let panes = String::from_utf8_lossy(&pane_output.stdout);
    assert_eq!(panes.matches("target ").count(), 4, "{panes}");
    let positions = [
        "at col 0, row 0",
        "at col 0, row 12",
        "at col 40, row 0",
        "at col 40, row 12",
    ];
    let mut previous = 0;
    for position in positions {
        let index = panes.find(position).expect("pane appears in spatial order");
        assert!(index >= previous, "pane output order: {panes}");
        previous = index;
    }

    let links = LinkStore::open(&fixture.server().join("links")).unwrap();
    let mut generation = None;
    for (pane, session) in [(7, 9), (9, 11), (10, 12), (8, 10)] {
        let records = links.links_for_old(target(72, 1, pane, session)).unwrap();
        assert_eq!(records.len(), 1, "old pane {pane}");
        assert_eq!(records[0].outcome, LinkOutcome::Applied);
        assert_eq!(records[0].source_generation, 72);
        assert_eq!(records[0].source_sequence, 14);
        assert_eq!(records[0].new_target.workspace, 1);
        match generation {
            Some(expected) => assert_eq!(records[0].new_target.generation, expected),
            None => generation = Some(records[0].new_target.generation),
        }
    }
}

#[test]
fn previous_slot_is_selected_and_running_workspace_is_refused_untouched() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Previous, &one_pane(61, 4));
    fixture.write_layout(LayoutSlot::Current, &one_pane(62, 5));
    let output = relaunch_and_assert_running(&fixture, &["recovery", "relaunch", "--previous"], 1);
    assert!(String::from_utf8_lossy(&output.stdout).contains("old 61:1:7:9:1"));

    let before = snapshot(&fixture.root);
    let refused = fixture.run(&["recovery", "relaunch"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        refused.stderr,
        b"panelink: stop the running workspace before relaunching (panelink stop)\n"
    );
    assert_eq!(snapshot(&fixture.root), before);
    fixture.assert_secret_absent(&refused);
}
