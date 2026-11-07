#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::client_context::ContextFile;
use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::layout_store::LayoutStore;
use panelink::recovery_links::LinkStore;
use panelink::server_storage::ServerStorage;
use panelink::transport::prepare_directory;
use panelink::workspace_index::store::WorkspaceIndexStore;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/panelink-acl-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn child(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // A deny-delete ACL is intentional test data. Clear only this owned
        // fixture's ACLs before removing it; production never repairs ACLs.
        let output = Command::new("/bin/chmod")
            .arg("-RN")
            .arg(&self.0)
            .output()
            .unwrap();
        assert!(output.status.success(), "ACL fixture cleanup failed");
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn acl(path: &Path, entry: &str) {
    let output = Command::new("/bin/chmod")
        .args(["+a", entry])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "ACL fixture setup failed");
}

fn target() -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    }
}

#[test]
fn private_directories_reject_granting_acls_despite_mode_0700() {
    let fixture = Fixture::new();
    acl(&fixture.0, "everyone allow list,search");
    assert_eq!(fs::metadata(&fixture.0).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        prepare_directory(&fixture.0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        ServerStorage::initialize(&fixture.0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        HistoryStore::open(&fixture.0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(LayoutStore::open(&fixture.0).is_err());
    assert!(LinkStore::open(&fixture.0).is_err());
    assert!(WorkspaceIndexStore::open(&fixture.0).is_err());
    assert!(ContextFile::create(
        &fixture.0.join("caller.ctx"),
        1,
        1,
        Instant::now() + Duration::from_secs(1)
    )
    .is_err());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
}

#[test]
fn inherited_grants_are_refused_before_storage_payloads_are_written() {
    let fixture = Fixture::new();
    acl(
        &fixture.0,
        "everyone allow list,search,read,file_inherit,directory_inherit",
    );
    let child = fixture.child("inherited");
    assert_eq!(fs::metadata(&child).unwrap().mode() & 0o777, 0o700);
    assert!(
        ServerStorage::initialize(&child).is_err(),
        "inherited ACL accepted"
    );
    assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
    assert!(HistoryStore::open(&child).is_err());
}

#[test]
fn existing_saved_files_with_read_grants_are_refused_without_rewriting() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(&fixture.0).unwrap();
    store.begin_history(target(), 1, false).unwrap();
    let record = fs::read_dir(&fixture.0)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let before = fs::read(&record).unwrap();
    acl(&record, "everyone allow read");
    assert_eq!(fs::metadata(&record).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        store.load_rows(target()).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fs::read(&record).unwrap(), before);
}

#[test]
fn acl_changes_after_open_stop_new_history_writes() {
    let fixture = Fixture::new();
    let store = HistoryStore::open(&fixture.0).unwrap();
    acl(&fixture.0, "everyone allow read,file_inherit,only_inherit");
    assert!(store.begin_history(target(), 1, false).is_err());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
}

#[test]
fn writable_ancestor_acl_is_refused_even_with_a_private_final_directory() {
    let fixture = Fixture::new();
    let child = fixture.child("private");
    acl(
        &fixture.0,
        "everyone allow add_file,add_subdirectory,delete_child",
    );
    assert_eq!(fs::metadata(&fixture.0).unwrap().mode() & 0o777, 0o700);
    assert!(prepare_directory(&child).is_err());
    assert!(ServerStorage::initialize(&child).is_err());
    assert!(HistoryStore::open(&child).is_err());
}

#[test]
fn deny_only_acls_keep_private_storage_usable() {
    let fixture = Fixture::new();
    acl(&fixture.0, "everyone deny delete");
    prepare_directory(&fixture.0).unwrap();
    let store = HistoryStore::open(&fixture.0).unwrap();
    store.begin_history(target(), 1, false).unwrap();
    assert!(store.load_rows(target()).unwrap().is_some());
}

#[test]
fn owner_only_acl_grants_remain_usable() {
    let fixture = Fixture::new();
    let owner = Command::new("/usr/bin/id").arg("-un").output().unwrap();
    assert!(owner.status.success());
    let name = std::str::from_utf8(&owner.stdout).unwrap().trim();
    acl(
        &fixture.0,
        &format!("user:{name} allow read,write,search,file_inherit,directory_inherit"),
    );
    prepare_directory(&fixture.0).unwrap();
    let store = HistoryStore::open(&fixture.0).unwrap();
    store.begin_history(target(), 1, false).unwrap();
    assert!(store.load_rows(target()).unwrap().is_some());
}

#[test]
fn harmless_ancestor_read_grants_do_not_block_a_private_child() {
    let fixture = Fixture::new();
    let child = fixture.child("private");
    acl(&fixture.0, "everyone allow list,search,readattr");
    prepare_directory(&child).unwrap();
    let store = HistoryStore::open(&child).unwrap();
    store.begin_history(target(), 1, false).unwrap();
    assert!(store.load_rows(target()).unwrap().is_some());
}

#[test]
fn all_ancestor_mutation_rights_are_checked() {
    for permission in [
        "write",
        "delete",
        "append",
        "delete_child",
        "writeattr",
        "writeextattr",
        "writesecurity",
        "chown",
    ] {
        let fixture = Fixture::new();
        let file = fs::File::open(&fixture.0).unwrap();
        acl(&fixture.0, &format!("everyone allow {permission}"));
        assert!(
            panelink::private_acl::require_safe_ancestor(&file).is_err(),
            "accepted {permission}"
        );
    }
}

#[test]
fn later_acl_entries_are_checked_and_descriptor_stays_open() {
    let fixture = Fixture::new();
    acl(&fixture.0, "everyone deny delete");
    let file = fs::File::open(&fixture.0).unwrap();
    panelink::private_acl::require_private(&file).unwrap();
    acl(&fixture.0, "everyone allow read");
    assert!(panelink::private_acl::require_private(&file).is_err());
    assert!(file.metadata().unwrap().is_dir());
}

#[test]
fn context_files_reject_late_grants_without_replacing_records() {
    let fixture = Fixture::new();
    let path = fixture.0.join("caller.ctx");
    let deadline = || Instant::now() + Duration::from_secs(1);
    drop(ContextFile::create(&path, 1, 1, deadline()).unwrap());
    let before = fs::read(&path).unwrap();
    acl(&path, "everyone allow read,write");
    assert!(ContextFile::open(&path, 1, 1, deadline()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
