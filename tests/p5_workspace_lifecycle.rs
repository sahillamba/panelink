#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Private-root lifecycle adapter checks plus the plain-start empty boundary.

use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::{Environment, LaunchContext};
use panelink::workspace_index::codec::MetadataRecord;
use panelink::workspace_index::store::WorkspaceIndexStore;
use panelink::workspace_index::tab_visibility::TabVisibilityStore;
use panelink::workspace_index::{ProjectId, WorkspaceTabId};
use panelink::workspace_tabs::WorkspaceRouteId;

static NEXT: AtomicUsize = AtomicUsize::new(0);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

mod default_open {
    use super::*;

    pub(crate) struct StateRoot;
    pub(crate) struct RouteLock;

    pub(crate) static LIVE: AtomicBool = AtomicBool::new(false);
    pub(crate) static GENERATION: AtomicU64 = AtomicU64::new(8);
    pub(crate) static SPAWNS: AtomicUsize = AtomicUsize::new(0);
    pub(crate) static STOPS: AtomicUsize = AtomicUsize::new(0);

    pub(crate) fn reset() {
        LIVE.store(false, Ordering::Release);
        GENERATION.store(8, Ordering::Release);
        SPAWNS.store(0, Ordering::Release);
        STOPS.store(0, Ordering::Release);
    }

    pub(crate) fn ensure_route_server(
        _root: &Path,
        _route: WorkspaceRouteId,
        _launch: &LaunchContext,
        may_spawn: bool,
    ) -> Result<(), ()> {
        if may_spawn {
            SPAWNS.fetch_add(1, Ordering::AcqRel);
        }
        LIVE.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn lock_workspace_route(
        _route: WorkspaceRouteId,
    ) -> Result<(StateRoot, RouteLock), ()> {
        Ok((StateRoot, RouteLock))
    }

    pub(crate) fn live_server_generation(_state: &StateRoot) -> Result<Option<u64>, ()> {
        Ok(LIVE
            .load(Ordering::Acquire)
            .then(|| GENERATION.load(Ordering::Acquire)))
    }

    pub(crate) fn stop_workspace_locked(_state: &StateRoot) -> Result<(), ()> {
        STOPS.fetch_add(1, Ordering::AcqRel);
        LIVE.store(false, Ordering::Release);
        Ok(())
    }
}

#[path = "../src/workspace_lifecycle.rs"]
mod workspace_lifecycle;

/// The fake runtime above is process-global, so fixtures serialize: the guard
/// is held until after `Drop` cleanup finishes.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture(
    PathBuf,
    #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
);

impl Fixture {
    fn new() -> Self {
        let guard = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        default_open::reset();
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-p5-lifecycle-{}-{serial}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create private lifecycle root");
        Self(
            fs::canonicalize(root).expect("canonicalize private lifecycle root"),
            guard,
        )
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn initialize_default(&self) {
        let mut store = WorkspaceIndexStore::open(&self.0).expect("open private registry");
        store.initialize_legacy().expect("initialize Default/Main");
    }

    fn initialize_project(&self) {
        let mut store = WorkspaceIndexStore::open(&self.0).expect("open private registry");
        if store.load().expect("load metadata").is_none() {
            store.initialize_legacy().expect("initialize Default/Main");
        }
        let old = store.load().expect("load initialized metadata").unwrap();
        let mut index = old.index().clone();
        index
            .add_project(
                ProjectId::new(2).unwrap(),
                "Editors",
                WorkspaceTabId::new(3).unwrap(),
                "Main",
            )
            .expect("add test project");
        let next = MetadataRecord::new(index, old.revision() + 1, 4).unwrap();
        store
            .save(old.revision(), &next)
            .expect("save test project");
    }

    fn run_cli(&self, arguments: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(arguments)
            .env_clear()
            .env("PANELINK_HOME", &self.0)
            .env("HOME", &self.0)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start private CLI command");
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            if child.try_wait().expect("poll CLI command").is_some() {
                return child.wait_with_output().expect("collect CLI output");
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output().expect("reap timed-out command");
                panic!("CLI command exceeded deadline: {arguments:?}; {output:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let server = self.0.join("server");
        if server_lock_held(&server) {
            let mut child = match Command::new(env!("CARGO_BIN_EXE_panelink"))
                .args(["workspace", "stop", "1", "1"])
                .env_clear()
                .env("PANELINK_HOME", &self.0)
                .env("HOME", &self.0)
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&self.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(_) => return,
            };
            let deadline = Instant::now() + COMMAND_TIMEOUT;
            loop {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if !server_lock_held(&server) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn server_lock_held(server: &Path) -> bool {
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(server.join("server.lock"))
    else {
        return false;
    };
    // SAFETY: the descriptor refers to the private fixture server lock.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        // SAFETY: release the probe lock acquired immediately above.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        false
    } else {
        true
    }
}

fn route(project: u64, tab: u64) -> WorkspaceRouteId {
    WorkspaceRouteId {
        project: ProjectId::new(project).unwrap(),
        tab: WorkspaceTabId::new(tab).unwrap(),
    }
}

fn launch_context(cwd: &Path) -> workspace_lifecycle::ProjectLaunchContext {
    workspace_lifecycle::ProjectLaunchContext {
        cwd: cwd.to_owned(),
        context: LaunchContext {
            working_directory: cwd.to_owned(),
            environment: [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("SHELL".into(), "/bin/sh".into()),
            ]
            .into_iter()
            .collect::<Environment>(),
        },
    }
}

#[test]
fn create_shell_tab_same_intent_reconciles_one_registered_route() {
    let fixture = Fixture::new();
    fixture.initialize_project();
    let mut session = workspace_lifecycle::LifecycleSession::default();
    let launch = launch_context(fixture.root());
    let first = workspace_lifecycle::create_shell_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        workspace_lifecycle::CreateIntentId(1),
        &mut session,
        &launch,
    )
    .unwrap();
    let retry = workspace_lifecycle::create_shell_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        workspace_lifecycle::CreateIntentId(1),
        &mut session,
        &launch,
    )
    .unwrap();
    assert_eq!(first.route, route(2, 4));
    assert_eq!(first.name, "Shell 1");
    assert_eq!(first.status, workspace_lifecycle::CreateStatus::Ready);
    assert_eq!(retry, first);
    assert_eq!(default_open::SPAWNS.load(Ordering::Acquire), 1);
    let store = WorkspaceIndexStore::open(fixture.root()).unwrap();
    let record = store.load().unwrap().unwrap();
    assert!(record
        .index()
        .project(ProjectId::new(2).unwrap())
        .unwrap()
        .tabs()
        .any(|tab| tab.id == WorkspaceTabId::new(4).unwrap()));
}

#[test]
fn create_rejects_ninth_tab_without_spawn() {
    let fixture = Fixture::new();
    fixture.initialize_project();
    for n in 0..7 {
        workspace_lifecycle::register_named_tab(
            fixture.root(),
            ProjectId::new(2).unwrap(),
            &format!("Extra {n}"),
        )
        .unwrap();
    }
    let mut session = workspace_lifecycle::LifecycleSession::default();
    let result = workspace_lifecycle::create_shell_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        workspace_lifecycle::CreateIntentId(2),
        &mut session,
        &launch_context(fixture.root()),
    );
    assert_eq!(
        result,
        Err(workspace_lifecycle::LifecycleError::TabLimitReached)
    );
    assert_eq!(default_open::SPAWNS.load(Ordering::Acquire), 0);
}

#[test]
fn rename_tab_revision_conflict_preserves_current_name() {
    let fixture = Fixture::new();
    fixture.initialize_default();
    let before = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    let result =
        workspace_lifecycle::rename_tab(fixture.root(), route(1, 1), "Changed", 0).unwrap();
    assert_eq!(
        result,
        workspace_lifecycle::RenameOutcome::Conflict {
            current_name: Some("Main".to_owned()),
            revision: before.revision(),
        }
    );
    let after = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn close_tab_verifies_identity_before_commit() {
    let fixture = Fixture::new();
    fixture.initialize_project();
    let added = workspace_lifecycle::register_named_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        "Close me",
    )
    .unwrap();
    default_open::LIVE.store(true, Ordering::Release);
    default_open::GENERATION.store(8, Ordering::Release);
    let before = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    let result = workspace_lifecycle::close_tab(
        fixture.root(),
        added,
        &workspace_lifecycle::ExpectedRuntimeIdentity {
            route: added,
            generation: Some(7),
        },
    );
    assert_eq!(
        result,
        workspace_lifecycle::CloseOutcome::Partial {
            reason: "tab was replaced"
        }
    );
    assert_eq!(default_open::STOPS.load(Ordering::Acquire), 0);
    assert!(default_open::LIVE.load(Ordering::Acquire));
    assert_eq!(
        WorkspaceIndexStore::open(fixture.root())
            .unwrap()
            .load()
            .unwrap()
            .unwrap(),
        before
    );
}

#[test]
fn close_nonreserved_route_removes_metadata_after_verified_stop_and_keeps_history() {
    let fixture = Fixture::new();
    fixture.initialize_project();
    let route = workspace_lifecycle::register_named_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        "Close cleanly",
    )
    .unwrap();
    let history = fixture.root().join(format!(
        "projects/2/workspaces/{}/server/history",
        route.tab.get()
    ));
    fs::create_dir_all(&history).unwrap();
    fs::set_permissions(&history, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(history.join("sentinel"), b"retained fixture history").unwrap();
    default_open::LIVE.store(true, Ordering::Release);

    assert_eq!(
        workspace_lifecycle::close_tab(
            fixture.root(),
            route,
            &workspace_lifecycle::ExpectedRuntimeIdentity {
                route,
                generation: Some(8),
            },
        ),
        workspace_lifecycle::CloseOutcome::Stopped
    );
    assert_eq!(default_open::STOPS.load(Ordering::Acquire), 1);
    let record = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert!(!record
        .index()
        .project(route.project)
        .unwrap()
        .tabs()
        .any(|tab| tab.id == route.tab));
    assert_eq!(
        fs::read(history.join("sentinel")).unwrap(),
        b"retained fixture history"
    );
}

#[test]
fn close_reserved_route_sets_hidden_but_keeps_1_1_registered() {
    let fixture = Fixture::new();
    fixture.initialize_default();
    assert_eq!(
        workspace_lifecycle::close_tab(
            fixture.root(),
            route(1, 1),
            &workspace_lifecycle::ExpectedRuntimeIdentity {
                route: route(1, 1),
                generation: None,
            },
        ),
        workspace_lifecycle::CloseOutcome::Stopped
    );
    let store = WorkspaceIndexStore::open(fixture.root()).unwrap();
    let record = store.load().unwrap().unwrap();
    assert!(record
        .index()
        .project(ProjectId::new(1).unwrap())
        .unwrap()
        .tabs()
        .any(|tab| tab.id == WorkspaceTabId::new(1).unwrap()));
    let mut store = store;
    let mut sidecar = TabVisibilityStore::new(&mut store);
    assert!(sidecar.load().unwrap().unwrap().hidden());
}

#[test]
fn workspace_tab_cli_rename_and_close_use_the_lifecycle_adapter() {
    let fixture = Fixture::new();
    fixture.initialize_default();
    let renamed = fixture.run_cli(&["workspace", "tab", "rename", "1", "1", "Primary"]);
    assert!(
        renamed.status.success(),
        "rename reserved route: {renamed:?}"
    );
    let record = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(
        record
            .index()
            .project(ProjectId::new(1).unwrap())
            .unwrap()
            .tabs()
            .next()
            .unwrap()
            .display_name,
        "Primary"
    );
    let closed = fixture.run_cli(&["workspace", "tab", "close", "1", "1"]);
    assert!(closed.status.success(), "close reserved route: {closed:?}");
    let store = WorkspaceIndexStore::open(fixture.root()).unwrap();
    assert!(store
        .load()
        .unwrap()
        .unwrap()
        .index()
        .project(ProjectId::new(1).unwrap())
        .unwrap()
        .tabs()
        .any(|tab| tab.id == WorkspaceTabId::new(1).unwrap()));
}

#[test]
fn workspace_tab_reserved_hidden_legacy_target_still_opens() {
    let fixture = Fixture::new();
    fixture.initialize_default();
    let hidden = fixture.run_cli(&["workspace", "tab", "close", "1", "1"]);
    assert!(hidden.status.success(), "hide legacy route: {hidden:?}");
    let opened = fixture.run_cli(&["workspace", "open", "1", "1"]);
    assert!(!opened.status.success(), "non-PTY attach should be refused");
    assert!(
        String::from_utf8_lossy(&opened.stderr).contains("attach needs an interactive terminal"),
        "explicit hidden-route open should reach the client: {opened:?}"
    );
    assert!(server_lock_held(&fixture.root().join("server")));
    let stopped = fixture.run_cli(&["workspace", "stop", "1", "1"]);
    assert!(
        stopped.status.success(),
        "stop explicit hidden route: {stopped:?}"
    );
    assert!(!server_lock_held(&fixture.root().join("server")));
}

#[test]
fn workspace_tab_last_close_plain_start_is_empty_no_spawn() {
    let fixture = Fixture::new();
    fixture.initialize_default();
    let closed = fixture.run_cli(&["workspace", "tab", "close", "1", "1"]);
    assert!(closed.status.success(), "close reserved route: {closed:?}");
    let output = fixture.run_cli(&[]);
    assert!(output.status.success(), "plain empty start: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No open tabs. Run `panelink workspace tab create ...`"
    );
    assert!(!fixture.root().join("server/server.lock").exists());
    assert!(!fixture.root().join("server/generation").exists());
}

#[test]
fn workspace_tab_crash_after_stop_before_metadata_commit() {
    let fixture = Fixture::new();
    fixture.initialize_project();
    let route = workspace_lifecycle::register_named_tab(
        fixture.root(),
        ProjectId::new(2).unwrap(),
        "Retain history",
    )
    .unwrap();
    let history = fixture.root().join(format!(
        "projects/2/workspaces/{}/server/history",
        route.tab.get()
    ));
    fs::create_dir_all(&history).unwrap();
    fs::set_permissions(&history, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(history.join("sentinel"), b"retained fixture history").unwrap();
    let before = WorkspaceIndexStore::open(fixture.root())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    default_open::LIVE.store(true, Ordering::Release);
    let result = workspace_lifecycle::close_tab_with_fault(
        fixture.root(),
        route,
        &workspace_lifecycle::ExpectedRuntimeIdentity {
            route,
            generation: Some(8),
        },
        true,
    );
    assert_eq!(
        result,
        workspace_lifecycle::CloseOutcome::Partial {
            reason: "tab stopped; metadata not saved"
        }
    );
    assert!(!default_open::LIVE.load(Ordering::Acquire));
    assert_eq!(default_open::STOPS.load(Ordering::Acquire), 1);
    assert_eq!(
        WorkspaceIndexStore::open(fixture.root())
            .unwrap()
            .load()
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        fs::read(history.join("sentinel")).unwrap(),
        b"retained fixture history"
    );
    let (presentation, _) = workspace_lifecycle::presentation_snapshot(fixture.root()).unwrap();
    assert!(presentation.projects.iter().any(|project| {
        project.id == route.project && project.tabs.iter().any(|tab| tab.route == route)
    }));
}
