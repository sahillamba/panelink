use panelink::workspace_index::codec::MetadataRecord;
use panelink::workspace_index::store::{SaveOutcome, StoreError, WorkspaceIndexStore};
use panelink::workspace_index::{ProjectId, WorkspaceIndex, WorkspaceTabId};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "panelink-workspace-store-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn default_record() -> MetadataRecord {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(
            ProjectId::new(1).unwrap(),
            "Default",
            WorkspaceTabId::new(1).unwrap(),
            "Main",
        )
        .unwrap();
    MetadataRecord::new(index, 1, 2).unwrap()
}

fn renamed_record(revision: u64, next_id: u64, name: &str) -> MetadataRecord {
    let mut index = default_record().index().clone();
    index
        .rename_project(ProjectId::new(1).unwrap(), name)
        .unwrap();
    MetadataRecord::new(index, revision, next_id).unwrap()
}

#[test]
fn child_process_lock_probe() {
    let Ok(path) = std::env::var("PANELINK_STORE_LOCK_PROBE") else {
        return;
    };
    assert!(matches!(
        WorkspaceIndexStore::open(path),
        Err(StoreError::Locked)
    ));
}

#[test]
fn migration_is_additive_and_same_process_and_child_cannot_reopen() {
    let root = Root::new();
    let legacy = root.0.join("server");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("journal"), b"preserve-me\0").unwrap();
    let before = fs::read(legacy.join("journal")).unwrap();

    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    assert_eq!(store.initialize_legacy().unwrap(), SaveOutcome::Written);
    let record = store.load().unwrap().unwrap();
    assert_eq!((record.revision(), record.next_id()), (1, 2));
    let project = record.index().project(ProjectId::new(1).unwrap()).unwrap();
    assert_eq!(project.display_name(), "Default");
    assert_eq!(project.tabs().next().unwrap().display_name, "Main");
    assert_eq!(fs::read(legacy.join("journal")).unwrap(), before);
    assert_eq!(
        fs::metadata(root.0.join("workspace-index.v1"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(root.0.join("workspace-index.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(matches!(
        WorkspaceIndexStore::open(&root.0),
        Err(StoreError::Locked)
    ));

    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_process_lock_probe")
        .arg("--nocapture")
        .env("PANELINK_STORE_LOCK_PROBE", &root.0)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn lost_ack_retry_is_idempotent_but_stale_revision_is_rejected() {
    let root = Root::new();
    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    store.initialize_legacy().unwrap();
    let next = renamed_record(2, 2, "Renamed");
    assert_eq!(store.save(1, &next).unwrap(), SaveOutcome::Written);
    assert_eq!(store.save(1, &next).unwrap(), SaveOutcome::AlreadyDurable);
    assert_eq!(
        store.save(1, &renamed_record(3, 2, "Other")),
        Err(StoreError::RevisionConflict)
    );
    assert_eq!(store.load().unwrap().unwrap(), next);
}

#[test]
fn corrupt_bytes_and_unsafe_registry_objects_are_preserved_and_rejected() {
    let root = Root::new();
    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    store.initialize_legacy().unwrap();
    drop(store);
    let registry = root.0.join("workspace-index.v1");
    let valid = fs::read(&registry).unwrap();
    fs::write(&registry, b"corrupt-but-preserve").unwrap();
    let corrupt = fs::read(&registry).unwrap();
    let store = WorkspaceIndexStore::open(&root.0).unwrap();
    assert_eq!(store.load(), Err(StoreError::CorruptRecord));
    assert_eq!(fs::read(&registry).unwrap(), corrupt);
    drop(store);

    fs::write(&registry, valid).unwrap();
    drop(WorkspaceIndexStore::open(&root.0).unwrap());
    fs::remove_file(&registry).unwrap();
    std::os::unix::fs::symlink("server", &registry).unwrap();
    let store = WorkspaceIndexStore::open(&root.0).unwrap();
    assert!(matches!(store.load(), Err(StoreError::UnsafeFile)));
}

#[test]
fn root_replacement_is_detected_and_never_recreated_or_redirected() {
    let root = Root::new();
    let store = WorkspaceIndexStore::open(&root.0).unwrap();
    let moved = root.0.with_extension("moved");
    fs::rename(&root.0, &moved).unwrap();
    fs::create_dir(&root.0).unwrap();
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(store.load(), Err(StoreError::UnsafeRoot)));
    fs::remove_dir_all(&root.0).unwrap();
    fs::rename(moved, &root.0).unwrap();
}

#[test]
fn store_never_accepts_a_hardlinked_registry() {
    let root = Root::new();
    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    store.initialize_legacy().unwrap();
    drop(store);
    let registry = root.0.join("workspace-index.v1");
    let alias = root.0.join("unrelated");
    fs::hard_link(&registry, &alias).unwrap();
    let store = WorkspaceIndexStore::open(&root.0).unwrap();
    assert!(matches!(store.load(), Err(StoreError::UnsafeFile)));
    assert!(registry.exists() && alias.exists());
}

#[test]
fn root_and_record_permissions_are_strict_and_symlinked_roots_are_refused() {
    let root = Root::new();
    drop(WorkspaceIndexStore::open(&root.0).unwrap());
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(matches!(
        WorkspaceIndexStore::open(&root.0),
        Err(StoreError::UnsafeRoot)
    ));
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    let lock = root.0.join("workspace-index.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        WorkspaceIndexStore::open(&root.0),
        Err(StoreError::UnsafeFile)
    ));
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();

    let alias = root.0.with_extension("alias");
    std::os::unix::fs::symlink(&root.0, &alias).unwrap();
    assert!(matches!(
        WorkspaceIndexStore::open(alias),
        Err(StoreError::UnsafeRoot)
    ));
}

#[test]
fn allocator_watermark_prevents_removed_id_reuse_and_tabs_cannot_change_project() {
    let root = Root::new();
    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    store.initialize_legacy().unwrap();
    // The fixture begins with the required legacy record, so advance through
    // the normal API using only IDs at or above its watermark.
    let current = store.load().unwrap().unwrap();
    let mut live = current.index().clone();
    live.add_project(
        ProjectId::new(2).unwrap(),
        "Second",
        WorkspaceTabId::new(2).unwrap(),
        "Other",
    )
    .unwrap();
    let expanded = MetadataRecord::new(live, 2, 3).unwrap();
    store.save(1, &expanded).unwrap();

    let mut moved = expanded.index().clone();
    moved
        .remove_tab(ProjectId::new(2).unwrap(), WorkspaceTabId::new(2).unwrap())
        .unwrap();
    moved
        .add_tab(
            ProjectId::new(1).unwrap(),
            WorkspaceTabId::new(2).unwrap(),
            "Moved",
        )
        .unwrap();
    let moved = MetadataRecord::new(moved, 3, 3).unwrap();
    assert_eq!(store.save(2, &moved), Err(StoreError::InvalidTransition));

    let mut removed = expanded.index().clone();
    removed
        .remove_tab(ProjectId::new(2).unwrap(), WorkspaceTabId::new(2).unwrap())
        .unwrap();
    removed.remove_project(ProjectId::new(2).unwrap()).unwrap();
    let removed = MetadataRecord::new(removed, 3, 3).unwrap();
    store.save(2, &removed).unwrap();

    let mut reused = removed.index().clone();
    reused
        .add_project(
            ProjectId::new(2).unwrap(),
            "Reused",
            WorkspaceTabId::new(3).unwrap(),
            "New tab",
        )
        .unwrap();
    let reused = MetadataRecord::new(reused, 4, 4).unwrap();
    assert_eq!(store.save(3, &reused), Err(StoreError::InvalidTransition));

    let mut lowered = removed.index().clone();
    lowered
        .rename_project(ProjectId::new(1).unwrap(), "Lowered")
        .unwrap();
    let lowered = MetadataRecord::new(lowered, 4, 2).unwrap();
    assert_eq!(store.save(3, &lowered), Err(StoreError::InvalidTransition));
    assert_eq!(store.load().unwrap().unwrap(), removed);
}
