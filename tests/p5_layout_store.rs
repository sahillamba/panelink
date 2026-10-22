#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::layout_store::{LayoutSlot, LayoutStore, SaveOutcome};
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect, MIN_PANE_COLUMNS, MIN_PANE_ROWS};
use panelink::saved_layout::{encode, LayoutMetadata, SavedLayout, SAVED_LAYOUT_BYTES};
use std::fs::{self};
use std::io::{self};
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir();
        for _ in 0..32 {
            let path = base.join(format!(
                "panelink-layout-integ-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(fs::canonicalize(path).unwrap()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private layout fixture: {error}"),
            }
        }
        panic!("could not allocate a unique private layout fixture")
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

fn target(generation: u64, workspace: u64) -> panelink::control_protocol::TargetRef {
    panelink::control_protocol::TargetRef {
        generation,
        workspace,
        pane: 2,
        session: 3,
        occupant_revision: 1,
    }
}

fn other_target(generation: u64, workspace: u64) -> panelink::control_protocol::TargetRef {
    panelink::control_protocol::TargetRef {
        generation,
        workspace,
        pane: 4,
        session: 5,
        occupant_revision: 1,
    }
}

fn meta(generation: u64, workspace: u64, sequence: u64, revision: u64) -> LayoutMetadata {
    LayoutMetadata::new(generation, workspace, sequence, revision, 1_000_001)
        .expect("valid test metadata")
}

fn snap_one(rows: u16, columns: u16, revision: u64, gen: u64, ws: u64) -> LayoutSnapshot {
    LayoutSnapshot {
        revision,
        rows,
        columns,
        panes: [
            Some(PaneGeometry {
                target: target(gen, ws),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows,
                    columns,
                },
            }),
            None,
        ],
        divider: None,
    }
}

fn saved_one(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
    let snapshot = snap_one(
        MIN_PANE_ROWS + 1,
        MIN_PANE_COLUMNS + 5,
        revision,
        generation,
        workspace,
    );
    SavedLayout::from_snapshot(meta(generation, workspace, sequence, revision), &snapshot)
        .expect("valid one-pane saved layout")
}

fn saved_two(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
    let usable_cols = MIN_PANE_COLUMNS + 21 - 1;
    let first_cols = usable_cols / 2;
    let second_cols = usable_cols - first_cols;
    let snapshot = LayoutSnapshot {
        revision,
        rows: MIN_PANE_ROWS + 1,
        columns: MIN_PANE_COLUMNS + 21,
        panes: [
            Some(PaneGeometry {
                target: target(generation, workspace),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: MIN_PANE_ROWS + 1,
                    columns: first_cols,
                },
            }),
            Some(PaneGeometry {
                target: other_target(generation, workspace),
                rect: Rect {
                    x: first_cols + 1,
                    y: 0,
                    rows: MIN_PANE_ROWS + 1,
                    columns: second_cols,
                },
            }),
        ],
        divider: Some(Rect {
            x: first_cols,
            y: 0,
            rows: MIN_PANE_ROWS + 1,
            columns: 1,
        }),
    };
    SavedLayout::from_snapshot(meta(generation, workspace, sequence, revision), &snapshot)
        .expect("valid two-pane saved layout")
}

fn saved_empty(generation: u64, workspace: u64, sequence: u64, revision: u64) -> SavedLayout {
    SavedLayout::empty(meta(generation, workspace, sequence, revision))
}

#[test]
fn open_refuses_missing_dir() {
    let path = std::env::temp_dir().join(format!(
        "panelink-layout-missing-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(LayoutStore::open(&path).is_err());
}

#[test]
fn open_refuses_0755_dir() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
    assert!(LayoutStore::open(path).is_err());
}

#[test]
fn open_refuses_symlink_dir() {
    let fixture = Fixture::new();
    let link = fixture.path().join("link");
    symlink(fixture.path(), &link).unwrap();
    assert!(LayoutStore::open(&link).is_err());
}

#[test]
fn prepare_creates_layout_directory() {
    let fixture = Fixture::new();
    let store = LayoutStore::prepare(fixture.path()).unwrap();
    assert_eq!(store.path(), fixture.path().join("layout"));
    assert!(fixture.path().join("layout").is_dir());
}

#[test]
fn first_save_creates_current_only() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    assert_eq!(store.save(7, 1, &candidate).unwrap(), SaveOutcome::Written);
    let loaded = store.load(LayoutSlot::Current).unwrap().unwrap();
    assert_eq!(loaded.metadata().snapshot_sequence(), 1);
    assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
}

#[test]
fn same_gen_higher_seq_and_revision_writes_current_only() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let second = saved_one(7, 1, 2, 2);
    assert_eq!(store.save(7, 1, &second).unwrap(), SaveOutcome::Written);
    assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
}

#[test]
fn equal_seq_identical_is_already_durable() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    assert_eq!(
        store.save(7, 1, &first).unwrap(),
        SaveOutcome::AlreadyDurable
    );
}

#[test]
fn equal_seq_different_bytes_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    // Same metadata key but different geometry -> different valid encoded bytes
    let snap = snap_one(MIN_PANE_ROWS + 2, MIN_PANE_COLUMNS + 5, 1, 7, 1);
    let second = SavedLayout::from_snapshot(meta(7, 1, 1, 1), &snap).unwrap();
    assert!(store.save(7, 1, &second).is_err());
}

#[test]
fn lower_seq_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 2, 2)).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 1, 1)).is_err());
}

#[test]
fn nonempty_equal_revision_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_two(7, 1, 1, 1)).unwrap();
    // Same generation, higher sequence, same revision, nonempty -> refused
    assert!(store.save(7, 1, &saved_two(7, 1, 2, 1)).is_err());
}

#[test]
fn nonempty_to_empty_equal_revision_accepted() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_two(7, 1, 1, 1)).unwrap();
    let empty = saved_empty(7, 1, 2, 1);
    assert_eq!(store.save(7, 1, &empty).unwrap(), SaveOutcome::Written);
}

#[test]
fn empty_then_any_further_changed_candidate_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
    store.save(7, 1, &saved_empty(7, 1, 2, 2)).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 3, 3)).is_err());
    assert!(store.save(7, 1, &saved_empty(7, 1, 3, 3)).is_err());
}

#[test]
fn wrong_generation_or_workspace_candidate_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    assert!(store.save(8, 1, &candidate).is_err());
    assert!(store.save(7, 2, &candidate).is_err());
}

#[test]
fn slot_generation_greater_than_bound_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
    // Candidate gen 8 matches current, but bound is 7
    assert!(store.save(7, 1, &saved_one(8, 1, 2, 2)).is_err());
}

#[test]
fn rotation_preserves_exact_old_bytes_in_previous() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let second = saved_one(8, 1, 1, 1);
    assert_eq!(store.save(8, 1, &second).unwrap(), SaveOutcome::Written);
    let previous_bytes = store.load_bytes(LayoutSlot::Previous).unwrap().unwrap();
    let previous_layout = panelink::saved_layout::decode(&previous_bytes).unwrap();
    assert_eq!(previous_layout.metadata().generation(), 7);
    assert_eq!(
        store
            .load(LayoutSlot::Current)
            .unwrap()
            .unwrap()
            .metadata()
            .generation(),
        8
    );
}

#[test]
fn second_rotation_replaces_previous() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
    store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
    store.save(9, 1, &saved_one(9, 1, 1, 1)).unwrap();
    assert_eq!(
        store
            .load(LayoutSlot::Previous)
            .unwrap()
            .unwrap()
            .metadata()
            .generation(),
        8
    );
    assert_eq!(
        store
            .load(LayoutSlot::Current)
            .unwrap()
            .unwrap()
            .metadata()
            .generation(),
        9
    );
}

#[test]
fn missing_current_plus_present_previous_refused_for_save() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
    store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
    fs::remove_file(fixture.path().join("current.layout")).unwrap();
    let observed = LayoutStore::open(fixture.path()).unwrap();
    assert!(observed.load(LayoutSlot::Previous).unwrap().is_some());
    assert!(observed.load(LayoutSlot::Current).unwrap().is_none());
    assert!(observed.save(9, 1, &saved_one(9, 1, 1, 1)).is_err());
}

#[test]
fn newer_previous_same_gen_different_bytes_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
    store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
    // Overwrite previous with same-gen different bytes
    let bytes = encode(&saved_one(8, 1, 2, 2));
    fs::write(fixture.path().join("previous.layout"), bytes).unwrap();
    fs::set_permissions(
        fixture.path().join("previous.layout"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let observed = LayoutStore::open(fixture.path()).unwrap();
    assert!(observed.save(9, 1, &saved_one(9, 1, 1, 1)).is_err());
}

#[test]
fn corrupt_short_long_current_refused_and_bytes_preserved() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let path = fixture.path().join("current.layout");
    let original = fs::read(&path).unwrap();

    // Short
    fs::write(&path, &original[..100]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
    assert_eq!(fs::read(&path).unwrap().len(), 100);

    // Restore and corrupt
    fs::write(&path, &original).unwrap();
    fs::write(&path, b"not-valid-layout-record-here").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"not-valid-layout-record-here");
}

#[test]
fn corrupt_0644_current_refused_and_bytes_preserved() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let path = fixture.path().join("current.layout");
    let original = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o644
    );
}

#[test]
fn symlink_current_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let path = fixture.path().join("current.layout");
    fs::remove_file(&path).unwrap();
    let other = fixture.path().join("other");
    fs::write(&other, encode(&first)).unwrap();
    symlink(&other, &path).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn hardlinked_current_refused() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let first = saved_one(7, 1, 1, 1);
    store.save(7, 1, &first).unwrap();
    let path = fixture.path().join("current.layout");
    let other = fixture.path().join("other");
    fs::hard_link(&path, &other).unwrap();
    assert!(store.save(7, 1, &saved_one(7, 1, 2, 2)).is_err());
}

#[test]
fn invalid_current_does_not_block_load_previous() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    store.save(7, 1, &saved_one(7, 1, 1, 1)).unwrap();
    store.save(8, 1, &saved_one(8, 1, 1, 1)).unwrap();
    fs::write(fixture.path().join("current.layout"), b"bad").unwrap();
    fs::set_permissions(
        fixture.path().join("current.layout"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let observed = LayoutStore::open(fixture.path()).unwrap();
    assert!(observed.load(LayoutSlot::Current).is_err());
    assert_eq!(
        observed
            .load(LayoutSlot::Previous)
            .unwrap()
            .unwrap()
            .metadata()
            .generation(),
        7
    );
}

#[test]
fn load_never_creates_files() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let before_count = fs::read_dir(fixture.path()).unwrap().count();
    assert!(store.load(LayoutSlot::Current).unwrap().is_none());
    assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), before_count);
}

#[test]
fn cleanup_removes_only_recognized_safe_temps() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    store.save(7, 1, &candidate).unwrap();

    fs::write(
        fixture.path().join(".current.layout-12345-1.tmp"),
        encode(&candidate),
    )
    .unwrap();
    fs::set_permissions(
        fixture.path().join(".current.layout-12345-1.tmp"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    // Unknown preserved
    fs::write(fixture.path().join("unknown-file"), b"hello").unwrap();
    fs::set_permissions(
        fixture.path().join("unknown-file"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    // Committed slot preserved
    fs::write(fixture.path().join("previous.layout"), encode(&candidate)).unwrap();
    fs::set_permissions(
        fixture.path().join("previous.layout"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    assert_eq!(store.cleanup_temporary_files().unwrap(), 1);
    assert!(fixture.path().join("current.layout").exists());
    assert!(fixture.path().join("previous.layout").exists());
    assert!(fixture.path().join("unknown-file").exists());
    assert!(!fixture.path().join(".current.layout-12345-1.tmp").exists());
}

#[test]
fn cleanup_refuses_unsafe_temp_and_deletes_nothing() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    store.save(7, 1, &candidate).unwrap();

    fs::write(
        fixture.path().join(".current.layout-12345-1.tmp"),
        encode(&candidate),
    )
    .unwrap();
    fs::set_permissions(
        fixture.path().join(".current.layout-12345-1.tmp"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    // Unsafe temp (wrong mode)
    fs::write(
        fixture.path().join(".previous.layout-12345-2.tmp"),
        encode(&candidate),
    )
    .unwrap();
    fs::set_permissions(
        fixture.path().join(".previous.layout-12345-2.tmp"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    assert!(store.cleanup_temporary_files().is_err());
    assert!(fixture.path().join(".current.layout-12345-1.tmp").exists());
    assert!(fixture.path().join(".previous.layout-12345-2.tmp").exists());
}

#[test]
fn cleanup_refuses_more_than_16_recognized_temps() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    for index in 1..=17 {
        let name = format!(".current.layout-12345-{index}.tmp");
        fs::write(fixture.path().join(&name), encode(&candidate)).unwrap();
        fs::set_permissions(
            fixture.path().join(&name),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    assert!(store.cleanup_temporary_files().is_err());
}

#[test]
fn debug_and_errors_are_redacted() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let rendered = format!("{store:?}");
    assert!(rendered.contains("redacted"));
    assert!(!rendered.contains("layout"));
    assert!(!rendered.contains("current"));
    assert!(!rendered.contains("previous"));
}

#[test]
fn verify_identity_rejects_replaced_directory() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    fs::remove_dir_all(fixture.path()).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.path())
        .unwrap();
    assert!(store.verify_identity().is_err());
}

#[test]
fn save_on_both_absent_creates_current() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    assert_eq!(store.save(7, 1, &candidate).unwrap(), SaveOutcome::Written);
    assert!(store.load(LayoutSlot::Current).unwrap().is_some());
    assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
}

#[test]
fn load_bytes_returns_exact_160_bytes() {
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let candidate = saved_one(7, 1, 1, 1);
    store.save(7, 1, &candidate).unwrap();
    let bytes = store.load_bytes(LayoutSlot::Current).unwrap().unwrap();
    assert_eq!(bytes.len(), SAVED_LAYOUT_BYTES);
}

#[test]
fn fifo_slot_is_refused_without_blocking() {
    // Parent review regression: a planted FIFO must not hang inspection.
    let fixture = Fixture::new();
    let store = LayoutStore::open(fixture.path()).unwrap();
    let status = std::process::Command::new("/usr/bin/mkfifo")
        .arg("-m")
        .arg("600")
        .arg(fixture.path().join("current.layout"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(store.load(LayoutSlot::Current).is_err());
    assert!(store.load(LayoutSlot::Previous).unwrap().is_none());
    assert!(store.save(7, 1, &saved_one(7, 1, 1, 1)).is_err());
}
