#![cfg(unix)]

use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
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
