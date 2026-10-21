#![cfg(unix)]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SEGMENT_HEADER_BYTES: usize = 72;
const ROWS_HEADER_BYTES: usize = 80;
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/panelink-history-corruption-{}-{}",
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

fn target_ref(session: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane: 2,
        session,
        occupant_revision: 1,
    }
}

fn rows_path(directory: &Path, target: TargetRef) -> PathBuf {
    directory.join(format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}.rows",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    ))
}

fn screen_path(directory: &Path, target: TargetRef, buffer: ScreenBuffer) -> PathBuf {
    let suffix = match buffer {
        ScreenBuffer::Normal => 'n',
        ScreenBuffer::Alternate => 'a',
    };
    directory.join(format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}-{suffix}.screen",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    ))
}

fn compact_rows(target: TargetRef, count: usize) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(2, 1, 1024);
    let initial = parser.screen().history_cursor().unwrap();
    for _ in 0..=count {
        parser.process(b"x\r\n");
    }
    let rows = parser
        .screen()
        .history_after(initial)
        .unwrap()
        .map(|row| history_rows::encode(target, 1, row, 123).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), count);
    rows
}

fn checkpoint(target: TargetRef, sequence: u64, buffer: ScreenBuffer) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap();
    let metadata = CheckpointMetadata::new(target, sequence, sequence + 1, sequence + 1).unwrap();
    let mut bytes = screen_checkpoint::encode(&snapshot, &metadata).unwrap();
    if buffer == ScreenBuffer::Alternate {
        // The public snapshot constructor starts in normal mode. This fixture
        // tag selects alternate mode, and the public decoder validates it.
        bytes[80] = 1;
    }
    assert_eq!(screen_checkpoint::decode(&bytes).unwrap().buffer(), buffer);
    bytes
}

fn assert_invalid_data<T>(result: io::Result<T>) {
    match result {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::InvalidData),
        Ok(_) => panic!("corrupt committed record was accepted"),
    }
}

#[test]
fn every_incomplete_committed_row_segment_prefix_is_rejected_and_preserved() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = compact_rows(target, 3);
    store.append_rows(target, &rows, false).unwrap();
    let path = rows_path(fixture.path(), target);
    let complete = fs::read(&path).unwrap();
    assert!(complete.len() < 1024, "fixture should stay compact");
    let first_row_length = u32::from_be_bytes(complete[72..76].try_into().unwrap()) as usize;
    let row_boundary = SEGMENT_HEADER_BYTES + 4 + first_row_length;
    assert!(row_boundary < complete.len());
    drop(store);

    for prefix_len in 0..complete.len() {
        let prefix = &complete[..prefix_len];
        fs::write(&path, prefix).unwrap();
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_invalid_data(reopened.load_rows(target));
        assert_eq!(fs::read(&path).unwrap(), prefix);
        if prefix_len == row_boundary {
            assert_eq!(fs::read(&path).unwrap(), complete[..row_boundary]);
        }
    }

    fs::write(&path, &complete).unwrap();
    let reopened = HistoryStore::open(fixture.path()).unwrap();
    let loaded = reopened.load_rows(target).unwrap().unwrap();
    assert_eq!(loaded.records, rows);
    assert_eq!(fs::read(path).unwrap(), complete);
}

#[test]
fn malformed_committed_rows_are_rejected_without_replacement_or_implicit_repair() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let other = target_ref(4);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = compact_rows(target, 3);
    let other_rows = compact_rows(other, 2);
    store.append_rows(target, &rows, false).unwrap();
    store.append_rows(other, &other_rows, false).unwrap();
    let path = rows_path(fixture.path(), target);
    let valid = fs::read(&path).unwrap();
    drop(store);

    let first_row = SEGMENT_HEADER_BYTES + 4;
    let first_len = u32::from_be_bytes(valid[72..76].try_into().unwrap()) as usize;
    let second_row = first_row + first_len + 4;
    let mut cases = Vec::new();

    let mut malformed = valid.clone();
    malformed[0] ^= 0xff;
    cases.push(("header magic", malformed));

    let mut malformed = valid.clone();
    malformed[56..58].copy_from_slice(&4_u16.to_be_bytes());
    cases.push(("count with missing row", malformed));

    let mut malformed = valid.clone();
    malformed[58] = 2;
    cases.push(("gap flag", malformed));

    let mut malformed = valid.clone();
    malformed[59] = 2;
    cases.push(("capture flag", malformed));

    let mut malformed = valid.clone();
    malformed[60] = 1;
    cases.push(("reserved header byte", malformed));

    let mut malformed = valid.clone();
    malformed[first_row + 32..first_row + 40].copy_from_slice(&99_u64.to_be_bytes());
    cases.push(("row target", malformed));

    let mut malformed = valid.clone();
    malformed[first_row + 48..first_row + 56].copy_from_slice(&2_u64.to_be_bytes());
    cases.push(("row epoch", malformed));

    let mut malformed = valid.clone();
    malformed[second_row + 56..second_row + 64].copy_from_slice(&1_u64.to_be_bytes());
    cases.push(("duplicate row ordinal", malformed));

    let mut malformed = valid.clone();
    malformed[first_row + ROWS_HEADER_BYTES + 10] = 0x1b;
    cases.push(("unsafe row cell", malformed));

    let mut malformed = valid.clone();
    malformed.push(0xaa);
    cases.push(("trailing byte", malformed));

    for (label, bytes) in cases {
        fs::write(&path, &bytes).unwrap();
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        let error = reopened.load_rows(target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{label}");
        assert_eq!(fs::read(&path).unwrap(), bytes, "{label}");
        assert_eq!(
            reopened.load_rows(other).unwrap().unwrap().records,
            other_rows,
            "healthy other target after {label}"
        );
    }
}

#[test]
fn corrupt_rows_refuse_lifecycle_mutations_and_leave_both_screen_buffers_readable() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let other = target_ref(4);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = compact_rows(target, 3);
    let other_rows = compact_rows(other, 2);
    store.append_rows(target, &rows, false).unwrap();
    store.append_rows(other, &other_rows, false).unwrap();

    let normal = checkpoint(target, 1, ScreenBuffer::Normal);
    let alternate = checkpoint(target, 2, ScreenBuffer::Alternate);
    let other_normal = checkpoint(other, 1, ScreenBuffer::Normal);
    let other_alternate = checkpoint(other, 2, ScreenBuffer::Alternate);
    store.save(target, ScreenBuffer::Normal, &normal).unwrap();
    store
        .save(target, ScreenBuffer::Alternate, &alternate)
        .unwrap();
    store
        .save(other, ScreenBuffer::Normal, &other_normal)
        .unwrap();
    store
        .save(other, ScreenBuffer::Alternate, &other_alternate)
        .unwrap();

    let path = rows_path(fixture.path(), target);
    let mut corrupt = fs::read(&path).unwrap();
    corrupt[58] = 2;
    fs::write(&path, &corrupt).unwrap();
    drop(store);

    let reopened = HistoryStore::open(fixture.path()).unwrap();
    assert_invalid_data(reopened.load_rows(target));
    assert_eq!(
        reopened.load_screens(target).unwrap(),
        [Some(normal.clone()), Some(alternate.clone())]
    );
    assert_eq!(
        reopened.load_rows(other).unwrap().unwrap().records,
        other_rows
    );

    assert!(reopened.begin_history(target, 2, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(reopened.append_rows(target, &rows, false).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(reopened.mark_history(target, 1, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(reopened.mark_ended(target, 999).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(reopened.clear(target).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    // The healthy target is live, so this call has no earlier eligible deletion.
    assert!(reopened.prune_expired(u64::MAX).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);

    assert_eq!(
        reopened.load_screens(target).unwrap(),
        [Some(normal), Some(alternate)]
    );
    assert_eq!(
        reopened.load_rows(other).unwrap().unwrap().records,
        other_rows
    );
    assert_eq!(
        reopened.load_screens(other).unwrap(),
        [Some(other_normal), Some(other_alternate)]
    );
}

#[test]
fn incomplete_checkpoint_prefixes_are_rejected_without_using_a_prefix() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let normal = checkpoint(target, 1, ScreenBuffer::Normal);
    let alternate = checkpoint(target, 2, ScreenBuffer::Alternate);
    assert!(normal.len() < 8192, "fixture should stay bounded");
    store.save(target, ScreenBuffer::Normal, &normal).unwrap();
    store
        .save(target, ScreenBuffer::Alternate, &alternate)
        .unwrap();
    let normal_path = screen_path(fixture.path(), target, ScreenBuffer::Normal);
    drop(store);

    let mut prefix_lengths = vec![0, 1, 95, 96, normal.len() / 2, normal.len() - 1];
    prefix_lengths.sort_unstable();
    prefix_lengths.dedup();
    for prefix_len in prefix_lengths {
        let prefix = &normal[..prefix_len];
        fs::write(&normal_path, prefix).unwrap();
        let reopened = HistoryStore::open(fixture.path()).unwrap();
        assert_invalid_data(reopened.load(target, ScreenBuffer::Normal));
        assert_eq!(fs::read(&normal_path).unwrap(), prefix);
        assert_eq!(
            reopened.load(target, ScreenBuffer::Alternate).unwrap(),
            Some(alternate.clone())
        );
    }

    fs::write(&normal_path, &normal).unwrap();
    let reopened = HistoryStore::open(fixture.path()).unwrap();
    assert_eq!(
        reopened.load(target, ScreenBuffer::Normal).unwrap(),
        Some(normal)
    );
}

#[test]
fn corrupt_checkpoint_refuses_replacement_and_clear_but_keeps_other_files_readable() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let other = target_ref(4);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let normal = checkpoint(target, 1, ScreenBuffer::Normal);
    let alternate = checkpoint(target, 2, ScreenBuffer::Alternate);
    let other_normal = checkpoint(other, 1, ScreenBuffer::Normal);
    store.save(target, ScreenBuffer::Normal, &normal).unwrap();
    store
        .save(target, ScreenBuffer::Alternate, &alternate)
        .unwrap();
    store
        .save(other, ScreenBuffer::Normal, &other_normal)
        .unwrap();
    let normal_path = screen_path(fixture.path(), target, ScreenBuffer::Normal);
    let corrupt = normal[..normal.len() / 2].to_vec();
    fs::write(&normal_path, &corrupt).unwrap();
    drop(store);

    let reopened = HistoryStore::open(fixture.path()).unwrap();
    assert_invalid_data(reopened.load(target, ScreenBuffer::Normal));
    assert!(reopened
        .save(
            target,
            ScreenBuffer::Normal,
            &checkpoint(target, 3, ScreenBuffer::Normal)
        )
        .is_err());
    assert_eq!(fs::read(&normal_path).unwrap(), corrupt);
    assert!(reopened.clear(target).is_err());
    assert_eq!(fs::read(&normal_path).unwrap(), corrupt);
    assert_eq!(
        reopened.load(target, ScreenBuffer::Alternate).unwrap(),
        Some(alternate)
    );
    assert_eq!(
        reopened.load(other, ScreenBuffer::Normal).unwrap(),
        Some(other_normal)
    );
}

#[test]
fn valid_shaped_cell_changes_are_not_detected_without_a_checksum() {
    let fixture = Fixture::new();
    let target = target_ref(3);
    let store = HistoryStore::open(fixture.path()).unwrap();
    let rows = compact_rows(target, 3);
    store.append_rows(target, &rows, false).unwrap();
    let path = rows_path(fixture.path(), target);
    let mut bytes = fs::read(&path).unwrap();
    let first_row = SEGMENT_HEADER_BYTES + 4;
    let text_byte = first_row + ROWS_HEADER_BYTES + 10;
    assert_eq!(bytes[text_byte], b'x');
    bytes[text_byte] = b'y';
    fs::write(path, bytes).unwrap();

    let reopened = HistoryStore::open(fixture.path()).unwrap();
    let loaded = reopened.load_rows(target).unwrap().unwrap();
    assert_eq!(
        history_rows::decode(&loaded.records[0])
            .unwrap()
            .cell_text(0),
        Some("y")
    );
}
