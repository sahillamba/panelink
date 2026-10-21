#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let parent = Path::new("/private/tmp");
        assert_eq!(fs::canonicalize(parent).unwrap(), parent);
        for _ in 0..100 {
            let path = parent.join(format!(
                "panelink-p5-history-cleanup-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private history fixture: {error}"),
            }
        }
        panic!("could not allocate a unique private history fixture")
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

fn target(session: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane: 2,
        session,
        occupant_revision: 1,
    }
}

fn fields(target: TargetRef) -> String {
    format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn row_name(target: TargetRef) -> String {
    format!("{}.rows", fields(target))
}

fn screen_name(target: TargetRef, buffer: ScreenBuffer) -> String {
    let suffix = match buffer {
        ScreenBuffer::Normal => "n",
        ScreenBuffer::Alternate => "a",
    };
    format!("{}-{suffix}.screen", fields(target))
}

fn temporary_name(final_name: &str, pid: &str, counter: &str) -> String {
    format!(".{final_name}-{pid}-{counter}.tmp")
}

fn create_private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn checkpoint(target: TargetRef, sequence: u64) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(8, 20).unwrap()).unwrap();
    let metadata = CheckpointMetadata::new(target, sequence, sequence + 10, sequence + 1).unwrap();
    screen_checkpoint::encode(&snapshot, &metadata).unwrap()
}

fn row_records(target: TargetRef, count: usize) -> Vec<Vec<u8>> {
    let mut parser = vt100::Parser::new(2, 20, 1024);
    let initial = parser.screen().history_cursor().unwrap();
    for index in 0..=count {
        parser.process(format!("row-{index}\r\n").as_bytes());
    }
    let records = parser
        .screen()
        .history_after(initial)
        .unwrap()
        .map(|row| history_rows::encode(target, 1, row, 123).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), count);
    records
}

#[test]
fn committed_rows_and_both_screens_survive_cleanup_and_reopen_byte_for_byte() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let committed = target(3);
    let rows = row_records(committed, 3);
    let normal = checkpoint(committed, 1);
    let mut alternate = checkpoint(committed, 2);
    alternate[80] = 1;
    assert_eq!(
        screen_checkpoint::decode(&alternate).unwrap().buffer(),
        ScreenBuffer::Alternate
    );
    store.append_rows(committed, &rows, false).unwrap();
    store
        .save(committed, ScreenBuffer::Normal, &normal)
        .unwrap();
    store
        .save(committed, ScreenBuffer::Alternate, &alternate)
        .unwrap();

    // A complete rows segment and a complete checkpoint are moved out of
    // their committed slots to model an orphan left before promotion. A
    // separate partial rows temp exercises incomplete writes.
    let orphan = target(4);
    let orphan_rows = row_records(orphan, 2);
    let orphan_screen = checkpoint(orphan, 1);
    store.append_rows(orphan, &orphan_rows, false).unwrap();
    store
        .save(orphan, ScreenBuffer::Normal, &orphan_screen)
        .unwrap();
    drop(store);
    let complete_rows_temp = temporary_name(&row_name(orphan), "11", "1");
    let complete_screen_temp =
        temporary_name(&screen_name(orphan, ScreenBuffer::Normal), "12", "2");
    fs::rename(
        fixture.path().join(row_name(orphan)),
        fixture.path().join(&complete_rows_temp),
    )
    .unwrap();
    fs::rename(
        fixture
            .path()
            .join(screen_name(orphan, ScreenBuffer::Normal)),
        fixture.path().join(&complete_screen_temp),
    )
    .unwrap();
    let partial_temp = temporary_name(&row_name(committed), "13", "3");
    create_private_file(&fixture.path().join(&partial_temp), b"partial segment");

    let store = HistoryStore::open(fixture.path()).unwrap();
    assert_eq!(store.cleanup_temporary_files().unwrap(), 3);
    assert!(!fixture.path().join(complete_rows_temp).exists());
    assert!(!fixture.path().join(complete_screen_temp).exists());
    assert!(!fixture.path().join(partial_temp).exists());
    assert!(store.load_rows(orphan).unwrap().is_none());
    assert!(store.load(orphan, ScreenBuffer::Normal).unwrap().is_none());
    drop(store);

    let reopened = HistoryStore::open(fixture.path()).unwrap();
    assert_eq!(
        reopened.load_rows(committed).unwrap().unwrap().records,
        rows
    );
    assert_eq!(
        reopened.load(committed, ScreenBuffer::Normal).unwrap(),
        Some(normal)
    );
    assert_eq!(
        reopened.load(committed, ScreenBuffer::Alternate).unwrap(),
        Some(alternate)
    );
}

#[test]
fn exact_writer_name_grammar_is_required_and_unknown_lookalikes_remain() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let final_name = row_name(target(3));
    let exact = temporary_name(&final_name, "123", "456");
    create_private_file(&fixture.path().join(&exact), b"orphan");

    let uppercase_target = fields(TargetRef {
        generation: 0xabcdef,
        ..target(3)
    })
    .replacen('a', "A", 1);
    let unknown = vec![
        temporary_name(&final_name, "0", "1"),
        temporary_name(&final_name, "01", "1"),
        temporary_name(&final_name, "123", "0"),
        temporary_name(&final_name, "123", "0456"),
        temporary_name(&final_name, "+123", "456"),
        temporary_name(&final_name, "4294967296", "456"),
        temporary_name(&final_name, "123", "18446744073709551616"),
        format!(".{final_name}-123-456.tmp.extra"),
        format!(".{uppercase_target}.rows-123-456.tmp"),
        format!("..{final_name}-123-456.tmp"),
    ];
    for name in &unknown {
        create_private_file(&fixture.path().join(name), b"preserve");
    }
    let non_utf8 = std::ffi::OsString::from_vec(vec![b'.', 0xff, b'-', b'1']);
    let non_utf8_path = fixture.path().join(&non_utf8);
    let non_utf8_created = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&non_utf8_path)
    {
        Ok(mut file) => {
            file.write_all(b"preserve").unwrap();
            true
        }
        // APFS may reject byte names which are not valid UTF-8.
        Err(error) if error.raw_os_error() == Some(libc::EILSEQ) => false,
        Err(error) => panic!("create non-UTF8 filename fixture: {error}"),
    };

    assert_eq!(store.cleanup_temporary_files().unwrap(), 1);
    assert!(!fixture.path().join(exact).exists());
    for name in unknown {
        assert!(fixture.path().join(name).exists());
    }
    if non_utf8_created {
        assert!(fixture.path().join(non_utf8).exists());
    }
}

#[test]
fn unsafe_recognized_candidates_abort_before_any_safe_temp_is_removed() {
    for kind in [
        "symlink",
        "hardlink",
        "fifo",
        "directory",
        "mode",
        "oversize",
    ] {
        let fixture = Fixture::new();
        let store = HistoryStore::open(fixture.path()).unwrap();
        let safe_name = temporary_name(&row_name(target(3)), "31", "1");
        let unsafe_name = temporary_name(&screen_name(target(4), ScreenBuffer::Normal), "32", "2");
        create_private_file(&fixture.path().join(&safe_name), b"safe candidate");
        let unsafe_path = fixture.path().join(&unsafe_name);
        match kind {
            "symlink" => {
                let referent = fixture.path().join("referent");
                create_private_file(&referent, b"do not follow");
                std::os::unix::fs::symlink(&referent, &unsafe_path).unwrap();
            }
            "hardlink" => {
                let referent = fixture.path().join("referent");
                create_private_file(&referent, b"two links");
                fs::hard_link(&referent, &unsafe_path).unwrap();
            }
            "fifo" => {
                let path = CString::new(unsafe_path.as_os_str().as_bytes()).unwrap();
                // O_NONBLOCK in the cleaner must make opening this FIFO safe.
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "directory" => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&unsafe_path)
                    .unwrap();
            }
            "mode" => {
                create_private_file(&unsafe_path, b"wrong mode");
                fs::set_permissions(&unsafe_path, fs::Permissions::from_mode(0o640)).unwrap();
            }
            "oversize" => {
                let oversized = vec![b'x'; screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES + 1];
                create_private_file(&unsafe_path, &oversized);
            }
            _ => unreachable!(),
        }

        assert!(store.cleanup_temporary_files().is_err(), "unsafe {kind}");
        assert!(
            fixture.path().join(&safe_name).exists(),
            "safe temp after {kind}"
        );
        assert!(unsafe_path.exists(), "unsafe candidate after {kind}");
    }
}

#[test]
fn replaced_or_moved_named_history_directory_is_denied() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let temporary = temporary_name(&row_name(target(3)), "41", "1");
    create_private_file(&fixture.path().join(&temporary), b"partial");
    let moved = fixture.path().with_extension("moved");
    fs::rename(fixture.path(), &moved).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.path())
        .unwrap();

    assert!(store.cleanup_temporary_files().is_err());
    assert!(moved.join(&temporary).exists());
    assert!(fs::read_dir(fixture.path()).unwrap().next().is_none());

    drop(store);
    fs::remove_dir_all(fixture.path()).unwrap();
    fs::rename(moved, fixture.path()).unwrap();
}

#[test]
fn scan_entry_bound_fails_before_deleting_candidates() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    let temporary = temporary_name(&row_name(target(3)), "51", "1");
    create_private_file(&fixture.path().join(&temporary), b"candidate");
    for index in 0..16_384 {
        create_private_file(
            &fixture.path().join(format!("unknown-{index}")),
            b"preserve",
        );
    }

    assert!(store.cleanup_temporary_files().is_err());
    assert!(fixture.path().join(temporary).exists());
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 16_385);
}

#[test]
fn candidate_count_bound_fails_before_deleting_candidates() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(fixture.path()).unwrap();
    for session in 1..=4097 {
        let temporary = temporary_name(&row_name(target(session)), "61", &session.to_string());
        create_private_file(&fixture.path().join(temporary), b"candidate");
    }

    assert!(store.cleanup_temporary_files().is_err());
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 4097);
}
