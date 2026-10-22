#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::layout_store::LayoutSlot;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::recovery_reader::{read_recovery, RecoveryReadError};
use panelink::recovery_view::{
    CheckpointAvailability, HistoryAvailability, RecoverySlot, ResetStatus,
};
use panelink::saved_layout::{encode, LayoutMetadata, SavedLayout};
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/panelink-recovery-reader-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path.join("layout"))
            .unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_layout(&self, slot: LayoutSlot, layout: &SavedLayout) {
        let name = match slot {
            LayoutSlot::Current => "current.layout",
            LayoutSlot::Previous => "previous.layout",
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.path().join("layout").join(name))
            .unwrap();
        file.write_all(&encode(layout)).unwrap();
        file.sync_all().unwrap();
    }

    fn create_history(&self) -> HistoryStore {
        let path = self.path().join("history");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        HistoryStore::open(&fs::canonicalize(path).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target(pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 8,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn layout(generation: u64, session: u64, pane_count: usize) -> SavedLayout {
    let metadata = LayoutMetadata::new(generation, 8, 1, 1, 10).unwrap();
    if pane_count == 0 {
        return SavedLayout::empty(metadata);
    }
    let first_target = TargetRef {
        generation,
        ..target(2, session)
    };
    let (panes, divider) = if pane_count == 1 {
        (
            [
                Some(PaneGeometry {
                    target: first_target,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 10,
                        columns: 30,
                    },
                }),
                None,
            ],
            None,
        )
    } else {
        (
            [
                Some(PaneGeometry {
                    target: first_target,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 10,
                        columns: 40,
                    },
                }),
                Some(PaneGeometry {
                    target: TargetRef {
                        generation,
                        ..target(4, session + 1)
                    },
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows: 10,
                        columns: 41,
                    },
                }),
            ],
            Some(Rect {
                x: 40,
                y: 0,
                rows: 10,
                columns: 1,
            }),
        )
    };
    SavedLayout::from_snapshot(
        metadata,
        &LayoutSnapshot {
            revision: 1,
            rows: 10,
            columns: if pane_count == 1 { 30 } else { 82 },
            panes,
            divider,
        },
    )
    .unwrap()
}

fn row_records(target: TargetRef, count: usize) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(2, 20, 1024);
    let cursor = parser.screen().history_cursor().unwrap();
    for index in 0..count + 1 {
        parser.process(format!("row-{index}\r\n").as_bytes());
    }
    parser
        .screen()
        .history_after(cursor)
        .unwrap()
        .map(|row| history_rows::encode(target, 4, row, 1234 + row.ordinal).unwrap())
        .collect()
}

fn checkpoint(target: TargetRef, buffer: ScreenBuffer, sequence: u64) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
    let mut record = screen_checkpoint::encode(
        &snapshot,
        &CheckpointMetadata::new(target, sequence, 99 + sequence, sequence + 1).unwrap(),
    )
    .unwrap();
    // The validated v1 header's buffer tag is fixed at byte 80.
    record[80] = u8::from(buffer == ScreenBuffer::Alternate);
    assert_eq!(screen_checkpoint::decode(&record).unwrap().buffer(), buffer);
    record
}

type TreeEntry = (PathBuf, bool, u32, u64, i64, i64, Vec<u8>);

fn snapshot_tree(root: &Path) -> Vec<TreeEntry> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<TreeEntry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
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

#[test]
fn empty_or_absent_storage_is_not_created_and_slot_selection_does_not_fallback() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Previous, &layout(6, 10, 1));
    let before = snapshot_tree(fixture.path());

    assert!(matches!(
        read_recovery(fixture.path(), RecoverySlot::Current),
        Err(RecoveryReadError::MissingLayout)
    ));
    let previous = read_recovery(fixture.path(), RecoverySlot::Previous).unwrap();
    assert_eq!(previous.slot, RecoverySlot::Previous);
    assert_eq!(previous.metadata.generation(), 6);
    assert!(!fixture.path().join("history").exists());

    let after = snapshot_tree(fixture.path());
    assert_eq!(before, after);
}

#[test]
fn reads_exact_target_rows_screens_gap_epoch_and_timestamps_without_mutation() {
    let fixture = Fixture::new();
    let saved_layout = layout(7, 20, 2);
    fixture.write_layout(LayoutSlot::Current, &saved_layout);
    let store = fixture.create_history();
    let first = target(2, 20);
    let second = target(4, 21);
    store
        .append_rows(first, &row_records(first, 2), true)
        .unwrap();
    store
        .save(
            first,
            ScreenBuffer::Normal,
            &checkpoint(first, ScreenBuffer::Normal, 3),
        )
        .unwrap();
    store
        .save(
            first,
            ScreenBuffer::Alternate,
            &checkpoint(first, ScreenBuffer::Alternate, 4),
        )
        .unwrap();
    store
        .save(
            second,
            ScreenBuffer::Normal,
            &checkpoint(second, ScreenBuffer::Normal, 8),
        )
        .unwrap();
    drop(store);

    let before = snapshot_tree(fixture.path());

    let view = read_recovery(fixture.path(), RecoverySlot::Current).unwrap();
    let HistoryAvailability::Available {
        rows,
        epoch,
        gap,
        reset,
        ..
    } = &view.panes[0].rows
    else {
        panic!("expected first pane rows")
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ordinal, 1);
    assert!(rows[0].captured_at_unix_ms > 1234);
    assert!((0..rows[0].width).all(|column| rows[0]
        .cell_text(column)
        .is_some_and(|cell| cell.chars().all(|character| !character.is_control()))));
    assert_eq!(*epoch, 4);
    assert!(*gap);
    assert_eq!(*reset, ResetStatus::PossibleFromGap);
    assert!(matches!(
        view.panes[0].normal,
        CheckpointAvailability::Available { info, .. }
            if info.output_sequence == 3
                && info.captured_at_unix_ms == 102
                && info.screen_revision == 4
                && (info.rows, info.columns) == (8, 20)
    ));
    let normal = view.panes[0].normal.checkpoint().unwrap();
    assert_eq!(normal.buffer(), ScreenBuffer::Normal);
    assert_eq!((normal.rows(), normal.columns()), (8, 20));
    assert!(matches!(
        view.panes[0].alternate,
        CheckpointAvailability::Available { info, .. } if info.buffer == ScreenBuffer::Alternate
    ));
    assert!(matches!(view.panes[1].rows, HistoryAvailability::Missing));
    assert!(matches!(
        view.panes[1].normal,
        CheckpointAvailability::Available { info, .. } if info.output_sequence == 8
    ));
    assert_eq!(before, snapshot_tree(fixture.path()));
}

#[test]
fn corrupt_current_is_invalid_and_does_not_hide_valid_previous() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Previous, &layout(6, 10, 1));
    fixture.write_layout(LayoutSlot::Current, &layout(7, 20, 1));
    fs::write(fixture.path().join("layout/current.layout"), b"bad record").unwrap();
    let before = snapshot_tree(fixture.path());

    assert!(matches!(
        read_recovery(fixture.path(), RecoverySlot::Current),
        Err(RecoveryReadError::InvalidLayout)
    ));
    let previous = read_recovery(fixture.path(), RecoverySlot::Previous).unwrap();
    assert_eq!(previous.metadata.generation(), 6);
    assert_eq!(snapshot_tree(fixture.path()), before);
}

#[test]
fn missing_corrupt_row_and_each_checkpoint_are_independent_sources() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &layout(7, 20, 2));
    let store = fixture.create_history();
    let first = target(2, 20);
    let second = target(4, 21);
    store
        .append_rows(first, &row_records(first, 2), false)
        .unwrap();
    store
        .save(
            first,
            ScreenBuffer::Normal,
            &checkpoint(first, ScreenBuffer::Normal, 1),
        )
        .unwrap();
    store
        .save(
            first,
            ScreenBuffer::Alternate,
            &checkpoint(first, ScreenBuffer::Alternate, 2),
        )
        .unwrap();
    store
        .append_rows(second, &row_records(second, 1), false)
        .unwrap();
    drop(store);

    let history = fixture.path().join("history");
    let files = fs::read_dir(&history)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    let first_rows = files.iter().find(|path| {
        path.extension().is_some_and(|ext| ext == "rows")
            && path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("0000000000000014")
    });
    let second_rows = files.iter().find(|path| {
        path.extension().is_some_and(|ext| ext == "rows")
            && path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("0000000000000015")
    });
    assert!(second_rows.is_some());
    OpenOptions::new()
        .append(true)
        .open(first_rows.unwrap())
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    let normal = files
        .iter()
        .find(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.contains("0000000000000002-0000000000000014") && name.ends_with("-n.screen")
        })
        .unwrap();
    fs::write(normal, b"corrupt checkpoint").unwrap();
    let alternate = files
        .iter()
        .find(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.contains("0000000000000002-0000000000000014") && name.ends_with("-a.screen")
        })
        .unwrap();
    fs::remove_file(alternate).unwrap();

    let view = read_recovery(fixture.path(), RecoverySlot::Current).unwrap();
    assert!(matches!(view.panes[0].rows, HistoryAvailability::Invalid));
    assert!(matches!(
        view.panes[0].normal,
        CheckpointAvailability::Invalid
    ));
    assert!(matches!(
        view.panes[0].alternate,
        CheckpointAvailability::Missing
    ));
    assert!(matches!(
        view.panes[1].rows,
        HistoryAvailability::Available { .. }
    ));
    assert!(matches!(
        view.panes[1].normal,
        CheckpointAvailability::Missing
    ));
}

#[test]
fn invalid_history_directory_is_reported_per_source_without_mutation() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &layout(7, 20, 1));
    fs::write(fixture.path().join("history"), b"not a directory").unwrap();
    let before = snapshot_tree(fixture.path());
    let view = read_recovery(fixture.path(), RecoverySlot::Current).unwrap();
    assert!(matches!(view.panes[0].rows, HistoryAvailability::Invalid));
    assert!(matches!(
        view.panes[0].normal,
        CheckpointAvailability::Invalid
    ));
    assert_eq!(snapshot_tree(fixture.path()), before);
}

#[test]
fn capture_disabled_and_gap_status_are_preserved_without_inventing_reset_cause() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &layout(7, 20, 1));
    let store = fixture.create_history();
    let first = target(2, 20);
    store.mark_history(first, 6, true).unwrap();
    drop(store);

    let view = read_recovery(fixture.path(), RecoverySlot::Current).unwrap();
    let HistoryAvailability::Available {
        epoch,
        gap,
        reset,
        capture_disabled,
        rows,
        ..
    } = &view.panes[0].rows
    else {
        panic!("expected history metadata")
    };
    assert_eq!(*epoch, 6);
    assert!(*gap);
    assert_eq!(*reset, ResetStatus::PossibleFromGap);
    assert!(*capture_disabled);
    assert!(rows.is_empty());
}

#[test]
fn checkpoint_with_another_embedded_target_is_invalid_for_its_exact_slot() {
    let fixture = Fixture::new();
    fixture.write_layout(LayoutSlot::Current, &layout(7, 20, 1));
    let store = fixture.create_history();
    drop(store);
    let first = target(2, 20);
    let other = target(4, 21);
    let filename = format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}-n.screen",
        first.generation, first.workspace, first.pane, first.session, first.occupant_revision
    );
    let history = fixture.path().join("history");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(history.join(filename))
        .unwrap();
    file.write_all(&checkpoint(other, ScreenBuffer::Normal, 9))
        .unwrap();

    let view = read_recovery(fixture.path(), RecoverySlot::Current).unwrap();
    assert!(matches!(
        view.panes[0].normal,
        CheckpointAvailability::Invalid
    ));
    assert!(matches!(
        view.panes[0].alternate,
        CheckpointAvailability::Missing
    ));
    assert!(matches!(view.panes[0].rows, HistoryAvailability::Missing));
}
