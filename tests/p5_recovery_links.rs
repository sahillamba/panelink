#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::recovery_links::{
    LinkOutcome, LinkStore, RelaunchLink, LINK_RETENTION_MS, MAX_LINKS,
};
use std::fs;
use std::io;
use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir();
        for _ in 0..32 {
            let path = base.join(format!(
                "panelink-recovery-links-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(fs::canonicalize(path).unwrap()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private relaunch-link fixture: {error}"),
            }
        }
        panic!("could not allocate a unique relaunch-link fixture")
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn links_dir(&self) -> PathBuf {
        self.path().join("links")
    }

    fn store(&self) -> LinkStore {
        LinkStore::prepare(self.path()).unwrap()
    }

    fn file(&self) -> PathBuf {
        self.links_dir().join("links.bin")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target(generation: u64, workspace: u64, pane: u64) -> TargetRef {
    TargetRef {
        generation,
        workspace,
        pane,
        session: pane + 100,
        occupant_revision: pane + 200,
    }
}

fn link(index: u64, linked_at_unix_ms: u64) -> RelaunchLink {
    RelaunchLink {
        source_generation: 1,
        source_sequence: 2 + index,
        old_target: target(1, 9, 10 + index),
        new_target: target(2, 9, 110 + index),
        outcome: if index.is_multiple_of(2) {
            LinkOutcome::Applied
        } else {
            LinkOutcome::Unknown
        },
        linked_at_unix_ms,
    }
}

#[test]
fn persisted_records_round_trip_one_and_two_links_and_filter_by_old_target() {
    let fixture = Fixture::new();
    let store = fixture.store();
    assert!(store.load().unwrap().is_empty());

    let first = link(0, 1_000);
    store.record(first, 1_000).unwrap();
    assert_eq!(store.load().unwrap(), vec![first]);
    assert_eq!(store.links_for_old(first.old_target).unwrap(), vec![first]);

    let second = link(1, 1_001);
    store.record(second, 1_001).unwrap();
    assert_eq!(store.load().unwrap(), vec![first, second]);
    assert_eq!(
        store.links_for_old(second.old_target).unwrap(),
        vec![second]
    );
}

#[test]
fn identical_retry_is_idempotent_and_conflicting_link_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let original = link(0, 1_000);
    store.record(original, 1_000).unwrap();
    let committed = fs::read(fixture.file()).unwrap();

    store.record(original, 1_001).unwrap();
    assert_eq!(fs::read(fixture.file()).unwrap(), committed);

    let conflict = RelaunchLink {
        outcome: LinkOutcome::Unknown,
        ..original
    };
    assert!(store.record(conflict, 1_001).is_err());
    assert_eq!(fs::read(fixture.file()).unwrap(), committed);
}

#[test]
fn multiple_generations_for_one_old_target_are_distinct_links() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let first = link(0, 1_000);
    let second = RelaunchLink {
        new_target: target(3, 9, 310),
        linked_at_unix_ms: 1_001,
        ..first
    };
    store.record(first, 1_000).unwrap();
    store.record(second, 1_001).unwrap();
    assert_eq!(
        store.links_for_old(first.old_target).unwrap(),
        vec![first, second]
    );
}

#[test]
fn same_generation_old_and_new_targets_are_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bad = RelaunchLink {
        new_target: target(1, 9, 310),
        ..link(0, 1_000)
    };
    assert!(store.record(bad, 1_000).is_err());
    assert!(store.load().unwrap().is_empty());
    assert!(!fixture.file().exists());
}

#[test]
fn zero_fields_and_workspace_scope_mismatch_are_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let valid = link(0, 1_000);
    let invalid_links = [
        RelaunchLink {
            source_generation: 0,
            ..valid
        },
        RelaunchLink {
            source_sequence: 0,
            ..valid
        },
        RelaunchLink {
            linked_at_unix_ms: 0,
            ..valid
        },
        RelaunchLink {
            old_target: TargetRef {
                pane: 0,
                ..valid.old_target
            },
            ..valid
        },
        RelaunchLink {
            new_target: TargetRef {
                occupant_revision: 0,
                ..valid.new_target
            },
            ..valid
        },
        RelaunchLink {
            new_target: target(2, 10, 110),
            ..valid
        },
    ];
    for invalid in invalid_links {
        assert!(store.record(invalid, 1_000).is_err());
    }
    assert!(store.load().unwrap().is_empty());
    assert!(!fixture.file().exists());
}

#[test]
fn retention_prunes_links_older_than_thirty_days() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let old = link(0, 10_000);
    store.record(old, old.linked_at_unix_ms).unwrap();
    let now = old.linked_at_unix_ms + LINK_RETENTION_MS;
    let fresh = link(1, now);
    store.record(fresh, now).unwrap();
    assert_eq!(store.load().unwrap(), vec![fresh]);
}

#[test]
fn cap_keeps_newest_links() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for index in 0..=MAX_LINKS as u64 {
        let record = link(index, 10_000 + index);
        store.record(record, record.linked_at_unix_ms).unwrap();
    }
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), MAX_LINKS);
    assert_eq!(loaded.first().unwrap(), &link(1, 10_001));
    assert_eq!(
        loaded.last().unwrap(),
        &link(MAX_LINKS as u64, 10_000 + MAX_LINKS as u64)
    );
    assert!(store
        .links_for_old(link(0, 10_000).old_target)
        .unwrap()
        .is_empty());
}

#[test]
fn corrupt_short_trailing_and_oversized_files_are_refused_without_rewrite() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let record = link(0, 1_000);
    store.record(record, 1_000).unwrap();
    let valid = fs::read(fixture.file()).unwrap();
    let cases = [
        valid[..valid.len() - 1].to_vec(),
        [valid.as_slice(), &[0x7f]].concat(),
        {
            let mut bytes = valid.clone();
            bytes[0] ^= 1;
            bytes
        },
        vec![0; MAX_LINKS * 200 + 1],
    ];
    for corrupted in cases {
        fs::write(fixture.file(), &corrupted).unwrap();
        assert!(store.load().is_err());
        assert!(store.links_for_old(record.old_target).is_err());
        assert_eq!(fs::read(fixture.file()).unwrap(), corrupted);
    }
}

#[test]
fn unsafe_directory_modes_and_symlinks_are_refused() {
    let fixture = Fixture::new();
    let unsafe_dir = fixture.path().join("unsafe-dir");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&unsafe_dir)
        .unwrap();
    fs::set_permissions(&unsafe_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(LinkStore::open(&unsafe_dir).is_err());

    let real_dir = fixture.path().join("real-dir");
    fs::DirBuilder::new().mode(0o700).create(&real_dir).unwrap();
    let directory_symlink = fixture.path().join("directory-symlink");
    symlink(&real_dir, &directory_symlink).unwrap();
    assert!(LinkStore::open(&directory_symlink).is_err());
}

#[test]
fn unsafe_symlink_hardlink_and_fifo_files_are_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let link = link(0, 1_000);
    store.record(link, 1_000).unwrap();
    let file = fixture.file();
    let bytes = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();

    let symlink_target = fixture.path().join("symlink-target");
    fs::write(&symlink_target, &bytes).unwrap();
    symlink(&symlink_target, &file).unwrap();
    assert!(store.load().is_err());
    fs::remove_file(&file).unwrap();

    fs::write(&file, &bytes).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(store.load().is_err());
    fs::remove_file(&file).unwrap();

    fs::write(&file, &bytes).unwrap();
    let hardlink = fixture.path().join("hardlink");
    fs::hard_link(&file, &hardlink).unwrap();
    assert_eq!(fs::metadata(&file).unwrap().nlink(), 2);
    assert!(store.load().is_err());
    fs::remove_file(&file).unwrap();
    fs::remove_file(&hardlink).unwrap();

    let status = Command::new("/usr/bin/mkfifo")
        .arg(&file)
        .status()
        .expect("run system mkfifo utility");
    assert!(status.success());
    assert!(store.load().is_err());
}

#[test]
fn open_never_creates_directory_or_file() {
    let fixture = Fixture::new();
    let absent = fixture.path().join("absent");
    assert!(LinkStore::open(&absent).is_err());
    assert!(!absent.exists());

    let store = fixture.store();
    assert!(!fixture.file().exists());
    assert!(store.load().unwrap().is_empty());
    assert!(!fixture.file().exists());
}

#[test]
fn persisted_bytes_contain_only_fixed_magic_and_numeric_record_fields() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.record(link(0, 1_000), 1_000).unwrap();
    let bytes = fs::read(fixture.file()).unwrap();
    assert_eq!(&bytes[..8], b"PLLNK001");
    assert_eq!(bytes.len(), 12 + 105);
    for forbidden in [
        b"/Users/".as_slice(),
        b"provider_session",
        b"credential",
        b"argv",
    ] {
        assert!(!bytes
            .windows(forbidden.len())
            .any(|window| window == forbidden));
    }
}

#[test]
fn debug_output_redacts_target_and_generation_values() {
    let value = RelaunchLink {
        source_generation: 987_654_321,
        source_sequence: 876_543_210,
        old_target: target(987_654_321, 765_432_109, 654_321_098),
        new_target: target(123_456_789, 765_432_109, 543_210_987),
        outcome: LinkOutcome::Applied,
        linked_at_unix_ms: 112_233_445_566,
    };
    let debug = format!("{value:?}");
    for secret in ["987654321", "765432109", "112233445566"] {
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("Applied"));

    let fixture = Fixture::new();
    let store = fixture.store();
    let debug = format!("{store:?}");
    assert!(!debug.contains(fixture.path().to_str().unwrap()));
    assert!(debug.contains("redacted"));
}
