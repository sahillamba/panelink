use panelink::workspace_index::codec::MetadataRecord;
use panelink::workspace_index::store::{SaveOutcome, StoreError, WorkspaceIndexStore};
use panelink::workspace_index::tab_visibility::{
    TabVisibilityCodecError, TabVisibilityRecord, TabVisibilityStore,
};
use panelink::workspace_index::{
    next_shell_name, plan_close_removal, ProjectId, TabPresentationSnapshot, WorkspaceIndex,
    WorkspaceIndexError, WorkspaceTabId, MAX_TABS_PER_PROJECT,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);
type MalformedCase = (fn(&mut [u8; 32]), TabVisibilityCodecError);

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "panelink-workspace-tab-state-{}-{}",
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

fn project(id: u64) -> ProjectId {
    ProjectId::new(id).unwrap()
}

fn tab(id: u64) -> WorkspaceTabId {
    WorkspaceTabId::new(id).unwrap()
}

fn legacy_index() -> WorkspaceIndex {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(project(1), "Default", tab(1), "Main")
        .unwrap();
    index
}

fn default_store(root: &Root) -> WorkspaceIndexStore {
    let mut store = WorkspaceIndexStore::open(&root.0).unwrap();
    store.initialize_legacy().unwrap();
    store
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn hidden_record_exact_bytes_round_trip_and_bounds() {
    let record = TabVisibilityRecord::new(1, true).unwrap();
    let bytes = record.encode();
    let expected = concat!(
        "504c574b48543031",
        "0001",
        "0020",
        "00000020",
        "0000000000000001",
        "01",
        "00000000000000"
    );
    assert_eq!(bytes.len(), 32);
    assert_eq!(hex(&bytes), expected);
    assert_eq!(TabVisibilityRecord::decode(&bytes), Ok(record));
    assert_eq!(
        TabVisibilityRecord::new(0, false),
        Err(TabVisibilityCodecError::InvalidRevision)
    );

    let cases: [MalformedCase; 7] = [
        (|bytes| bytes[0] ^= 1, TabVisibilityCodecError::InvalidMagic),
        (
            |bytes| bytes[9] = 2,
            TabVisibilityCodecError::UnsupportedVersion,
        ),
        (
            |bytes| bytes[11] = 31,
            TabVisibilityCodecError::InvalidHeaderLength,
        ),
        (
            |bytes| bytes[15] = 31,
            TabVisibilityCodecError::InvalidTotalLength,
        ),
        (
            |bytes| bytes[23] = 0,
            TabVisibilityCodecError::InvalidRevision,
        ),
        (
            |bytes| bytes[24] = 2,
            TabVisibilityCodecError::InvalidHiddenFlag,
        ),
        (
            |bytes| bytes[31] = 1,
            TabVisibilityCodecError::ReservedField,
        ),
    ];
    for (mutate, expected_error) in cases {
        let mut malformed = bytes;
        mutate(&mut malformed);
        assert_eq!(TabVisibilityRecord::decode(&malformed), Err(expected_error));
    }
    assert_eq!(
        TabVisibilityRecord::decode(&bytes[..31]),
        Err(TabVisibilityCodecError::InvalidLength)
    );
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert_eq!(
        TabVisibilityRecord::decode(&trailing),
        Err(TabVisibilityCodecError::InvalidLength)
    );
}

#[test]
fn legacy_v1_without_sidecar_migrates_all_tabs_visible() {
    let root = Root::new();
    let mut workspace = default_store(&root);
    let current = workspace.load().unwrap().unwrap();
    let mut index = current.index().clone();
    index
        .add_project(project(2), "Tools", tab(3), "Review")
        .unwrap();
    let expanded = MetadataRecord::new(index, 2, 4).unwrap();
    workspace.save(1, &expanded).unwrap();
    let v1_before = fs::read(root.0.join("workspace-index.v1")).unwrap();

    let mut tabs = TabVisibilityStore::new(&mut workspace);
    assert_eq!(tabs.load().unwrap(), None);
    let initialized = tabs.initialize().unwrap();
    assert_eq!((initialized.revision(), initialized.hidden()), (1, false));
    assert_eq!(
        fs::read(root.0.join("workspace-index.v1")).unwrap(),
        v1_before
    );

    let presentation = TabPresentationSnapshot::new(expanded.index(), false);
    assert_eq!(presentation.projects.len(), 2);
    assert_eq!(presentation.projects[0].tabs[0].route, route(1, 1));
    assert_eq!(presentation.projects[1].tabs[0].route, route(2, 3));
    assert_eq!(presentation.active_route, Some(route(1, 1)));
    let hidden = TabPresentationSnapshot::new(expanded.index(), true);
    assert_eq!(hidden.active_route, Some(route(2, 3)));
    let no_visible_route = TabPresentationSnapshot::new(&legacy_index(), true);
    assert!(no_visible_route.projects.is_empty());
    assert_eq!(no_visible_route.active_project, None);
    assert_eq!(no_visible_route.active_route, None);
}

#[test]
fn shell_names_skip_casefolded_collisions() {
    let mut index = legacy_index();
    index.add_tab(project(1), tab(2), "Shell 1").unwrap();
    index.add_tab(project(1), tab(3), "sHeLl 2").unwrap();
    assert_eq!(next_shell_name(&index, project(1)), "Shell 3");
}

#[test]
fn eight_tabs_capacity_and_close_releases_slot() {
    let mut index = legacy_index();
    for id in 2..=MAX_TABS_PER_PROJECT as u64 {
        index
            .add_tab(project(1), tab(id), &format!("Tab {id}"))
            .unwrap();
    }
    assert_eq!(
        index.add_tab(project(1), tab(9), "Ninth"),
        Err(WorkspaceIndexError::TabLimitReached)
    );
    let mut index = plan_close_removal(&index, project(1), tab(8)).unwrap();
    index.add_tab(project(1), tab(9), "Replacement").unwrap();
    assert_eq!(
        index.project(project(1)).unwrap().tabs().len(),
        MAX_TABS_PER_PROJECT
    );
}

#[test]
fn close_selected_last_project_tab_uses_valid_route() {
    let mut index = legacy_index();
    index
        .add_project(project(2), "Tools", tab(3), "Only")
        .unwrap();
    index.select_project(project(2)).unwrap();
    let next = plan_close_removal(&index, project(2), tab(3)).unwrap();
    assert_eq!(next.active_project_id(), Some(project(1)));
    assert_eq!(next.active_tab_id(), Some(tab(1)));
    assert_eq!(next.project(project(2)).unwrap().tabs().len(), 0);
    MetadataRecord::new(next, 2, 4).unwrap();
    assert_eq!(
        plan_close_removal(&legacy_index(), project(1), tab(1)),
        Err(WorkspaceIndexError::ReservedRouteRequiresSidecar)
    );
}

#[test]
fn sidecar_compare_and_swap_conflict_and_retry() {
    let root = Root::new();
    let mut workspace = default_store(&root);
    let mut tabs = TabVisibilityStore::new(&mut workspace);
    tabs.initialize().unwrap();
    assert_eq!(tabs.save_hidden(1, true), Ok(SaveOutcome::Written));
    assert_eq!(tabs.load().unwrap().unwrap().revision(), 2);
    assert_eq!(
        tabs.save_hidden(1, false),
        Err(StoreError::RevisionConflict)
    );
    assert_eq!(tabs.save_hidden(1, true), Ok(SaveOutcome::AlreadyDurable));
    assert_eq!(tabs.save_hidden(2, false), Ok(SaveOutcome::Written));
    assert_eq!(tabs.load().unwrap().unwrap().revision(), 3);
}

#[test]
fn corrupt_sidecar_fails_closed() {
    let root = Root::new();
    let mut workspace = default_store(&root);
    let path = root.0.join("workspace-tabs.v1");
    fs::write(&path, b"broken").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut tabs = TabVisibilityStore::new(&mut workspace);
    assert_eq!(tabs.load(), Err(StoreError::CorruptRecord));
    assert_eq!(fs::read(path).unwrap(), b"broken");
}

fn route(project_id: u64, tab_id: u64) -> panelink::workspace_tabs::WorkspaceRouteId {
    panelink::workspace_tabs::WorkspaceRouteId {
        project: project(project_id),
        tab: tab(tab_id),
    }
}
