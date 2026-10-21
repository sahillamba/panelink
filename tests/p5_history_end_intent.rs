#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::history_writer::{HistoryBatch, HistoryWriter, ShutdownOutcome};
use panelink::server_storage::ServerStorage;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-end-intent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn real_store_known_end_is_retained_with_no_format_or_quota_change() {
    let fixture = Fixture::new();
    let storage = ServerStorage::initialize(&fixture.0).unwrap();
    let history = fixture.0.join("history");
    fs::DirBuilder::new().mode(0o700).create(&history).unwrap();
    let target = TargetRef {
        generation: storage.generation(),
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    };
    let store = HistoryStore::open(&history).unwrap();
    let mut writer = HistoryWriter::start(store, &storage).unwrap();
    let mut receipt = writer
        .try_submit(HistoryBatch::new(target, 1, 1, true, vec![], vec![], Some(777)).unwrap())
        .unwrap();
    assert_eq!(
        writer.shutdown(Duration::from_millis(250)).outcome,
        ShutdownOutcome::Drained
    );
    assert_eq!(
        receipt.poll(),
        panelink::history_writer::CommitStatus::Committed
    );
    let saved = HistoryStore::open(&history)
        .unwrap()
        .load_rows(target)
        .unwrap()
        .unwrap();
    assert_eq!(saved.ended_at_unix_ms, 777);
    assert!(saved.gap);
    assert!(saved.records.is_empty());
}
