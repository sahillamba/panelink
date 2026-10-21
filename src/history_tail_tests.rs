//! Independent parent acceptance against the real store under gated pressure.
use crate::control_protocol::TargetRef;
use crate::history_capture::{CaptureObservation, CheckpointRecord, HistoryCaptureCoordinator};
use crate::history_diagnostics::{HistoryDrainOutcome, HistoryReason};
use crate::history_rows;
use crate::history_store::{HistoryStore, SavedHistory};
use crate::history_writer::{
    CapacityReason, CommitStatus, HistoryBackend, HistoryBatch, HistoryWriter, Receipt,
    ShutdownOutcome, MAX_BATCH_ROWS, MAX_QUEUED_PAYLOAD_BYTES, MAX_SHUTDOWN_WAIT,
};
use crate::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use crate::server_storage::ServerStorage;
use crate::terminal::{ScreenSnapshot, TerminalSize};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vt100::HistoryCursor;

static NEXT: AtomicU64 = AtomicU64::new(0);
const FINAL: TargetRef = TargetRef {
    generation: 1,
    workspace: 1,
    pane: 3,
    session: 3,
    occupant_revision: 1,
};

struct Fixture {
    root: PathBuf,
    storage: Option<ServerStorage>,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-final-tail-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
            .unwrap();
        let storage = ServerStorage::initialize(&root.join("server")).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server/history"))
            .unwrap();
        Self {
            root,
            storage: Some(storage),
        }
    }

    fn store(&self) -> HistoryStore {
        HistoryStore::open(&self.root.join("server/history")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.storage.take());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match ServerStorage::lock_for_maintenance(&self.root.join("server")) {
                Ok(lock) => {
                    let _ = fs::remove_dir_all(&self.root);
                    drop(lock);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        // Never delete beneath a detached writer on test failure.
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return,
            }
        }
    }
}

struct GatedStore {
    store: HistoryStore,
    entered: Option<SyncSender<()>>,
    release: Receiver<()>,
}

impl HistoryBackend for GatedStore {
    fn load_rows(&mut self, target: TargetRef) -> io::Result<Option<SavedHistory>> {
        if let Some(entered) = self.entered.take() {
            entered
                .send(())
                .map_err(|_| io::Error::other("test gate closed"))?;
            self.release
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::Error::other("test gate closed"))?;
        }
        self.store.load_rows(target)
    }
    fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
        self.store.begin_history(target, epoch, gap)
    }
    fn mark_history(&mut self, target: TargetRef, epoch: u64, disabled: bool) -> io::Result<()> {
        self.store.mark_history(target, epoch, disabled)
    }
    fn append_rows(&mut self, target: TargetRef, rows: &[Vec<u8>], gap: bool) -> io::Result<()> {
        self.store.append_rows(target, rows, gap)
    }
    fn save_screen(
        &mut self,
        target: TargetRef,
        buffer: ScreenBuffer,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.store.save(target, buffer, bytes)
    }
    fn mark_ended(&mut self, target: TargetRef, at: u64) -> io::Result<()> {
        self.store.mark_ended(target, at)
    }
}

fn rows(target: TargetRef, epoch: u64, count: usize) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(1, 240, 2048);
    let cursor = parser.screen().history_cursor().unwrap();
    for index in 0..=count {
        parser.process(format!("{:<240}\r\n", format!("TAIL-{index:04}")).as_bytes());
    }
    let rows: Vec<_> = parser
        .screen()
        .history_after(cursor)
        .unwrap()
        .take(count)
        .map(|row| history_rows::encode(target, epoch, row, 111).unwrap())
        .collect();
    assert_eq!(rows.len(), count);
    assert!(rows
        .iter()
        .all(|row| row.len() == history_rows::MAX_ROW_BYTES));
    rows
}

fn screen(buffer: ScreenBuffer) -> (CheckpointRecord, Vec<u8>) {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(80, 240).unwrap()).unwrap();
    let sequence = if buffer == ScreenBuffer::Normal { 8 } else { 9 };
    let metadata = CheckpointMetadata::new(FINAL, sequence, 123, 4).unwrap();
    let mut bytes = screen_checkpoint::encode(&snapshot, &metadata).unwrap();
    bytes[80] = if buffer == ScreenBuffer::Normal { 0 } else { 1 };
    let candidate = CheckpointRecord::new(FINAL, buffer, metadata, bytes.clone()).unwrap();
    (candidate, bytes)
}

#[test]
fn final_tail_actual_store_preserves_newest_suffix_screens_epoch_and_first_end() {
    let fixture = Fixture::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let writer = HistoryWriter::start_with_backend(
        Box::new(GatedStore {
            store: fixture.store(),
            entered: Some(entered_tx),
            release: release_rx,
        }),
        fixture.storage.as_ref().unwrap(),
    )
    .unwrap();
    let first = TargetRef {
        pane: 1,
        session: 1,
        ..FINAL
    };
    let second = TargetRef {
        pane: 2,
        session: 2,
        ..FINAL
    };
    let failure = Arc::new(AtomicBool::new(false));
    let diagnostics = writer.diagnostics();
    let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
    for target in [first, second, FINAL] {
        assert!(capture.register_target(
            target,
            HistoryCursor {
                epoch: 1,
                ordinal: 0
            }
        ));
    }
    let first_rows = rows(first, 1, MAX_BATCH_ROWS);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        capture.observe(
            first,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: MAX_BATCH_ROWS as u64,
                },
                gap: false,
                rows: first_rows.clone(),
                screens: Vec::new(),
            },
        );
        if capture.has_regular_pending(first) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "initial admission did not settle"
        );
        std::thread::yield_now();
    }
    drop(first_rows);
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    capture.observe(
        second,
        CaptureObservation {
            cursor: HistoryCursor {
                epoch: 1,
                ordinal: MAX_BATCH_ROWS as u64,
            },
            gap: false,
            rows: rows(second, 1, MAX_BATCH_ROWS),
            screens: Vec::new(),
        },
    );
    assert!(capture.has_regular_pending(second));
    for target in [first, second] {
        assert!(capture.finish_target(
            target,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: MAX_BATCH_ROWS as u64
                },
                gap: false,
                rows: Vec::new(),
                screens: Vec::new(),
            },
            Some(555)
        ));
    }
    let (normal, normal_bytes) = screen(ScreenBuffer::Normal);
    let (alternate, alternate_bytes) = screen(ScreenBuffer::Alternate);
    let available = MAX_QUEUED_PAYLOAD_BYTES - 2 * MAX_BATCH_ROWS * history_rows::MAX_ROW_BYTES;
    let expected_count =
        (available - normal_bytes.len() - alternate_bytes.len()) / history_rows::MAX_ROW_BYTES;
    assert!(expected_count > 0 && expected_count < 900);
    assert!(capture.finish_target(
        FINAL,
        CaptureObservation {
            cursor: HistoryCursor {
                epoch: 2,
                ordinal: 900
            },
            gap: false,
            rows: rows(FINAL, 2, 900),
            screens: vec![normal, alternate],
        },
        Some(777)
    ));
    assert!(capture.finish_target(
        FINAL,
        CaptureObservation {
            cursor: HistoryCursor {
                epoch: 2,
                ordinal: 900
            },
            gap: false,
            rows: Vec::new(),
            screens: Vec::new(),
        },
        Some(999)
    ));
    assert!(failure.load(Ordering::Acquire));
    assert_eq!(
        capture.submitted_cursor(FINAL),
        Some(HistoryCursor {
            epoch: 2,
            ordinal: 900
        })
    );
    assert_eq!(
        capture.durable_cursor(FINAL),
        Some(HistoryCursor {
            epoch: 1,
            ordinal: 0
        })
    );
    assert!(diagnostics
        .snapshot()
        .contains(HistoryReason::FinalPayloadDroppedBytes));
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !capture.all_targets_settled() {
        capture.drive();
        assert!(
            Instant::now() < deadline,
            "real history suffix did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let writer = capture.take_writer_for_shutdown().unwrap();
    writer.shutdown(MAX_SHUTDOWN_WAIT);
    assert_eq!(
        diagnostics.snapshot().writer_drain_outcome(),
        HistoryDrainOutcome::Drained
    );
    let store = fixture.store();
    let saved = store.load_rows(FINAL).unwrap().unwrap();
    assert_eq!((saved.epoch, saved.ended_at_unix_ms), (2, 777));
    assert!(saved.gap);
    assert_eq!(saved.records.len(), expected_count);
    for (index, bytes) in saved.records.iter().enumerate() {
        let row = history_rows::decode(bytes).unwrap();
        assert_eq!(row.ordinal, 900 - expected_count as u64 + 1 + index as u64);
        let text: String = (0..row.width)
            .map(|column| row.cell_text(column).unwrap())
            .collect();
        assert_eq!(text.trim_end(), format!("TAIL-{:04}", row.ordinal - 1));
    }
    assert_eq!(
        store.load(FINAL, ScreenBuffer::Normal).unwrap().unwrap(),
        normal_bytes
    );
    assert_eq!(
        store.load(FINAL, ScreenBuffer::Alternate).unwrap().unwrap(),
        alternate_bytes
    );
}

struct FinalizingStore {
    store: HistoryStore,
    load_entered: Option<SyncSender<()>>,
    load_release: Receiver<()>,
    end_entered: Option<SyncSender<()>>,
    end_release: Receiver<()>,
    payload_called: Arc<AtomicBool>,
}

impl HistoryBackend for FinalizingStore {
    fn load_rows(&mut self, target: TargetRef) -> io::Result<Option<SavedHistory>> {
        if let Some(entered) = self.load_entered.take() {
            entered
                .send(())
                .map_err(|_| io::Error::other("test gate closed"))?;
            self.load_release
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::Error::other("test gate closed"))?;
        }
        self.store.load_rows(target)
    }
    fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
        self.store.begin_history(target, epoch, gap)
    }
    fn mark_history(&mut self, target: TargetRef, epoch: u64, disabled: bool) -> io::Result<()> {
        self.store.mark_history(target, epoch, disabled)
    }
    fn append_rows(&mut self, target: TargetRef, rows: &[Vec<u8>], gap: bool) -> io::Result<()> {
        self.payload_called.store(true, Ordering::Release);
        self.store.append_rows(target, rows, gap)
    }
    fn save_screen(
        &mut self,
        target: TargetRef,
        buffer: ScreenBuffer,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.payload_called.store(true, Ordering::Release);
        self.store.save(target, buffer, bytes)
    }
    fn mark_ended(&mut self, target: TargetRef, at: u64) -> io::Result<()> {
        self.store.mark_ended(target, at)
    }
    fn finalize_incomplete_end(
        &mut self,
        target: TargetRef,
        epoch: u64,
        at: u64,
    ) -> io::Result<()> {
        if let Some(entered) = self.end_entered.take() {
            entered
                .send(())
                .map_err(|_| io::Error::other("test gate closed"))?;
            self.end_release
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::Error::other("test gate closed"))?;
        }
        self.store.finalize_incomplete_end(target, epoch, at)
    }
}

fn wait_canceled(receipt: &mut Receipt) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match receipt.poll() {
            CommitStatus::Pending => {
                assert!(Instant::now() < deadline, "canceled receipt did not settle");
                std::thread::sleep(Duration::from_millis(1));
            }
            status => {
                assert_eq!(status, CommitStatus::Canceled);
                return;
            }
        }
    }
}

fn canceled_end_actual_store(explicit_timeout: bool) {
    let mut fixture = Fixture::new();
    let second = TargetRef {
        pane: 4,
        session: 4,
        ..FINAL
    };
    let store = fixture.store();
    let original_rows = rows(FINAL, 1, 2);
    store.append_rows(FINAL, &original_rows, false).unwrap();
    let (_, original_normal) = screen(ScreenBuffer::Normal);
    store
        .save(FINAL, ScreenBuffer::Normal, &original_normal)
        .unwrap();
    let (_, candidate_alternate) = screen(ScreenBuffer::Alternate);
    let (load_tx, load_rx) = mpsc::sync_channel(1);
    let (load_release_tx, load_release_rx) = mpsc::sync_channel(1);
    let (end_tx, end_rx) = mpsc::sync_channel(1);
    let (end_release_tx, end_release_rx) = mpsc::sync_channel(1);
    let payload_called = Arc::new(AtomicBool::new(false));
    let mut writer = HistoryWriter::start_with_backend(
        Box::new(FinalizingStore {
            store: fixture.store(),
            load_entered: Some(load_tx),
            load_release: load_release_rx,
            end_entered: Some(end_tx),
            end_release: end_release_rx,
            payload_called: Arc::clone(&payload_called),
        }),
        fixture.storage.as_ref().unwrap(),
    )
    .unwrap();
    let observer = writer.diagnostics();
    let mut batch = HistoryBatch::new(
        FINAL,
        1,
        1,
        false,
        rows(FINAL, 1, 3),
        vec![candidate_alternate],
        Some(777),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut first = loop {
        match writer.try_submit(batch) {
            Ok(receipt) => break receipt,
            Err(error) => {
                assert_eq!(error.capacity_reason(), Some(CapacityReason::Contended));
                assert!(Instant::now() < deadline);
                batch = error.into_batch();
                std::thread::yield_now();
            }
        }
    };
    load_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut queued = writer
        .try_submit(
            HistoryBatch::new(
                second,
                1,
                2,
                false,
                rows(second, 1, 3),
                Vec::new(),
                Some(888),
            )
            .unwrap(),
        )
        .unwrap();
    let started = Instant::now();
    if explicit_timeout {
        assert_eq!(
            writer.shutdown(Duration::from_millis(5)).outcome,
            ShutdownOutcome::TimedOut
        );
    } else {
        drop(writer);
    }
    assert!(started.elapsed() < MAX_SHUTDOWN_WAIT + Duration::from_millis(100));
    assert_eq!(first.poll(), CommitStatus::Pending);
    assert_eq!(store.load_rows(FINAL).unwrap().unwrap().ended_at_unix_ms, 0);
    assert!(!observer
        .snapshot()
        .contains(HistoryReason::CanceledEndMetadataFinalized));
    drop(fixture.storage.take());
    load_release_tx.send(()).unwrap();
    if explicit_timeout {
        end_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(first.poll(), CommitStatus::Pending);
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root.join("server"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        end_release_tx.send(()).unwrap();
    }
    wait_canceled(&mut first);
    wait_canceled(&mut queued);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match ServerStorage::lock_for_maintenance(&fixture.root.join("server")) {
            Ok(lock) => {
                drop(lock);
                break;
            }
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                assert!(Instant::now() < deadline, "finalizer lock did not settle");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert!(
        !payload_called.load(Ordering::Acquire),
        "cancellation must skip payload writes"
    );
    let saved = store.load_rows(FINAL).unwrap().unwrap();
    assert_eq!(saved.records, original_rows);
    assert_eq!(
        store.load(FINAL, ScreenBuffer::Normal).unwrap().unwrap(),
        original_normal
    );
    assert!(store
        .load(FINAL, ScreenBuffer::Alternate)
        .unwrap()
        .is_none());
    let snapshot = observer.snapshot();
    if explicit_timeout {
        assert!(saved.gap);
        assert_eq!(saved.ended_at_unix_ms, 777);
        let queued = store.load_rows(second).unwrap().unwrap();
        assert!(queued.gap && queued.records.is_empty());
        assert_eq!(queued.ended_at_unix_ms, 888);
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    } else {
        assert!(!saved.gap);
        assert_eq!(saved.ended_at_unix_ms, 0);
        assert!(store.load_rows(second).unwrap().is_none());
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::NotRequested
        );
    }
}

#[test]
fn canceled_end_actual_store_is_finalized_only_after_explicit_timeout() {
    canceled_end_actual_store(true);
}

#[test]
fn canceled_end_actual_store_plain_drop_does_not_finalize() {
    canceled_end_actual_store(false);
}
