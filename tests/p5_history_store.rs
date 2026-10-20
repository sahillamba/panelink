#![cfg(unix)]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/panelink-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
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
        generation: 7,
        workspace: 1,
        pane: 2,
        session: 3,
        occupant_revision: 1,
    }
}

fn record(target: TargetRef, sequence: u64) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
    screen_checkpoint::encode(
        &snapshot,
        &CheckpointMetadata::new(target, sequence, sequence + 1, sequence + 1).unwrap(),
    )
    .unwrap()
}

fn row_records(target: TargetRef, count: usize) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(2, 20, 1024);
    let initial = parser.screen().history_cursor().unwrap();
    for index in 0..count + 1 {
        parser.process(format!("row-{index}\r\n").as_bytes());
    }
    parser
        .screen()
        .history_after(initial)
        .unwrap()
        .map(|row| history_rows::encode(target, 1, row, 123).unwrap())
        .collect()
}

#[test]
fn scrollback_batch_retains_only_bounded_rows_and_marks_truncation() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let target = target();
    let rows = row_records(target, 700);
    store.append_rows(target, &rows, false).unwrap();
    assert_eq!(store.load_rows(target).unwrap().unwrap().records.len(), 700);
    let mut second = vt100::Parser::new(2, 20, 1024);
    for index in 0..1401 {
        second.process(format!("row-{index}\r\n").as_bytes());
    }
    let after = vt100::HistoryCursor {
        epoch: 1,
        ordinal: 700,
    };
    let later = second
        .screen()
        .history_after(after)
        .unwrap()
        .map(|row| history_rows::encode(target, 1, row, 124).unwrap())
        .collect::<Vec<_>>();
    store.append_rows(target, &later, false).unwrap();
    let saved = store.load_rows(target).unwrap().unwrap();
    assert_eq!(saved.records.len(), 1024);
    assert!(saved.gap);
    assert!(fs::read_dir(fixture.path()).unwrap().all(|entry| entry
        .unwrap()
        .metadata()
        .unwrap()
        .len()
        <= 8 * 1024 * 1024));
}

#[test]
fn expiry_and_clear_are_scoped_to_ended_targets() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let ended = target();
    let live = TargetRef {
        session: 4,
        ..ended
    };
    store
        .append_rows(ended, &row_records(ended, 2), false)
        .unwrap();
    store
        .append_rows(live, &row_records(live, 2), false)
        .unwrap();
    store.mark_ended(ended, 1000).unwrap();
    assert_eq!(
        store
            .prune_expired(1000 + 30 * 24 * 60 * 60 * 1000 - 1)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .prune_expired(1000 + 30 * 24 * 60 * 60 * 1000)
            .unwrap(),
        1
    );
    assert!(store.load_rows(ended).unwrap().is_none());
    assert!(store.load_rows(live).unwrap().is_some());
    store.clear(live).unwrap();
    assert!(store.load_rows(live).unwrap().is_none());
}

#[test]
fn corrupt_segment_and_symlink_fail_without_overwriting_other_history() {
    let fixture = Fixture::new();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = row_records(target, 3);
    store.append_rows(target, &rows, false).unwrap();
    let segment = fs::read_dir(fixture.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::OpenOptions::new()
        .append(true)
        .open(&segment)
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    assert_eq!(
        store.load_rows(target).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(store.append_rows(target, &rows, false).is_err());
    assert!(store.clear(target).is_err());
    assert!(
        segment.exists(),
        "corruption is preserved for explicit repair"
    );
    fs::remove_file(&segment).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &segment).unwrap();
    assert!(store.load_rows(target).is_err());
    assert!(store.append_rows(target, &rows, false).is_err());
    assert!(store.clear(target).is_err());
}

#[test]
fn lost_ack_retry_accepts_only_identical_saved_rows() {
    let fixture = Fixture::new();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = row_records(target, 3);
    store.append_rows(target, &rows, false).unwrap();
    store.append_rows(target, &rows, false).unwrap();
    assert_eq!(store.load_rows(target).unwrap().unwrap().records.len(), 3);
    let mut recaptured = rows.clone();
    for row in &mut recaptured {
        row[64..72].copy_from_slice(&456_u64.to_be_bytes());
    }
    store.append_rows(target, &recaptured, false).unwrap();
    assert_eq!(store.load_rows(target).unwrap().unwrap().records.len(), 3);
    let mut changed = rows;
    changed[1][history_rows::HEADER_BYTES + 10] = b'X';
    assert!(store.append_rows(target, &changed, false).is_err());
    assert_eq!(store.load_rows(target).unwrap().unwrap().records.len(), 3);
}

#[test]
fn full_segment_retry_does_not_redispatch_or_duplicate_rows() {
    let fixture = Fixture::new();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = row_records(target, 1024);
    assert_eq!(rows.len(), 1024);
    store.append_rows(target, &rows, false).unwrap();
    let start = std::time::Instant::now();
    store.append_rows(target, &rows, false).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(
        store.load_rows(target).unwrap().unwrap().records.len(),
        1024
    );
}

#[test]
fn maintenance_lock_blocks_direct_server_open_and_fails_closed_on_missing_metadata() {
    let fixture = Fixture::new();
    assert!(ServerStorage::lock_for_maintenance(fixture.path()).is_err());
    let server = ServerStorage::initialize(fixture.path()).unwrap();
    assert!(ServerStorage::lock_for_maintenance(fixture.path()).is_err());
    drop(server);
    let guard = ServerStorage::lock_for_maintenance(fixture.path()).unwrap();
    assert!(ServerStorage::open(fixture.path()).is_err());
    drop(guard);
    assert!(ServerStorage::open(fixture.path()).is_ok());
}

#[test]
fn checkpoint_survives_reopen_and_rejects_stale_replacement() {
    let fixture = Fixture::new();
    let target = target();
    let first = record(target, 1);
    let second = record(target, 2);
    let store = HistoryStore::open(fixture.path()).unwrap();
    assert!(store.load(target, ScreenBuffer::Normal).unwrap().is_none());
    store.save(target, ScreenBuffer::Normal, &first).unwrap();
    assert_eq!(
        store.load(target, ScreenBuffer::Normal).unwrap(),
        Some(first.clone())
    );
    store.save(target, ScreenBuffer::Normal, &first).unwrap();
    assert_eq!(
        store.load(target, ScreenBuffer::Normal).unwrap(),
        Some(first)
    );
    store.save(target, ScreenBuffer::Normal, &second).unwrap();
    drop(store);
    let reopened = HistoryStore::open(fixture.path()).unwrap();
    assert_eq!(
        reopened.load(target, ScreenBuffer::Normal).unwrap(),
        Some(second)
    );
    assert!(reopened
        .load(target, ScreenBuffer::Alternate)
        .unwrap()
        .is_none());
    assert!(reopened
        .load(
            TargetRef {
                session: 4,
                ..target
            },
            ScreenBuffer::Normal
        )
        .unwrap()
        .is_none());
}

#[test]
fn malformed_or_unsafe_records_fail_closed_without_replacing_last_commit() {
    let fixture = Fixture::new();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let valid = record(target, 1);
    store.save(target, ScreenBuffer::Normal, &valid).unwrap();
    let mut malformed = valid.clone();
    malformed[0] = 0;
    assert_eq!(
        store
            .save(target, ScreenBuffer::Normal, &malformed)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        store
            .save(
                TargetRef {
                    session: 9,
                    ..target
                },
                ScreenBuffer::Normal,
                &valid
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        store
            .save(target, ScreenBuffer::Alternate, &valid)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        store.load(target, ScreenBuffer::Normal).unwrap(),
        Some(valid)
    );

    let filename = fs::read_dir(fixture.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut file = OpenOptions::new().append(true).open(&filename).unwrap();
    file.write_all(b"partial").unwrap();
    assert_eq!(
        store.load(target, ScreenBuffer::Normal).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        store
            .save(target, ScreenBuffer::Normal, &record(target, 2))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    drop(store);
    assert_eq!(
        HistoryStore::open(fixture.path())
            .unwrap()
            .load(target, ScreenBuffer::Normal)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn private_directory_and_symlink_entries_are_required() {
    let fixture = Fixture::new();
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        HistoryStore::open(fixture.path()).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let link = fixture.path().with_extension("link");
    std::os::unix::fs::symlink(fixture.path(), &link).unwrap();
    assert_eq!(
        HistoryStore::open(&link).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    fs::remove_file(link).unwrap();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    store
        .save(target, ScreenBuffer::Normal, &record(target, 1))
        .unwrap();
    let entry = fs::read_dir(fixture.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(&entry).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &entry).unwrap();
    assert!(store.load(target, ScreenBuffer::Normal).is_err());
    assert!(store
        .save(target, ScreenBuffer::Normal, &record(target, 2))
        .is_err());
}

#[test]
fn removed_history_directory_is_not_written_through_the_open_descriptor() {
    let fixture = Fixture::new();
    let target = target();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let moved = fixture.path().with_extension("moved");
    fs::rename(fixture.path(), &moved).unwrap();
    assert!(store
        .save(target, ScreenBuffer::Normal, &record(target, 1))
        .is_err());
    assert!(fs::read_dir(&moved).unwrap().next().is_none());
    fs::rename(moved, fixture.path()).unwrap();
    store
        .save(target, ScreenBuffer::Normal, &record(target, 1))
        .unwrap();
}
