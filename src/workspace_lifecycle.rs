//! Blocking workspace-tab lifecycle operations.
//!
//! Callers run these operations on a metadata/lifecycle worker, never on the
//! terminal UI event loop. The catalog remains the authority for route IDs;
//! runtime actions are coordinated by `default_open`'s per-route lifecycle
//! locks.

use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::LaunchContext;
use panelink::workspace_index::codec::MetadataRecord;
use panelink::workspace_index::store::{SaveOutcome, StoreError, WorkspaceIndexStore};
use panelink::workspace_index::tab_visibility::TabVisibilityStore;
use panelink::workspace_index::{
    next_shell_name, plan_close_removal, ProjectId, TabPresentationSnapshot, WorkspaceIndex,
    WorkspaceIndexError, WorkspaceTabId, MAX_TABS_PER_PROJECT,
};
use panelink::workspace_tabs::WorkspaceRouteId;

const ERROR: LifecycleError = LifecycleError::MetadataUnavailable;
const REGISTRY_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const REGISTRY_LOCK_BACKOFF: Duration = Duration::from_millis(8);

/// Caller-captured launch context. `cwd` is transient and is never persisted.
#[derive(Clone)]
#[allow(dead_code)] // Consumed by the UX-05c client metadata worker.
pub(crate) struct ProjectLaunchContext {
    pub(crate) cwd: PathBuf,
    pub(crate) context: LaunchContext,
}

impl fmt::Debug for ProjectLaunchContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectLaunchContext")
            .field("cwd", &self.cwd)
            .field("context", &self.context)
            .finish()
    }
}

/// Monotonic within one `RouteController` create-intent stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // Allocated by one UX-05c RouteController.
pub(crate) struct CreateIntentId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Returned to the UX-05c route installation worker.
pub(crate) enum CreateStatus {
    Ready,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Returned to the UX-05c route installation worker.
pub(crate) struct CreateShellTab {
    pub(crate) route: WorkspaceRouteId,
    pub(crate) name: String,
    pub(crate) status: CreateStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RenameOutcome {
    Saved {
        revision: u64,
    },
    Conflict {
        current_name: Option<String>,
        revision: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExpectedRuntimeIdentity {
    pub(crate) route: WorkspaceRouteId,
    pub(crate) generation: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseOutcome {
    Stopped,
    Partial { reason: &'static str },
    Unknown { reason: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleError {
    MetadataUnavailable,
    InvalidRoute,
    InvalidName,
    TabLimitReached,
    RevisionConflict,
    IntentConflict,
    IntentUncertain,
    LaunchContextInvalid,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MetadataUnavailable => "workspace metadata unavailable",
            Self::InvalidRoute => "workspace route is invalid or no longer registered",
            Self::InvalidName => "tab name is invalid",
            Self::TabLimitReached => "project already has eight tabs",
            Self::RevisionConflict => "workspace metadata revision conflict",
            Self::IntentConflict => "create intent is stale or already superseded",
            Self::IntentUncertain => "previous create intent is unresolved",
            Self::LaunchContextInvalid => "launch context is invalid",
        })
    }
}

impl std::error::Error for LifecycleError {}

#[derive(Clone, Debug)]
struct TabPlan {
    route: WorkspaceRouteId,
    name: String,
    expected_revision: u64,
    expected_next_id: u64,
}

#[derive(Clone, Debug)]
struct IntentRecord {
    intent: CreateIntentId,
    plan: TabPlan,
    registered: bool,
    spawn_attempted: bool,
    ready: bool,
}

/// Bounded, caller-owned intent reconciliation state (one most-recent intent).
#[derive(Default)]
#[allow(dead_code)] // Owned by the UX-05c route metadata worker.
pub(crate) struct LifecycleSession {
    last: Option<IntentRecord>,
}

/// Load v1 plus the visibility sidecar. An absent sidecar means every tab is
/// visible; the returned revision is the v1 catalog revision.
#[allow(dead_code)] // Queried by the UX-05c client metadata worker.
pub(crate) fn presentation_snapshot(
    app_root: &Path,
) -> Result<(TabPresentationSnapshot, u64), LifecycleError> {
    let mut store = WorkspaceIndexStore::open(app_root).map_err(|_| ERROR)?;
    let record = store.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    let mut visibility = TabVisibilityStore::new(&mut store);
    let hidden = visibility
        .load()
        .map_err(|_| ERROR)?
        .is_some_and(|record| record.hidden());
    Ok((
        TabPresentationSnapshot::new(record.index(), hidden),
        record.revision(),
    ))
}

/// Read-only presentation snapshot that waits up to `limit` for a concurrent
/// holder of the workspace-index lock. Never initializes or writes records.
#[allow(dead_code)] // Used by no-argument startup in the binary crate.
pub(crate) fn presentation_snapshot_within(
    app_root: &Path,
    limit: Duration,
) -> Result<(TabPresentationSnapshot, u64), LifecycleError> {
    let deadline = Instant::now() + limit;
    loop {
        let attempt = (|| {
            let mut store = WorkspaceIndexStore::open(app_root)?;
            let record = store.load()?.ok_or(StoreError::Uninitialized)?;
            let mut visibility = TabVisibilityStore::new(&mut store);
            let hidden = visibility.load()?.is_some_and(|record| record.hidden());
            Ok::<_, StoreError>((
                TabPresentationSnapshot::new(record.index(), hidden),
                record.revision(),
            ))
        })();
        match attempt {
            Ok(snapshot) => return Ok(snapshot),
            Err(StoreError::Locked) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(REGISTRY_LOCK_BACKOFF.min(remaining));
            }
            Err(_) => return Err(ERROR),
        }
    }
}

/// Whether the v1 workspace catalog has been created. A read-only probe used
/// before any startup decision so that declining to open writes nothing.
#[allow(dead_code)] // Used by no-argument startup in the binary crate.
pub(crate) fn registry_exists(app_root: &Path) -> bool {
    app_root.join("workspace-index.v1").is_file()
}

/// Initialize the legacy catalog and visibility sidecar under the same
/// workspace-index lock before ordinary startup chooses a route.
#[allow(dead_code)] // Used by no-argument startup in the binary crate.
pub(crate) fn initialize_presentation(
    app_root: &Path,
) -> Result<(WorkspaceIndex, TabPresentationSnapshot, u64), LifecycleError> {
    let deadline = Instant::now() + REGISTRY_LOCK_TIMEOUT;
    loop {
        match initialize_presentation_once(app_root) {
            Ok(snapshot) => return Ok(snapshot),
            Err(StoreError::Locked) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(REGISTRY_LOCK_BACKOFF.min(remaining));
            }
            Err(_) => return Err(ERROR),
        }
    }
}

fn initialize_presentation_once(
    app_root: &Path,
) -> Result<(WorkspaceIndex, TabPresentationSnapshot, u64), StoreError> {
    let mut store = WorkspaceIndexStore::open(app_root)?;
    if store.load()?.is_none() {
        store.initialize_legacy()?;
    }
    let record = store.load()?.ok_or(StoreError::Uninitialized)?;
    let mut visibility = TabVisibilityStore::new(&mut store);
    let hidden = visibility.initialize()?.hidden();
    let snapshot = TabPresentationSnapshot::new(record.index(), hidden);
    Ok((record.index().clone(), snapshot, record.revision()))
}

/// Register an explicitly named tab using the same ID allocation and CAS path
/// used by managed-shell creation. This legacy CLI operation remains
/// metadata-only and does not start a server.
pub(crate) fn register_named_tab(
    app_root: &Path,
    project: ProjectId,
    name: &str,
) -> Result<WorkspaceRouteId, LifecycleError> {
    let plan = plan_tab_add(app_root, project, name.to_owned())?;
    commit_tab_plan(app_root, &plan)?;
    Ok(plan.route)
}

/// Allocate, register and start one managed shell. Repeating the same intent
/// reconciles its exact route and never makes a second spawn attempt.
#[allow(dead_code)] // Wired to the UX-05c client metadata worker in its owned slice.
pub(crate) fn create_shell_tab(
    app_root: &Path,
    project: ProjectId,
    intent: CreateIntentId,
    session: &mut LifecycleSession,
    launch: &ProjectLaunchContext,
) -> Result<CreateShellTab, LifecycleError> {
    if intent.0 == 0 || !launch.cwd.is_absolute() {
        return Err(LifecycleError::LaunchContextInvalid);
    }

    let repeat = session
        .last
        .as_ref()
        .is_some_and(|record| record.intent == intent);
    if !repeat {
        if let Some(previous) = session.last.as_ref() {
            if !previous.ready {
                return Err(LifecycleError::IntentUncertain);
            }
            if intent <= previous.intent {
                return Err(LifecycleError::IntentConflict);
            }
        }
        let plan = plan_shell_tab(app_root, project)?;
        session.last = Some(IntentRecord {
            intent,
            plan,
            registered: false,
            spawn_attempted: false,
            ready: false,
        });
    }

    let record = session
        .last
        .as_mut()
        .ok_or(LifecycleError::IntentConflict)?;
    if record.intent != intent {
        return Err(LifecycleError::IntentConflict);
    }
    if !record.registered {
        commit_tab_plan(app_root, &record.plan)?;
        record.registered = true;
    }

    let may_spawn = !record.spawn_attempted;
    record.spawn_attempted = true;
    let mut context = launch.context.clone();
    context.working_directory = launch.cwd.clone();
    let ready =
        crate::default_open::ensure_route_server(app_root, record.plan.route, &context, may_spawn)
            .is_ok();
    record.ready = ready;
    Ok(CreateShellTab {
        route: record.plan.route,
        name: record.plan.name.clone(),
        status: if ready {
            CreateStatus::Ready
        } else {
            CreateStatus::Unknown
        },
    })
}

/// Rename only metadata. A revision mismatch returns the current value and
/// does not write; validation happens before any mutation.
pub(crate) fn rename_tab(
    app_root: &Path,
    route: WorkspaceRouteId,
    name: &str,
    expected_revision: u64,
) -> Result<RenameOutcome, LifecycleError> {
    if validate_name(name).is_err() {
        return Err(LifecycleError::InvalidName);
    }
    let mut store = WorkspaceIndexStore::open(app_root).map_err(|_| ERROR)?;
    let record = store.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    let current_name = record
        .index()
        .project(route.project)
        .ok()
        .and_then(|project| {
            project
                .tabs()
                .find(|tab| tab.id == route.tab)
                .map(|tab| tab.display_name.to_owned())
        });
    if current_name.is_none() {
        return Err(LifecycleError::InvalidRoute);
    }
    if record.revision() != expected_revision {
        return Ok(RenameOutcome::Conflict {
            current_name,
            revision: record.revision(),
        });
    }
    let mut index = record.index().clone();
    index
        .rename_tab(route.project, route.tab, name)
        .map_err(map_index_error)?;
    let revision = record
        .revision()
        .checked_add(1)
        .ok_or(LifecycleError::MetadataUnavailable)?;
    let next = MetadataRecord::new(index, revision, record.next_id())
        .map_err(|_| LifecycleError::MetadataUnavailable)?;
    store
        .save(record.revision(), &next)
        .map_err(map_store_error)?;
    Ok(RenameOutcome::Saved { revision })
}

/// Stop the exact route and only then commit its visibility/removal metadata.
pub(crate) fn close_tab(
    app_root: &Path,
    route: WorkspaceRouteId,
    expected: &ExpectedRuntimeIdentity,
) -> CloseOutcome {
    close_tab_impl(app_root, route, expected, false)
}

/// Fault seam for the crash-window fixture: simulates process death after a
/// verified stop and before the catalog/sidecar commit.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn close_tab_with_fault(
    app_root: &Path,
    route: WorkspaceRouteId,
    expected: &ExpectedRuntimeIdentity,
    fail_after_stop: bool,
) -> CloseOutcome {
    close_tab_impl(app_root, route, expected, fail_after_stop)
}

fn close_tab_impl(
    app_root: &Path,
    route: WorkspaceRouteId,
    expected: &ExpectedRuntimeIdentity,
    fail_after_stop: bool,
) -> CloseOutcome {
    if route != expected.route {
        return CloseOutcome::Partial {
            reason: "tab was replaced",
        };
    }
    let (state, _route_lock) = match crate::default_open::lock_workspace_route(route) {
        Ok(value) => value,
        Err(_) => {
            return CloseOutcome::Unknown {
                reason: "workspace state uncertain",
            }
        }
    };
    if !registered_route(app_root, route) {
        return CloseOutcome::Unknown {
            reason: "workspace route is not registered",
        };
    }
    match crate::default_open::live_server_generation(&state) {
        Ok(Some(generation))
            if expected
                .generation
                .is_some_and(|expected| expected != generation) =>
        {
            return CloseOutcome::Partial {
                reason: "tab was replaced",
            };
        }
        Err(_) => {
            return CloseOutcome::Unknown {
                reason: "workspace state uncertain",
            }
        }
        _ => {}
    }
    if crate::default_open::stop_workspace_locked(&state).is_err() {
        return CloseOutcome::Unknown {
            reason: "workspace stop could not be verified",
        };
    }
    if fail_after_stop {
        return CloseOutcome::Partial {
            reason: "tab stopped; metadata not saved",
        };
    }

    let mut store = match WorkspaceIndexStore::open(app_root) {
        Ok(store) => store,
        Err(_) => {
            return CloseOutcome::Partial {
                reason: "tab stopped; metadata not saved",
            }
        }
    };
    let record = match store.load() {
        Ok(Some(record)) => record,
        _ => {
            return CloseOutcome::Partial {
                reason: "tab stopped; metadata not saved",
            }
        }
    };
    let exists = record
        .index()
        .project(expected.route.project)
        .ok()
        .is_some_and(|project| project.tabs().any(|tab| tab.id == expected.route.tab));
    if !exists {
        return CloseOutcome::Partial {
            reason: "tab stopped; metadata not saved",
        };
    }

    if expected.route.project.get() == 1 && expected.route.tab.get() == 1 {
        let mut visibility = TabVisibilityStore::new(&mut store);
        let sidecar = match visibility.load() {
            Ok(Some(record)) => record,
            Ok(None) => match visibility.initialize() {
                Ok(record) => record,
                Err(_) => {
                    return CloseOutcome::Partial {
                        reason: "tab stopped; metadata not saved",
                    }
                }
            },
            Err(_) => {
                return CloseOutcome::Partial {
                    reason: "tab stopped; metadata not saved",
                }
            }
        };
        if sidecar.hidden() {
            return CloseOutcome::Stopped;
        }
        match visibility.save_hidden(sidecar.revision(), true) {
            Ok(SaveOutcome::Written | SaveOutcome::AlreadyDurable) => CloseOutcome::Stopped,
            Err(_) => CloseOutcome::Partial {
                reason: "tab stopped; metadata not saved",
            },
        }
    } else {
        let index =
            match plan_close_removal(record.index(), expected.route.project, expected.route.tab) {
                Ok(index) => index,
                Err(_) => {
                    return CloseOutcome::Partial {
                        reason: "tab stopped; metadata not saved",
                    }
                }
            };
        let Some(revision) = record.revision().checked_add(1) else {
            return CloseOutcome::Partial {
                reason: "tab stopped; metadata not saved",
            };
        };
        let next = match MetadataRecord::new(index, revision, record.next_id()) {
            Ok(record) => record,
            Err(_) => {
                return CloseOutcome::Partial {
                    reason: "tab stopped; metadata not saved",
                }
            }
        };
        match store.save(record.revision(), &next) {
            Ok(SaveOutcome::Written | SaveOutcome::AlreadyDurable) => CloseOutcome::Stopped,
            Err(_) => CloseOutcome::Partial {
                reason: "tab stopped; metadata not saved",
            },
        }
    }
}

fn registered_route(app_root: &Path, route: WorkspaceRouteId) -> bool {
    let Ok(store) = WorkspaceIndexStore::open(app_root) else {
        return false;
    };
    let Ok(Some(record)) = store.load() else {
        return false;
    };
    record
        .index()
        .project(route.project)
        .is_ok_and(|project| project.tabs().any(|tab| tab.id == route.tab))
}

fn plan_tab_add(
    app_root: &Path,
    project: ProjectId,
    name: String,
) -> Result<TabPlan, LifecycleError> {
    validate_name(&name).map_err(|_| LifecycleError::InvalidName)?;
    let store = WorkspaceIndexStore::open(app_root).map_err(|_| ERROR)?;
    let record = store.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    plan_tab_add_from_record(&record, project, name)
}

fn plan_shell_tab(app_root: &Path, project: ProjectId) -> Result<TabPlan, LifecycleError> {
    let store = WorkspaceIndexStore::open(app_root).map_err(|_| ERROR)?;
    let record = store.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    let project_view = record
        .index()
        .project(project)
        .map_err(|_| LifecycleError::InvalidRoute)?;
    if project_view.tabs().len() >= MAX_TABS_PER_PROJECT {
        return Err(LifecycleError::TabLimitReached);
    }
    let name = next_shell_name(record.index(), project);
    plan_tab_add_from_record(&record, project, name)
}

fn plan_tab_add_from_record(
    record: &MetadataRecord,
    project: ProjectId,
    name: String,
) -> Result<TabPlan, LifecycleError> {
    let project_view = record
        .index()
        .project(project)
        .map_err(|_| LifecycleError::InvalidRoute)?;
    if project_view.tabs().len() >= MAX_TABS_PER_PROJECT {
        return Err(LifecycleError::TabLimitReached);
    }
    let id = WorkspaceTabId::new(record.next_id()).map_err(|_| ERROR)?;
    Ok(TabPlan {
        route: WorkspaceRouteId { project, tab: id },
        name,
        expected_revision: record.revision(),
        expected_next_id: record.next_id(),
    })
}

fn commit_tab_plan(app_root: &Path, plan: &TabPlan) -> Result<(), LifecycleError> {
    let mut store = WorkspaceIndexStore::open(app_root).map_err(|_| ERROR)?;
    let record = store.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    if let Ok(project) = record.index().project(plan.route.project) {
        if let Some(existing) = project.tabs().find(|tab| tab.id == plan.route.tab) {
            if existing.display_name == plan.name {
                return Ok(());
            }
            return Err(LifecycleError::IntentConflict);
        }
    }
    if record.revision() != plan.expected_revision || record.next_id() != plan.expected_next_id {
        return Err(LifecycleError::RevisionConflict);
    }
    let mut index = record.index().clone();
    match index.add_tab(plan.route.project, plan.route.tab, &plan.name) {
        Ok(()) => {}
        Err(WorkspaceIndexError::TabLimitReached) => return Err(LifecycleError::TabLimitReached),
        Err(error) => return Err(map_index_error(error)),
    }
    let revision = record
        .revision()
        .checked_add(1)
        .ok_or(LifecycleError::MetadataUnavailable)?;
    let next_id = plan
        .route
        .tab
        .get()
        .checked_add(1)
        .ok_or(LifecycleError::MetadataUnavailable)?;
    let next = MetadataRecord::new(index, revision, next_id)
        .map_err(|_| LifecycleError::MetadataUnavailable)?;
    store
        .save(record.revision(), &next)
        .map_err(map_store_error)?;
    Ok(())
}

fn validate_name(name: &str) -> Result<(), WorkspaceIndexError> {
    let mut sample = WorkspaceIndex::new();
    sample.add_project(ProjectId::new(1)?, "Default", WorkspaceTabId::new(1)?, name)
}

fn map_index_error(error: WorkspaceIndexError) -> LifecycleError {
    match error {
        WorkspaceIndexError::InvalidName => LifecycleError::InvalidName,
        WorkspaceIndexError::TabLimitReached => LifecycleError::TabLimitReached,
        WorkspaceIndexError::UnknownProject
        | WorkspaceIndexError::UnknownTab
        | WorkspaceIndexError::InvalidId => LifecycleError::InvalidRoute,
        _ => LifecycleError::MetadataUnavailable,
    }
}

fn map_store_error(error: StoreError) -> LifecycleError {
    match error {
        StoreError::RevisionConflict => LifecycleError::RevisionConflict,
        StoreError::Uninitialized | StoreError::InvalidTransition => LifecycleError::InvalidRoute,
        _ => LifecycleError::MetadataUnavailable,
    }
}
