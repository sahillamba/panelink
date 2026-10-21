#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Component acceptance only: no live capture scheduling or server sockets.

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::history_writer::{
    CommitStatus, HistoryBatch, HistoryWriter, Receipt, ShutdownOutcome, SubmitReason,
};
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/pl-writer-acceptance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn history(&self) -> PathBuf {
        self.0.join("history")
    }

    fn prepare(&self) -> (ServerStorage, HistoryStore, TargetRef) {
        let storage = ServerStorage::initialize(&self.0).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.history())
            .unwrap();
        let store = HistoryStore::open(&self.history()).unwrap();
        let target = TargetRef {
            generation: storage.generation(),
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        (storage, store, target)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rows(target: TargetRef, epoch: u64) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(2, 20, 1024);
    let cursor = parser.screen().history_cursor().unwrap();
    parser.process(b"saved-one\r\nsaved-two\r\nsaved-three\r\n");
    parser
        .screen()
        .history_after(cursor)
        .unwrap()
        .map(|row| history_rows::encode(target, epoch, row, 456).unwrap())
        .collect()
}

fn screen(target: TargetRef, alternate: bool) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
    let mut bytes = screen_checkpoint::encode(
        &snapshot,
        &CheckpointMetadata::new(target, 7, 456, 8).unwrap(),
    )
    .unwrap();
    // Make a codec-valid alternate blank fixture without launching a process.
    bytes[80] = u8::from(alternate);
    screen_checkpoint::decode(&bytes).unwrap();
    bytes
}

fn settle(receipt: &mut Receipt) -> CommitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = receipt.poll();
        if status != CommitStatus::Pending {
            assert_eq!(receipt.poll(), status, "receipt result must remain stable");
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "component receipt did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn durable_batch_survives_owner_drop_and_reopens_with_rows_both_buffers_and_end() {
    let fixture = Fixture::new();
    let (storage, store, target) = fixture.prepare();
    let expected_rows = rows(target, 1);
    let normal = screen(target, false);
    let alternate = screen(target, true);
    let mut writer = HistoryWriter::start(store, &storage).unwrap();
    let batch = HistoryBatch::new(
        target,
        1,
        29,
        false,
        expected_rows.clone(),
        vec![normal.clone(), alternate.clone()],
        Some(1000),
    )
    .unwrap();
    let mut receipt = writer.try_submit(batch).unwrap();
    assert_eq!(receipt.target(), target);
    assert_eq!(receipt.epoch(), 1);
    assert_eq!(receipt.token(), 29);
    drop(storage);
    assert_eq!(
        ServerStorage::lock_for_maintenance(&fixture.0)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(settle(&mut receipt), CommitStatus::Committed);
    assert!(!writer.had_storage_error());
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
    assert_eq!(receipt.poll(), CommitStatus::Committed);

    let reopened = HistoryStore::open(&fixture.history()).unwrap();
    let saved = reopened.load_rows(target).unwrap().unwrap();
    assert_eq!(saved.records, expected_rows);
    assert_eq!(saved.ended_at_unix_ms, 1000);
    assert!(!saved.gap);
    assert!(!saved.capture_disabled);
    assert_eq!(
        reopened.load(target, ScreenBuffer::Normal).unwrap(),
        Some(normal)
    );
    assert_eq!(
        reopened.load(target, ScreenBuffer::Alternate).unwrap(),
        Some(alternate)
    );
    // The worker released its retained lock before reporting a drained exit.
    drop(ServerStorage::lock_for_maintenance(&fixture.0).unwrap());
}

#[test]
fn fifo_reset_replaces_prior_epoch_before_end_without_advancing_old_receipts() {
    let fixture = Fixture::new();
    let (storage, store, target) = fixture.prepare();
    let mut writer = HistoryWriter::start(store, &storage).unwrap();
    let mut old = writer
        .try_submit(HistoryBatch::new(target, 1, 1, false, rows(target, 1), vec![], None).unwrap())
        .unwrap();
    let new_rows = rows(target, 2);
    let mut current = writer
        .try_submit(
            HistoryBatch::new(target, 2, 2, true, new_rows.clone(), vec![], Some(2000)).unwrap(),
        )
        .unwrap();
    assert_eq!(settle(&mut current), CommitStatus::Committed);
    assert_eq!(settle(&mut old), CommitStatus::Committed);
    assert_eq!(old.epoch(), 1);
    assert_eq!(current.epoch(), 2);
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
    let saved = HistoryStore::open(&fixture.history())
        .unwrap()
        .load_rows(target)
        .unwrap()
        .unwrap();
    assert_eq!(saved.epoch, 2);
    assert_eq!(saved.records, new_rows);
    assert!(saved.gap);
    assert_eq!(saved.ended_at_unix_ms, 2000);
}

#[test]
fn unsafe_store_fails_receipts_and_later_batches_without_writing_replacement() {
    let fixture = Fixture::new();
    let (storage, store, target) = fixture.prepare();
    let mut writer = HistoryWriter::start(store, &storage).unwrap();
    fs::rename(fixture.history(), fixture.0.join("old-history")).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.history())
        .unwrap();
    let mut first = writer
        .try_submit(HistoryBatch::new(target, 1, 1, false, vec![], vec![], Some(3000)).unwrap())
        .unwrap();
    let later = writer
        .try_submit(HistoryBatch::new(target, 1, 2, false, rows(target, 1), vec![], None).unwrap());
    assert_eq!(settle(&mut first), CommitStatus::Failed);
    match later {
        Ok(mut receipt) => assert_eq!(settle(&mut receipt), CommitStatus::Failed),
        Err(rejected) => assert_eq!(rejected.reason(), SubmitReason::Failed),
    }
    assert!(writer.had_storage_error());
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Failed
    );
    assert_eq!(fs::read_dir(fixture.history()).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(fixture.0.join("old-history")).unwrap().count(),
        0
    );
}

#[test]
fn writer_rejects_a_store_from_a_different_server_lock_domain() {
    let first = Fixture::new();
    let second = Fixture::new();
    let (first_storage, first_store, _) = first.prepare();
    let (_second_storage, second_store, _) = second.prepare();
    assert!(HistoryWriter::start(second_store, &first_storage).is_err());
    // Failed binding must not prevent a correctly bound worker from starting.
    let writer = HistoryWriter::start(first_store, &first_storage).unwrap();
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
    assert_eq!(fs::read_dir(first.history()).unwrap().count(), 0);
    assert_eq!(fs::read_dir(second.history()).unwrap().count(), 0);
}

#[test]
fn reused_receipt_token_is_rejected_before_disk_effects() {
    let fixture = Fixture::new();
    let (storage, store, target) = fixture.prepare();
    let mut writer = HistoryWriter::start(store, &storage).unwrap();
    let mut initial = writer
        .try_submit(HistoryBatch::new(target, 1, 1, false, vec![], vec![], None).unwrap())
        .unwrap();
    assert_eq!(settle(&mut initial), CommitStatus::Committed);
    let rejected = writer
        .try_submit(HistoryBatch::new(target, 1, 1, false, vec![], vec![], Some(999)).unwrap())
        .unwrap_err();
    assert_eq!(rejected.reason(), SubmitReason::StaleToken);
    assert_eq!(
        HistoryStore::open(&fixture.history())
            .unwrap()
            .load_rows(target)
            .unwrap()
            .unwrap()
            .ended_at_unix_ms,
        0
    );
    let mut final_receipt = writer
        .try_submit(HistoryBatch::new(target, 1, 2, false, vec![], vec![], Some(1000)).unwrap())
        .unwrap();
    assert_eq!(settle(&mut final_receipt), CommitStatus::Committed);
    assert_eq!(initial.poll(), CommitStatus::Committed);
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
}

#[test]
fn only_one_writer_can_claim_a_server_until_the_worker_exits() {
    let fixture = Fixture::new();
    let (storage, store, _) = fixture.prepare();
    let first = HistoryWriter::start(store, &storage).unwrap();
    let second_store = HistoryStore::open(&fixture.history()).unwrap();
    assert_eq!(
        HistoryWriter::start(second_store, &storage)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        first.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
    let replacement =
        HistoryWriter::start(HistoryStore::open(&fixture.history()).unwrap(), &storage).unwrap();
    assert_eq!(
        replacement.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
}
