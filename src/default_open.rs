//! Zero-configuration `panelink`: open (or reattach to) the user's workspace.
//!
//! Running `panelink` with no arguments resolves one private per-user state
//! root, compiles the bundled terminfo entry once, starts the background
//! multi-session server if none is running, and attaches the interactive
//! client. Ctrl-X detaches; the shells keep running in the server, and the next
//! `panelink` reattaches. `panelink stop` ends the workspace.
//!
//! Lifecycle rules:
//! - Every opener and `stop` first takes an exclusive `flock` on
//!   `lifecycle.lock` in the state root. Liveness checks, stale cleanup,
//!   storage initialization and server spawn all happen under it, so two
//!   invocations can never both initialize, both spawn, or race a removal.
//! - A running server holds `server/server.lock` for its whole lifetime.
//!   A control socket is removed only while holding the lifecycle lock, when
//!   the server lock is free, and the entry is a socket owned by this user.
//! - The spawned server's identity (pid plus kernel process start time) is
//!   recorded in `server.pid`. `stop` signals only a process whose pid,
//!   start time and owner all still match, so pid reuse cannot redirect it.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::layout_store::{LayoutSlot, LayoutStore};
use panelink::pane_layout::{PaneGeometry, Rect, SplitDirection};
use panelink::recovery_chooser::{self, ChooserChoice, ChooserPlan, LayoutSummary};
use panelink::recovery_view::RecoverySlot;
use panelink::saved_layout::{LayoutAxis, SavedLayout};
use panelink::transport::AuthenticatedStream;
use rustix::fs::{Mode, OFlags};
use rustix::termios::{isatty, tcgetattr, tcsetattr, OptionalActions, Termios};

/// Terminfo source embedded at build time so the binary is self-contained.
const TERMINFO_SOURCE: &str = include_str!("../docs/terminfo/panelink-probe.terminfo");
const SOCKET_NAME: &str = "control.sock";
const LOG_NAME: &str = "server.log";
const PID_NAME: &str = "server.pid";
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_STEP: Duration = Duration::from_millis(25);
const REGISTRY_READ_TIMEOUT: Duration = Duration::from_secs(2);
const REGISTRY_READ_BACKOFF: Duration = Duration::from_millis(8);
/// macOS `sun_path` capacity, including the terminating NUL.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Fixed user-facing failure labels; never contain paths or environment data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenError {
    HomeUnavailable,
    StateRootUnsafe,
    StateRootUnavailable,
    SocketPathTooLong,
    TerminfoCompileFailed,
    ServerStartFailed,
    ServerStartTimedOut,
    StaleEndpointUnsafe,
    StopFailed,
    WorkspaceUnavailable,
    WorkspaceRegistryBusy,
    WorkspaceRegistryUnavailable,
}

impl OpenError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::HomeUnavailable => "cannot find your home directory (set PANELINK_HOME)",
            Self::StateRootUnsafe => {
                "panelink state directory must be owned by you and not group/world writable"
            }
            Self::StateRootUnavailable => "cannot create the panelink state directory",
            Self::SocketPathTooLong => "panelink state path is too long for a socket",
            Self::TerminfoCompileFailed => "could not compile the panelink terminfo entry",
            Self::ServerStartFailed => "could not start the panelink server (see server.log)",
            Self::ServerStartTimedOut => "panelink server did not become ready (see server.log)",
            Self::StaleEndpointUnsafe => "unexpected file at the panelink socket path",
            Self::StopFailed => "could not stop the panelink server",
            Self::WorkspaceUnavailable => "workspace is no longer registered",
            Self::WorkspaceRegistryBusy => "workspace registry is busy; try again",
            Self::WorkspaceRegistryUnavailable => "workspace registry unavailable",
        }
    }
}

/// Resolved private per-user layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StateRoot {
    pub(crate) root: PathBuf,
    pub(crate) server: PathBuf,
    pub(crate) terminfo: PathBuf,
    pub(crate) contexts: PathBuf,
}

/// Resolve one stable registry route. Numeric IDs are the only path inputs.
pub(crate) fn workspace_state(project: u64, tab: u64) -> Result<StateRoot, OpenError> {
    let app = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    if project == 1 && tab == 1 {
        return Ok(app);
    }
    let root = app
        .root
        .join("projects")
        .join(project.to_string())
        .join("workspaces")
        .join(tab.to_string());
    let state = StateRoot {
        root: root.clone(),
        server: root.join("server"),
        terminfo: app.terminfo,
        contexts: root.join("contexts"),
    };
    if state.socket_path().as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return Err(OpenError::SocketPathTooLong);
    }
    Ok(state)
}

pub(crate) fn prepare_app_root() -> Result<PathBuf, OpenError> {
    let app = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    let root = normalized_validated_root(&app.root)?;
    ensure_private_dir(&root)?;
    Ok(root)
}

/// Validate a configured root without creating anything. `false` means the
/// root does not exist yet; unsafe components remain an error.
pub(crate) fn validate_app_root_readonly() -> Result<bool, OpenError> {
    let app = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    ancestors_ok(&app.root, current_uid())?;
    match fs::symlink_metadata(&app.root) {
        Ok(metadata) => {
            private_dir_ok(&metadata, current_uid())?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(OpenError::StateRootUnsafe),
    }
}

/// Canonicalize only after checking every configured component and the final
/// directory. This normalizes macOS /tmp and /var aliases for strict stores,
/// while refusing a symlink at the user-selected app root itself.
fn normalized_validated_root(root: &Path) -> Result<PathBuf, OpenError> {
    ancestors_ok(root, current_uid())?;
    match fs::symlink_metadata(root) {
        Ok(metadata) => private_dir_ok(&metadata, current_uid())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(OpenError::StateRootUnsafe),
    }

    let mut prefix = root.to_path_buf();
    let mut suffix = Vec::new();
    while fs::symlink_metadata(&prefix).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    {
        let name = prefix
            .file_name()
            .ok_or(OpenError::StateRootUnavailable)?
            .to_os_string();
        suffix.push(name);
        if !prefix.pop() {
            return Err(OpenError::StateRootUnavailable);
        }
    }
    let mut canonical = fs::canonicalize(&prefix).map_err(|_| OpenError::StateRootUnsafe)?;
    for component in suffix.into_iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

pub(crate) fn open_workspace_ids(project: u64, tab: u64) -> i32 {
    match workspace_state(project, tab) {
        Ok(state) => {
            if (project != 1 || tab != 1)
                && prepare_workspace_namespace(project, tab, &state).is_err()
            {
                eprintln!("panelink: workspace directory unavailable");
                return 1;
            }
            open_routed_workspace(state, project, tab)
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

fn open_routed_workspace(state: StateRoot, project: u64, tab: u64) -> i32 {
    let app_root = match prepare_app_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let result: Result<(), OpenError> = (|| {
        prepare_root(&state)?;
        ensure_terminfo(&state)?;
        ensure_server_checked(&state, || {
            workspace_catalog_checked(&app_root, project, tab)
                .map(|_| ())
                .map_err(map_registry_route_error)
        })?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            let (index, route) = match workspace_catalog_checked(&app_root, project, tab) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    eprintln!("panelink: {}", map_registry_route_error(error).message());
                    return 1;
                }
            };
            crate::ui_client::run_workspace(state.server, state.contexts, app_root, index, route)
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

/// Load one immutable catalog and verify the requested route before the host
/// is entered. The store lock is released before attachment setup begins.
#[allow(dead_code)] // Kept as the existing non-cancellable crate-local entry point.
pub(crate) fn workspace_catalog(
    app_root: &Path,
    project: u64,
    tab: u64,
) -> Result<
    (
        panelink::workspace_index::WorkspaceIndex,
        panelink::workspace_tabs::WorkspaceRouteId,
    ),
    (),
> {
    workspace_catalog_checked(app_root, project, tab).map_err(|_| ())
}

fn workspace_catalog_checked(
    app_root: &Path,
    project: u64,
    tab: u64,
) -> Result<
    (
        panelink::workspace_index::WorkspaceIndex,
        panelink::workspace_tabs::WorkspaceRouteId,
    ),
    panelink::workspace_index::store::StoreError,
> {
    workspace_catalog_with_loader(
        app_root,
        project,
        tab,
        || false,
        || load_workspace_record(app_root),
    )
}

pub(crate) fn workspace_catalog_cancellable(
    app_root: &Path,
    project: u64,
    tab: u64,
    canceled: impl Fn() -> bool,
) -> Result<
    (
        panelink::workspace_index::WorkspaceIndex,
        panelink::workspace_tabs::WorkspaceRouteId,
    ),
    &'static str,
> {
    workspace_catalog_with_loader(app_root, project, tab, canceled, || {
        load_workspace_record(app_root)
    })
    .map_err(map_workspace_read_label)
}

fn workspace_catalog_with_loader(
    _app_root: &Path,
    project: u64,
    tab: u64,
    canceled: impl Fn() -> bool,
    mut load_record: impl FnMut() -> Result<
        Option<panelink::workspace_index::codec::MetadataRecord>,
        panelink::workspace_index::store::StoreError,
    >,
) -> Result<
    (
        panelink::workspace_index::WorkspaceIndex,
        panelink::workspace_tabs::WorkspaceRouteId,
    ),
    panelink::workspace_index::store::StoreError,
> {
    use panelink::workspace_index::store::StoreError;
    use panelink::workspace_index::{ProjectId, WorkspaceTabId};

    let record =
        retry_registry_read(&mut load_record, canceled)?.ok_or(StoreError::Uninitialized)?;
    let route = panelink::workspace_tabs::WorkspaceRouteId {
        project: ProjectId::new(project).map_err(|_| StoreError::InvalidTransition)?,
        tab: WorkspaceTabId::new(tab).map_err(|_| StoreError::InvalidTransition)?,
    };
    if record.index().project(route.project).is_err()
        || record
            .index()
            .project(route.project)
            .is_ok_and(|entry| !entry.tabs().any(|candidate| candidate.id == route.tab))
    {
        return Err(StoreError::Uninitialized);
    }
    Ok((record.index().clone(), route))
}

fn load_workspace_record(
    app_root: &Path,
) -> Result<
    Option<panelink::workspace_index::codec::MetadataRecord>,
    panelink::workspace_index::store::StoreError,
> {
    use panelink::workspace_index::store::WorkspaceIndexStore;
    WorkspaceIndexStore::open(app_root).and_then(|store| store.load())
}

pub(crate) fn load_workspace_index_snapshot(
    app_root: &Path,
) -> Result<
    Option<panelink::workspace_index::WorkspaceIndex>,
    panelink::workspace_index::store::StoreError,
> {
    let record = retry_registry_read(&mut || load_workspace_record(app_root), || false)?;
    Ok(record.map(|record| record.index().clone()))
}

fn retry_registry_read<T>(
    operation: &mut impl FnMut() -> Result<T, panelink::workspace_index::store::StoreError>,
    canceled: impl Fn() -> bool,
) -> Result<T, panelink::workspace_index::store::StoreError> {
    retry_registry_read_with(
        operation,
        Instant::now() + REGISTRY_READ_TIMEOUT,
        Instant::now,
        thread::sleep,
        canceled,
    )
}

fn retry_registry_read_with<T>(
    operation: &mut impl FnMut() -> Result<T, panelink::workspace_index::store::StoreError>,
    deadline: Instant,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(Duration),
    canceled: impl Fn() -> bool,
) -> Result<T, panelink::workspace_index::store::StoreError> {
    use panelink::workspace_index::store::StoreError;

    loop {
        if canceled() {
            return Err(StoreError::Locked);
        }
        match operation() {
            Err(StoreError::Locked) => {
                let current = now();
                if current >= deadline {
                    return Err(StoreError::Locked);
                }
                wait(REGISTRY_READ_BACKOFF.min(deadline - current));
            }
            result => return result,
        }
    }
}

fn map_registry_route_error(error: panelink::workspace_index::store::StoreError) -> OpenError {
    use panelink::workspace_index::store::StoreError;
    match error {
        StoreError::Locked => OpenError::WorkspaceRegistryBusy,
        StoreError::Uninitialized => OpenError::WorkspaceUnavailable,
        _ => OpenError::WorkspaceRegistryUnavailable,
    }
}

#[allow(dead_code)] // Kept as the existing non-cancellable crate-local entry point.
pub(crate) fn resolve_workspace_route(
    app_root: &Path,
    route: panelink::workspace_tabs::WorkspaceRouteId,
) -> Result<(StateRoot, panelink::workspace_index::WorkspaceIndex), &'static str> {
    resolve_workspace_route_cancellable(app_root, route, || false)
}

pub(crate) fn resolve_workspace_route_cancellable(
    app_root: &Path,
    route: panelink::workspace_tabs::WorkspaceRouteId,
    canceled: impl Fn() -> bool,
) -> Result<(StateRoot, panelink::workspace_index::WorkspaceIndex), &'static str> {
    let project = route.project.get();
    let tab = route.tab.get();
    let (index, _) = workspace_catalog_with_loader(app_root, project, tab, canceled, || {
        load_workspace_record(app_root)
    })
    .map_err(map_workspace_read_label)?;
    let state = workspace_state(project, tab).map_err(|_| "workspace unavailable")?;
    Ok((state, index))
}

fn map_workspace_read_label(error: panelink::workspace_index::store::StoreError) -> &'static str {
    use panelink::workspace_index::store::StoreError;
    match error {
        StoreError::Locked => "workspace registry busy",
        StoreError::Uninitialized => "workspace is no longer registered",
        _ => "workspace registry unavailable",
    }
}

#[allow(dead_code)] // Kept as the existing non-cancellable crate-local entry point.
pub(crate) fn ensure_workspace_route(
    state: &StateRoot,
    route: panelink::workspace_tabs::WorkspaceRouteId,
    index: &panelink::workspace_index::WorkspaceIndex,
) -> Result<(), &'static str> {
    let app_root = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))
        .map_err(|_| "workspace unavailable")?
        .root;
    ensure_workspace_route_cancellable(&app_root, state, route, index, || false)
}

pub(crate) fn ensure_workspace_route_cancellable(
    app_root: &Path,
    state: &StateRoot,
    route: panelink::workspace_tabs::WorkspaceRouteId,
    index: &panelink::workspace_index::WorkspaceIndex,
    canceled: impl Fn() -> bool,
) -> Result<(), &'static str> {
    let project = route.project.get();
    prepare_root(state).map_err(|_| "workspace unavailable")?;
    ensure_terminfo(state).map_err(|_| "workspace unavailable")?;
    ensure_server_checked(state, || {
        workspace_catalog_with_loader(app_root, project, route.tab.get(), canceled, || {
            load_workspace_record(app_root)
        })
        .map(|(catalog, _)| {
            catalog
                .project(route.project)
                .is_ok_and(|entry| entry.tabs().any(|candidate| candidate.id == route.tab))
                && index
                    .project(route.project)
                    .is_ok_and(|entry| entry.tabs().any(|candidate| candidate.id == route.tab))
        })
        .map_err(map_registry_route_error)
        .and_then(|registered| {
            registered
                .then_some(())
                .ok_or(OpenError::WorkspaceUnavailable)
        })
    })
    .map_err(|error| match error {
        OpenError::WorkspaceRegistryBusy => "workspace registry busy",
        OpenError::WorkspaceUnavailable => "workspace is no longer registered",
        _ => "workspace registry unavailable",
    })?;
    Ok(())
}

/// Persist the current selection as a best-effort metadata-only update.
/// Callers run this on the bounded metadata worker, never the UI loop.
pub(crate) fn save_workspace_selection(
    app_root: &Path,
    route: panelink::workspace_tabs::WorkspaceRouteId,
) -> Result<(), &'static str> {
    use panelink::workspace_index::codec::MetadataRecord;
    use panelink::workspace_index::store::WorkspaceIndexStore;

    let mut store = WorkspaceIndexStore::open(app_root).map_err(|_| "selection not saved")?;
    let record = store
        .load()
        .map_err(|_| "selection not saved")?
        .ok_or("selection not saved")?;
    let mut index = record.index().clone();
    index
        .select_tab(route.project, route.tab)
        .map_err(|_| "selection not saved")?;
    let next_revision = record
        .revision()
        .checked_add(1)
        .ok_or("selection not saved")?;
    let next = MetadataRecord::new(index, next_revision, record.next_id())
        .map_err(|_| "selection not saved")?;
    store
        .save(record.revision(), &next)
        .map_err(|_| "selection not saved")?;
    Ok(())
}

fn prepare_workspace_namespace(project: u64, tab: u64, state: &StateRoot) -> Result<(), OpenError> {
    let app = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    ancestors_ok(&app.root, current_uid())?;
    ensure_private_dir(&app.root)?;
    let projects = app.root.join("projects");
    ensure_private_dir(&projects)?;
    let project_dir = projects.join(project.to_string());
    ensure_private_dir(&project_dir)?;
    let workspaces = project_dir.join("workspaces");
    ensure_private_dir(&workspaces)?;
    let tab_dir = workspaces.join(tab.to_string());
    ensure_private_dir(&tab_dir)?;
    ensure_private_dir(&state.root)?;
    ensure_private_dir(&state.server)?;
    ensure_private_dir(&state.contexts)?;
    ensure_private_dir(&state.terminfo)?;
    Ok(())
}

pub(crate) fn stop_workspace_ids(project: u64, tab: u64) -> Result<(), &'static str> {
    let state = workspace_state(project, tab).map_err(|_| "workspace unavailable")?;
    if !validate_workspace_root(project, tab, &state)? {
        return Ok(());
    }
    let _lifecycle = lifecycle_lock(&state, STOP_TIMEOUT).map_err(|_| "workspace busy")?;
    let lock_held = server_lock_held_checked(&state.server)?;
    if !lock_held {
        if path_entry_exists(&state.root.join(PID_NAME))?
            || endpoint_state_checked(&state.socket_path())? != EndpointState::Absent
            || server_ready(&state)
        {
            return Err("workspace state uncertain");
        }
        return Ok(());
    }
    let pid = live_server_pid(&state).ok_or("workspace identity unavailable")?;
    // SAFETY: `live_server_pid` verifies the exact recorded process identity.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err("workspace stop failed");
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while server_lock_held_checked(&state.server)? {
        if Instant::now() >= deadline {
            return Err("workspace stop failed");
        }
        thread::sleep(POLL_STEP);
    }
    if endpoint_state_checked(&state.socket_path())? != EndpointState::Absent {
        return Err("workspace state uncertain");
    }
    remove_pid_record(&state.root.join(PID_NAME))?;
    Ok(())
}

pub(crate) fn workspace_remove_guard<T>(
    project: u64,
    tab: u64,
    operation: impl FnOnce() -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    let state = workspace_state(project, tab).map_err(|_| "workspace unavailable")?;
    if !validate_workspace_root(project, tab, &state)? {
        // Removal must serialize with first open even when the namespace is
        // absent. Otherwise an opener can validate registration and start
        // between this absence check and the registry mutation below.
        if project == 1 && tab == 1 {
            prepare_root(&state)
        } else {
            prepare_workspace_namespace(project, tab, &state)
        }
        .map_err(|_| "workspace unavailable")?;
    }
    let _lifecycle = lifecycle_lock(&state, STOP_TIMEOUT).map_err(|_| "workspace busy")?;
    if server_lock_held_checked(&state.server)? || server_ready(&state) {
        return Err("workspace is running");
    }
    if path_entry_exists(&state.root.join(PID_NAME))? {
        // A leftover identity record is ambiguous until the route's normal
        // stop path has verified and retired it.
        return Err("workspace identity is uncertain");
    }
    if endpoint_state_checked(&state.socket_path())? != EndpointState::Absent {
        return Err("workspace state uncertain");
    }
    operation()
}

/// Validate an existing workspace namespace without creating directories.
/// A missing namespace cannot currently own a server. Removal must still
/// serialize with first open before mutating registration. Existing
/// components must all be private real dirs.
fn validate_workspace_root(
    project: u64,
    tab: u64,
    state: &StateRoot,
) -> Result<bool, &'static str> {
    let app = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))
        .map_err(|_| "workspace unavailable")?;
    ancestors_ok(&app.root, current_uid()).map_err(|_| "workspace state unsafe")?;
    let mut paths = vec![app.root.clone()];
    if project != 1 || tab != 1 {
        paths.extend([
            app.root.join("projects"),
            app.root.join("projects").join(project.to_string()),
            app.root
                .join("projects")
                .join(project.to_string())
                .join("workspaces"),
            state.root.clone(),
        ]);
    }
    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                private_dir_ok(&metadata, current_uid()).map_err(|_| "workspace state unsafe")?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err("workspace state unsafe"),
        }
    }
    for path in [&state.server, &state.contexts] {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                private_dir_ok(&metadata, current_uid()).map_err(|_| "workspace state unsafe")?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("workspace state unsafe"),
        }
    }
    Ok(true)
}

impl StateRoot {
    fn from_root(root: PathBuf) -> Self {
        Self {
            server: root.join("server"),
            terminfo: root.join("terminfo"),
            contexts: root.join("contexts"),
            root,
        }
    }

    fn socket_path(&self) -> PathBuf {
        self.server.join(SOCKET_NAME)
    }
}

/// Select the state root: `$PANELINK_HOME` (absolute) or
/// `~/Library/Application Support/Panelink`.
pub(crate) fn resolve_root(
    panelink_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<StateRoot, OpenError> {
    let root = match panelink_home.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => {
            let home = home
                .filter(|value| !value.is_empty())
                .ok_or(OpenError::HomeUnavailable)?;
            PathBuf::from(home).join("Library/Application Support/Panelink")
        }
    };
    if !root.is_absolute() {
        return Err(OpenError::StateRootUnavailable);
    }
    let state = StateRoot::from_root(root);
    let socket_len = state.socket_path().as_os_str().len();
    if socket_len > MAX_SOCKET_PATH_BYTES {
        return Err(OpenError::SocketPathTooLong);
    }
    Ok(state)
}

/// Create (0700) or validate one private directory owned by this user.
fn ensure_private_dir(path: &Path) -> Result<(), OpenError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|_| OpenError::StateRootUnavailable)?;
        }
        Err(_) => return Err(OpenError::StateRootUnavailable),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| OpenError::StateRootUnavailable)?;
    private_dir_ok(&metadata, current_uid())
}

fn private_dir_ok(metadata: &fs::Metadata, uid: u32) -> Result<(), OpenError> {
    if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(OpenError::StateRootUnsafe);
    }
    Ok(())
}

/// Every existing ancestor must be a real directory owned by root or this
/// user and not writable by others (sticky root-owned dirs such as /tmp are
/// accepted), so no other account can swap a path component.
fn ancestors_ok(path: &Path, uid: u32) -> Result<(), OpenError> {
    let mut current = PathBuf::from("/");
    let components: Vec<_> = path.components().skip(1).collect();
    for component in &components[..components.len().saturating_sub(1)] {
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(OpenError::StateRootUnsafe),
        };
        let file_type = metadata.file_type();
        // Accept only macOS's system-owned aliases, not arbitrary root-owned
        // symlink ancestors that could redirect the configured app root.
        if file_type.is_symlink() {
            let expected = match current.as_path() {
                path if path == Path::new("/tmp") => Path::new("/private/tmp"),
                path if path == Path::new("/var") => Path::new("/private/var"),
                _ => return Err(OpenError::StateRootUnsafe),
            };
            if metadata.uid() != 0
                || fs::canonicalize(&current).map_err(|_| OpenError::StateRootUnsafe)? != expected
            {
                return Err(OpenError::StateRootUnsafe);
            }
            continue;
        }
        let owner_ok = metadata.uid() == 0 || metadata.uid() == uid;
        let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        if !file_type.is_dir() || !owner_ok || (metadata.mode() & 0o022 != 0 && !sticky_root) {
            return Err(OpenError::StateRootUnsafe);
        }
    }
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn prepare_root(state: &StateRoot) -> Result<(), OpenError> {
    ancestors_ok(&state.root, current_uid())?;
    ensure_private_dir(&state.root)?;
    for directory in [&state.server, &state.terminfo, &state.contexts] {
        ensure_private_dir(directory)?;
    }
    Ok(())
}

/// FNV-1a over the embedded source; identifies the compiled terminfo build.
fn terminfo_fingerprint(source: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}\n")
}

fn terminfo_entry_present(directory: &Path) -> bool {
    ["p", "70"]
        .iter()
        .any(|prefix| directory.join(prefix).join("panelink-probe").is_file())
}

/// Whether the compiled entry must be (re)built for `source`.
pub(crate) fn terminfo_needs_compile(directory: &Path, source: &str) -> bool {
    if !terminfo_entry_present(directory) {
        return true;
    }
    fs::read_to_string(directory.join("source.fingerprint"))
        .map_or(true, |stored| stored != terminfo_fingerprint(source))
}

/// Compile into a private per-process staging directory, then publish the
/// compiled entry and fingerprint with atomic renames so concurrent first
/// opens never observe or produce a half-written entry.
fn ensure_terminfo(state: &StateRoot) -> Result<(), OpenError> {
    if !terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE) {
        return Ok(());
    }
    let staging = state
        .root
        .join(format!("terminfo.staging.{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    ensure_private_dir(&staging)?;
    let result = compile_and_publish(state, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn compile_and_publish(state: &StateRoot, staging: &Path) -> Result<(), OpenError> {
    let source_path = staging.join("panelink-probe.src");
    write_private(&source_path, TERMINFO_SOURCE.as_bytes())?;
    let status = Command::new("/usr/bin/tic")
        .arg("-x")
        .arg("-o")
        .arg(staging)
        .arg(&source_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| OpenError::TerminfoCompileFailed)?;
    if !status.success() {
        return Err(OpenError::TerminfoCompileFailed);
    }
    let mut published = false;
    for prefix in ["p", "70"] {
        let compiled = staging.join(prefix).join("panelink-probe");
        if !compiled.is_file() {
            continue;
        }
        let target_dir = state.terminfo.join(prefix);
        ensure_private_dir(&target_dir)?;
        fs::rename(&compiled, target_dir.join("panelink-probe"))
            .map_err(|_| OpenError::TerminfoCompileFailed)?;
        published = true;
    }
    if !published {
        return Err(OpenError::TerminfoCompileFailed);
    }
    let fingerprint = staging.join("source.fingerprint");
    write_private(
        &fingerprint,
        terminfo_fingerprint(TERMINFO_SOURCE).as_bytes(),
    )?;
    fs::rename(&fingerprint, state.terminfo.join("source.fingerprint"))
        .map_err(|_| OpenError::TerminfoCompileFailed)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), OpenError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| OpenError::StateRootUnavailable)?;
    file.write_all(bytes)
        .map_err(|_| OpenError::StateRootUnavailable)
}

/// What exists at the socket path and whether a server holds the lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointState {
    /// No socket entry.
    Absent,
    /// A socket entry owned by this user.
    Socket,
    /// Anything else (regular file, symlink, other owner).
    Foreign,
}

/// Decision for one observed endpoint/lock combination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointAction {
    /// A server is alive: try to attach (retrying while it finishes startup).
    UseRunning,
    /// No server and no socket: start one.
    Start,
    /// No server but a leftover socket: remove it, then start.
    RemoveStaleThenStart,
    /// Refuse to touch an unexpected entry.
    Refuse,
}

pub(crate) fn decide_endpoint(endpoint: EndpointState, lock_held: bool) -> EndpointAction {
    match (endpoint, lock_held) {
        (EndpointState::Foreign, _) => EndpointAction::Refuse,
        (_, true) => EndpointAction::UseRunning,
        (EndpointState::Absent, false) => EndpointAction::Start,
        (EndpointState::Socket, false) => EndpointAction::RemoveStaleThenStart,
    }
}

fn endpoint_state(path: &Path) -> EndpointState {
    match fs::symlink_metadata(path) {
        Err(_) => EndpointState::Absent,
        Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == current_uid() => {
            EndpointState::Socket
        }
        Ok(_) => EndpointState::Foreign,
    }
}

fn endpoint_state_checked(path: &Path) -> Result<EndpointState, &'static str> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(EndpointState::Absent),
        Err(_) => Err("workspace state uncertain"),
        Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == current_uid() => {
            Ok(EndpointState::Socket)
        }
        Ok(_) => Err("workspace state unsafe"),
    }
}

/// Any PID path entry, including a dangling symlink or an unreadable entry,
/// is ambiguous until explicitly retired by the verified stop path.
fn path_entry_exists(path: &Path) -> Result<bool, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("workspace state uncertain"),
    }
}

fn remove_pid_record(path: &Path) -> Result<(), &'static str> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("workspace state uncertain"),
    }
}

/// Whether a live server holds the storage lock. A missing lock file means
/// storage was never initialized, so no server can be running.
fn server_lock_held(server_dir: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(server_dir.join("server.lock"))
    else {
        return false;
    };
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    // SAFETY: fd is a valid open descriptor owned by `file`; a successful
    // probe lock is released by the following unlock/close.
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        // SAFETY: as above.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        false
    } else {
        true
    }
}

/// Liveness probe for destructive workspace operations. Unlike the general
/// startup probe, every unexpected open/flock failure is treated as unknown.
fn server_lock_held_checked(server_dir: &Path) -> Result<bool, &'static str> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(server_dir.join("server.lock"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("workspace state uncertain"),
    };
    let metadata = file.metadata().map_err(|_| "workspace state uncertain")?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err("workspace state unsafe");
    }
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    // SAFETY: fd is a valid open descriptor owned by `file`.
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        // SAFETY: the successful probe lock is released before `file` closes.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) || error.raw_os_error() == Some(libc::EAGAIN)
    {
        Ok(true)
    } else {
        Err("workspace state uncertain")
    }
}

/// Exclusive per-root lifecycle lock (released on drop/close).
struct LifecycleLock {
    _file: File,
}

fn lifecycle_lock(state: &StateRoot, timeout: Duration) -> Result<LifecycleLock, OpenError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(state.root.join("lifecycle.lock"))
        .map_err(|_| OpenError::StateRootUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| OpenError::StateRootUnavailable)?;
    if !metadata.is_file() || metadata.uid() != current_uid() {
        return Err(OpenError::StateRootUnsafe);
    }
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: fd is a valid descriptor owned by `file`.
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(LifecycleLock { _file: file });
        }
        if Instant::now() >= deadline {
            return Err(OpenError::ServerStartTimedOut);
        }
        thread::sleep(POLL_STEP);
    }
}

fn storage_initialized(server_dir: &Path) -> bool {
    server_dir.join("server.lock").exists() && server_dir.join("generation").exists()
}

fn server_ready(state: &StateRoot) -> bool {
    AuthenticatedStream::connect(&state.server, Instant::now() + Duration::from_millis(500)).is_ok()
}

/// Rotate the bounded server log before a new server appends to it.
fn open_log(state: &StateRoot) -> Result<File, OpenError> {
    let path = state.root.join(LOG_NAME);
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&path, state.root.join("server.log.1"));
    }
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| OpenError::ServerStartFailed)
}

/// Shell for new workspaces: absolute executable `$SHELL`, else `/bin/zsh`.
pub(crate) fn default_shell(shell: Option<OsString>) -> PathBuf {
    shell
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && is_executable_file(path))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

/// Start `panelink serve` detached in its own session. The server inherits
/// this invocation's environment and working directory as the explicit
/// launch context for its first shell (existing `serve` behavior).
fn spawn_server(state: &StateRoot) -> Result<std::process::Child, OpenError> {
    let executable = std::env::current_exe().map_err(|_| OpenError::ServerStartFailed)?;
    let log = open_log(state)?;
    let log_err = log.try_clone().map_err(|_| OpenError::ServerStartFailed)?;
    let shell = default_shell(std::env::var_os("SHELL"));
    let mut command = Command::new(executable);
    command
        .arg("serve")
        .arg("--directory")
        .arg(&state.server)
        .arg("--terminfo")
        .arg(&state.terminfo);
    if !storage_initialized(&state.server) {
        command.arg("--initialize");
    }
    command
        .arg("--allow-input")
        .arg("--")
        .arg(shell)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // SAFETY: setsid is async-signal-safe and only detaches the child from
    // this terminal's session so closing the window cannot hang it up.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|_| OpenError::ServerStartFailed)?;
    // Spawned under the lifecycle lock, so this is the only starter. Record
    // pid + kernel start time; `stop` re-verifies both before signalling.
    let pid = i32::try_from(child.id()).map_err(|_| OpenError::ServerStartFailed)?;
    let start = process_start_time(pid).ok_or(OpenError::ServerStartFailed)?;
    write_private(
        &state.root.join(PID_NAME),
        format!("{pid} {start}\n").as_bytes(),
    )?;
    Ok(child)
}

/// Kernel start time (microseconds) of a live process owned by this user.
fn process_start_time(pid: i32) -> Option<u64> {
    // SAFETY: proc_bsdinfo is plain data; zeroed is a valid initial value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the buffer pointer/size describe `info` exactly.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).ok()?,
        )
    };
    if usize::try_from(written).ok()? != size || info.pbi_uid != current_uid() {
        return None;
    }
    // A zombie (exited, unreaped) is not a live server.
    if info.pbi_status == libc::SZOMB {
        return None;
    }
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// Ensure a server is running for `state`, starting one if needed.
///
/// The whole inspect/cleanup/spawn/wait sequence runs under the lifecycle
/// lock, so a concurrent opener simply waits and then finds the server.
fn ensure_server(state: &StateRoot) -> Result<(), OpenError> {
    ensure_server_checked(state, || Ok(()))
}

fn ensure_server_checked(
    state: &StateRoot,
    still_registered: impl FnOnce() -> Result<(), OpenError>,
) -> Result<(), OpenError> {
    let _lifecycle = lifecycle_lock(state, START_TIMEOUT + STOP_TIMEOUT)?;
    still_registered()?;
    let endpoint = endpoint_state(&state.socket_path());
    let lock_held = server_lock_held(&state.server);
    match decide_endpoint(endpoint, lock_held) {
        EndpointAction::Refuse => return Err(OpenError::StaleEndpointUnsafe),
        EndpointAction::UseRunning => return wait_ready(state, None),
        EndpointAction::RemoveStaleThenStart => {
            // Still under the lifecycle lock: nobody can start a server
            // between the lock check above and this removal.
            fs::remove_file(state.socket_path()).map_err(|_| OpenError::StaleEndpointUnsafe)?;
        }
        EndpointAction::Start => {}
    }
    let child = spawn_server(state)?;
    wait_ready(state, Some(child))
}

/// Wait until the server accepts an authenticated connection. When we just
/// spawned it, fail fast if it exits instead of becoming ready.
fn wait_ready(state: &StateRoot, mut child: Option<std::process::Child>) -> Result<(), OpenError> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if endpoint_state(&state.socket_path()) == EndpointState::Socket && server_ready(state) {
            // The child keeps running after we exit; it is not waited on.
            drop(child);
            return Ok(());
        }
        if let Some(process) = child.as_mut() {
            if process.try_wait().ok().flatten().is_some() {
                return Err(OpenError::ServerStartFailed);
            }
        }
        if Instant::now() >= deadline {
            return Err(OpenError::ServerStartTimedOut);
        }
        thread::sleep(POLL_STEP);
    }
}

fn prepare() -> Result<StateRoot, OpenError> {
    let state = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    prepare_root(&state)?;
    Ok(state)
}

/// `panelink` with no arguments.
pub(crate) fn open() -> i32 {
    let state = match resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };

    // Preserve direct attachment when the server is already reachable. On a
    // stopped root, inspect historical metadata before creating or initializing
    // any runtime paths.
    let live_server = observe_live_server(&state).unwrap_or(false);
    let (current, previous) = if live_server {
        (None, None)
    } else {
        inspect_saved_layouts(&state)
    };
    let interactive = isatty(io::stdin()) && isatty(io::stdout());
    match recovery_chooser::plan(live_server, interactive, current, previous) {
        ChooserPlan::AttachExisting | ChooserPlan::FreshShell => open_workspace(state),
        ChooserPlan::Prompt { .. } => {
            run_startup_chooser(state, current.expect("prompt has current layout"), previous)
        }
    }
}

/// Resume the existing plain-open path after a user selected a fresh shell or
/// when the chooser does not apply.
fn open_workspace(state: StateRoot) -> i32 {
    let result: Result<StateRoot, OpenError> = (|| {
        prepare_root(&state)?;
        ensure_terminfo(&state)?;
        ensure_server(&state)?;
        Ok(state)
    })();
    match result {
        Ok(state) => crate::ui_client::run(state.server, state.contexts),
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

/// Validate existing directories without creating them before probing an
/// endpoint. A missing root/server is an ordinary first startup.
fn observe_live_server(state: &StateRoot) -> Result<bool, OpenError> {
    let uid = current_uid();
    if ancestors_ok(&state.root, uid).is_err() {
        return Ok(false);
    }
    if !existing_private_directory(&state.root, uid)?
        || !existing_private_directory(&state.server, uid)?
    {
        return Ok(false);
    }
    Ok(server_ready(state))
}

fn existing_private_directory(path: &Path, uid: u32) -> Result<bool, OpenError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            private_dir_ok(&metadata, uid)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(OpenError::StateRootUnavailable),
    }
}

/// Read both saved slots without creating the layout directory or printing
/// storage errors. A missing current slot is not a chooser condition.
fn inspect_saved_layouts(state: &StateRoot) -> (Option<LayoutSummary>, Option<LayoutSummary>) {
    let Ok(store) = LayoutStore::open(&state.server.join("layout")) else {
        return (None, None);
    };
    let Ok(Some(current)) = store.load(LayoutSlot::Current) else {
        return (None, None);
    };
    let previous = store.load(LayoutSlot::Previous).ok().flatten();
    let summary = |layout: SavedLayout| LayoutSummary {
        pane_count: layout.pane_count(),
        observed_at_unix_ms: layout.metadata().observed_at_unix_ms(),
    };
    (Some(summary(current)), previous.map(summary))
}

fn run_startup_chooser(
    state: StateRoot,
    current: LayoutSummary,
    previous: Option<LayoutSummary>,
) -> i32 {
    loop {
        let prompt = chooser_prompt(current, previous);
        match read_startup_choice(prompt.as_bytes()) {
            ChooserChoice::Fresh => return open_workspace(state),
            ChooserChoice::Quit => return 0,
            ChooserChoice::ViewCurrent => {
                view_saved_layout(&state, RecoverySlot::Current);
            }
            ChooserChoice::ViewPrevious if previous.is_some() => {
                view_saved_layout(&state, RecoverySlot::Previous);
            }
            ChooserChoice::ViewPrevious => return open_workspace(state),
        }
    }
}

fn chooser_prompt(current: LayoutSummary, previous: Option<LayoutSummary>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0);
    let age = recovery_chooser::relative_age(current, now);
    let previous_option = if previous.is_some() {
        "  [p] view previous"
    } else {
        ""
    };
    format!(
        "panelink: a saved layout from {age} ({} panes) exists. [Enter] fresh shell  [v] view saved layout{previous_option}  [q] quit\r\n",
        current.pane_count
    )
}

fn view_saved_layout(state: &StateRoot, slot: RecoverySlot) {
    if let Ok(view) = panelink::recovery_reader::read_recovery(&state.server, slot) {
        let _ = crate::ui_client::recovery::run(view);
    }
}

struct ChooserTerminal {
    tty: File,
    original: Termios,
    restore_needed: bool,
}

impl ChooserTerminal {
    fn open() -> Option<Self> {
        let fd = rustix::fs::open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        let tty = File::from(fd);
        let original = tcgetattr(&tty).ok()?;
        Some(Self {
            tty,
            original,
            restore_needed: true,
        })
    }

    fn read_choice(&mut self, prompt: &[u8]) -> ChooserChoice {
        let mut raw = self.original.clone();
        raw.make_raw();
        // A failed tcsetattr may have changed some terminal flags, so always
        // restore the exact captured attributes after this point.
        if tcsetattr(&self.tty, OptionalActions::Now, &raw).is_err() {
            return ChooserChoice::Fresh;
        }
        if self.tty.write_all(prompt).is_err() {
            return ChooserChoice::Fresh;
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut byte = [0_u8; 1];
        loop {
            if Instant::now() >= deadline {
                return ChooserChoice::Fresh;
            }
            match self.tty.read(&mut byte) {
                Ok(1) => return recovery_chooser::choice(byte[0]),
                Ok(_) => return ChooserChoice::Fresh,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return ChooserChoice::Fresh,
            }
        }
    }
}

impl Drop for ChooserTerminal {
    fn drop(&mut self) {
        if self.restore_needed {
            let _ = tcsetattr(&self.tty, OptionalActions::Now, &self.original);
        }
    }
}

fn read_startup_choice(prompt: &[u8]) -> ChooserChoice {
    ChooserTerminal::open()
        .map(|mut terminal| terminal.read_choice(prompt))
        .unwrap_or(ChooserChoice::Fresh)
}

/// Parse `PID START` from the private identity record.
pub(crate) fn parse_identity(text: &str) -> Option<(i32, u64)> {
    let mut parts = text.split_whitespace();
    let pid = parts.next()?.parse::<i32>().ok().filter(|pid| *pid > 1)?;
    let start = parts
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|start| *start > 0)?;
    parts.next().is_none().then_some((pid, start))
}

/// The recorded server pid, only if that exact process (same start time and
/// owner) is still alive.
fn live_server_pid(state: &StateRoot) -> Option<i32> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(state.root.join(PID_NAME))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return None;
    }
    let mut text = String::new();
    file.take(64).read_to_string(&mut text).ok()?;
    let (pid, start) = parse_identity(&text)?;
    (process_start_time(pid) == Some(start)).then_some(pid)
}

/// `panelink stop`: end the background workspace (server and its shells).
pub(crate) fn stop() -> i32 {
    match stop_inner() {
        Ok(true) => {
            println!("panelink: workspace stopped");
            0
        }
        Ok(false) => {
            println!("panelink: no workspace is running");
            0
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

fn stop_inner() -> Result<bool, OpenError> {
    let state = prepare()?;
    let _lifecycle = lifecycle_lock(&state, STOP_TIMEOUT)?;
    if !server_lock_held(&state.server) {
        if endpoint_state(&state.socket_path()) == EndpointState::Socket {
            let _ = fs::remove_file(state.socket_path());
        }
        let _ = fs::remove_file(state.root.join(PID_NAME));
        return Ok(false);
    }
    let pid = live_server_pid(&state).ok_or(OpenError::StopFailed)?;
    // SAFETY: kill with a verified pid (same owner and kernel start time as
    // the server this root spawned) and SIGTERM, which serve handles cleanly.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(OpenError::StopFailed);
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while server_lock_held(&state.server) {
        if Instant::now() >= deadline {
            return Err(OpenError::StopFailed);
        }
        thread::sleep(POLL_STEP);
    }
    let _ = fs::remove_file(state.root.join(PID_NAME));
    Ok(true)
}

/// Private context file reused by `panelink panes` and `panelink close`.
///
/// It lives in the private state root (not `contexts/`, which belongs to the
/// attach client), so both commands resume one lease instead of issuing a new
/// one per run.
const CLI_CONTEXT_NAME: &str = "cli.ctx";

const NO_WORKSPACE: &str = "no workspace is running";

fn saved_store() -> Result<HistoryStore, &'static str> {
    let state = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))
        .map_err(|_| "history directory unavailable")?;
    HistoryStore::open(&state.server.join("history")).map_err(|_| "history directory unavailable")
}

/// List recorded targets. This is a read-only filesystem observation and
/// does not start, attach to, or stop a managed process.
pub(crate) fn history_list() -> i32 {
    let result =
        saved_store().and_then(|store| store.list_targets().map_err(|_| "saved history invalid"));
    match result {
        Ok(targets) => {
            for target in targets {
                println!(
                    "{}:{}:{}:{}:{}",
                    target.generation,
                    target.workspace,
                    target.pane,
                    target.session,
                    target.occupant_revision
                );
            }
            0
        }
        Err(error) => {
            eprintln!("panelink: {error}");
            1
        }
    }
}

fn parse_history_target(value: &std::ffi::OsStr) -> Option<TargetRef> {
    let parts = value
        .to_str()?
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.len() != 5 || parts.contains(&0) {
        return None;
    }
    Some(TargetRef {
        generation: parts[0],
        workspace: parts[1],
        pane: parts[2],
        session: parts[3],
        occupant_revision: parts[4],
    })
}

/// Read the retained sanitized rows of exactly one immutable target.
pub(crate) fn history_show(value: &std::ffi::OsStr) -> i32 {
    let Some(target) = parse_history_target(value) else {
        eprintln!("panelink: invalid target");
        return 2;
    };
    let result = saved_store().and_then(|store| {
        let rows = store
            .load_rows(target)
            .map_err(|_| "saved history invalid")?;
        let screens = store
            .load_screens(target)
            .map_err(|_| "saved history invalid")?;
        Ok((rows, screens))
    });
    match result {
        Ok((Some(saved), screens)) => {
            println!(
                "saved history epoch {} (gap: {}, capture disabled: {})",
                saved.epoch, saved.gap, saved.capture_disabled
            );
            for record in saved.records {
                let Ok(row) = history_rows::decode(&record) else {
                    eprintln!("panelink: saved history invalid");
                    return 1;
                };
                let mut line = String::new();
                for column in 0..row.width {
                    line.push_str(row.cell_text(column).unwrap_or(""));
                }
                println!(
                    "{}:{} {}{}",
                    row.epoch,
                    row.ordinal,
                    line.trim_end(),
                    if row.wrapped { " ↩" } else { "" }
                );
            }
            for (buffer, bytes) in ["normal", "alternate"].into_iter().zip(screens) {
                let Some(bytes) = bytes else { continue };
                let checkpoint = match panelink::screen_checkpoint::decode(&bytes) {
                    Ok(checkpoint) => checkpoint,
                    Err(_) => {
                        eprintln!("panelink: saved history invalid");
                        return 1;
                    }
                };
                println!(
                    "checkpoint {buffer} captured at {}:",
                    checkpoint.metadata().captured_at_unix_ms()
                );
                for row in 0..checkpoint.rows() {
                    let mut line = String::new();
                    for column in 0..checkpoint.columns() {
                        if let Some(cell) = checkpoint.cell(row, column) {
                            line.push_str(cell.text());
                        }
                    }
                    println!("{}", line.trim_end());
                }
            }
            0
        }
        Ok((None, _)) => {
            eprintln!("panelink: no saved history for target");
            1
        }
        Err(error) => {
            eprintln!("panelink: {error}");
            1
        }
    }
}

/// Clear only saved Panelink rows and screen checkpoints for the exact target.
pub(crate) fn history_clear(value: &std::ffi::OsStr) -> i32 {
    let Some(target) = parse_history_target(value) else {
        eprintln!("panelink: invalid target");
        return 2;
    };
    let state = match resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")) {
        Ok(state) => state,
        Err(_) => {
            eprintln!("panelink: history directory unavailable");
            return 1;
        }
    };
    let _lifecycle = match lifecycle_lock(&state, STOP_TIMEOUT) {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!("panelink: history clear failed");
            return 1;
        }
    };
    // Direct `panelink serve` does not take lifecycle.lock. Hold the same
    // verified storage lock it needs throughout deletion; a failed/missing
    // lock is never interpreted as proof that the server is absent.
    let _server = match panelink::server_storage::ServerStorage::lock_for_maintenance(&state.server)
    {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!("panelink: stop the workspace before clearing saved history");
            return 1;
        }
    };
    match saved_store().and_then(|store| store.clear(target).map_err(|_| "history clear failed")) {
        Ok(()) => {
            println!("panelink: saved history cleared");
            0
        }
        Err(error) => {
            eprintln!("panelink: {error}");
            1
        }
    }
}

/// Discard only recognized uncommitted history files while excluding writers.
pub(crate) fn history_cleanup() -> i32 {
    let state = match resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")) {
        Ok(state) => state,
        Err(_) => {
            eprintln!("panelink: history directory unavailable");
            return 1;
        }
    };
    let _lifecycle = match lifecycle_lock(&state, STOP_TIMEOUT) {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!("panelink: history cleanup failed");
            return 1;
        }
    };
    // This excludes direct serve as well as zero-config startup for the
    // entire cleanup. Missing metadata is never evidence of a stopped server.
    let _server = match panelink::server_storage::ServerStorage::lock_for_maintenance(&state.server)
    {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!("panelink: stop the workspace before cleaning up saved history");
            return 1;
        }
    };
    match saved_store().and_then(|store| {
        store.cleanup_temporary_files().map_err(|_| {
            "history cleanup failed; uncommitted files may have been partially removed"
        })
    }) {
        Ok(removed) => {
            println!("panelink: uncommitted history files removed: {removed}");
            0
        }
        Err(error) => {
            eprintln!("panelink: {error}");
            1
        }
    }
}

/// Search normalized retained rows and latest committed checkpoints of one
/// exact target. Query is plain Unicode text; no terminal bytes are executed.
pub(crate) fn history_search(target_value: &std::ffi::OsStr, query_value: &std::ffi::OsStr) -> i32 {
    let Some(target) = parse_history_target(target_value) else {
        eprintln!("panelink: invalid target");
        return 2;
    };
    let Some(query) = query_value.to_str().filter(|value| {
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
    }) else {
        eprintln!("panelink: invalid search text");
        return 2;
    };
    let result = saved_store().and_then(|store| {
        let rows = store
            .load_rows(target)
            .map_err(|_| "saved history invalid")?;
        let screens = store
            .load_screens(target)
            .map_err(|_| "saved history invalid")?;
        Ok((rows, screens))
    });
    let (rows, screens) = match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("panelink: {error}");
            return 1;
        }
    };
    if let Some(saved) = rows {
        println!(
            "saved history epoch {} (gap: {}, capture disabled: {})",
            saved.epoch, saved.gap, saved.capture_disabled
        );
        for record in saved.records {
            let Ok(row) = history_rows::decode(&record) else {
                return 1;
            };
            let text = (0..row.width)
                .filter_map(|column| row.cell_text(column))
                .collect::<String>();
            if text.contains(query) {
                println!("row {}:{} {}", row.epoch, row.ordinal, text.trim_end());
            }
        }
    }
    for (buffer, bytes) in ["normal", "alternate"].into_iter().zip(screens) {
        let Some(bytes) = bytes else { continue };
        let Ok(checkpoint) = panelink::screen_checkpoint::decode(&bytes) else {
            return 1;
        };
        for row in 0..checkpoint.rows() {
            let text = (0..checkpoint.columns())
                .filter_map(|column| checkpoint.cell(row, column))
                .map(|cell| cell.text())
                .collect::<String>();
            if text.contains(query) {
                println!(
                    "checkpoint {buffer} {} row {row}: {}",
                    checkpoint.metadata().captured_at_unix_ms(),
                    text.trim_end()
                );
            }
        }
    }
    0
}

/// The default workspace's state root when its server is running; located
/// exactly as `panelink stop` does, and never starts a server.
fn running_workspace() -> Result<Option<StateRoot>, OpenError> {
    let state = prepare()?;
    if !server_lock_held(&state.server)
        || endpoint_state(&state.socket_path()) != EndpointState::Socket
    {
        return Ok(None);
    }
    Ok(Some(state))
}

fn workspace_link(state: &StateRoot) -> Result<crate::p3_cli::WorkspaceLink, &'static str> {
    let link =
        crate::p3_cli::WorkspaceLink::open(&state.server, &state.root.join(CLI_CONTEXT_NAME))?;
    if link.discarded_uncertain {
        eprintln!("panelink: note: an earlier close from a previous workspace was never confirmed");
    }
    Ok(link)
}

/// Human-readable one-line description of one pane.
pub(crate) fn pane_line(index: usize, pane: &panelink::multi_runtime::PaneEntry) -> String {
    let target = pane.target;
    format!(
        "  {index}  {:<7} {}x{} at col {}, row {}  (target {}:{}:{}:{}:{})",
        if pane.live { "running" } else { "exited" },
        pane.rect.rows,
        pane.rect.columns,
        pane.rect.x,
        pane.rect.y,
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision
    )
}

/// `panelink panes`: list the default workspace's panes in layout order.
pub(crate) fn panes() -> i32 {
    let state = match running_workspace() {
        Ok(Some(state)) => state,
        Ok(None) => {
            eprintln!("panelink: {NO_WORKSPACE}");
            return 1;
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let reply = match workspace_link(&state).and_then(|mut link| link.panes()) {
        Ok(reply) => reply,
        Err(_) => {
            eprintln!("panelink: could not read the workspace panes");
            return 1;
        }
    };
    let count = reply.panes.iter().flatten().count();
    println!(
        "{count} pane{} (layout revision {}, {}x{}):",
        if count == 1 { "" } else { "s" },
        reply.layout_revision,
        reply.rows,
        reply.columns
    );
    for (index, pane) in reply.panes.iter().flatten().enumerate() {
        println!("{}", pane_line(index + 1, pane));
    }
    0
}

/// Parse a 1-based pane index argument; `None` is a usage error.
pub(crate) fn parse_pane_index(value: &std::ffi::OsStr) -> Option<usize> {
    let text = value.to_str()?;
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok().filter(|index| *index != 0)
}

/// User-facing result of one `panelink close` (message, exit code).
pub(crate) fn close_message(
    index: usize,
    last_pane: bool,
    result: Result<panelink::close_protocol::CloseResult, crate::p3_cli::CloseFailure>,
) -> (String, i32) {
    use crate::p3_cli::CloseFailure;
    use panelink::close_protocol::{CloseOutcome, CloseRejection, StopOutcome};
    let result = match result {
        Ok(result) => result,
        Err(CloseFailure::Refused) => {
            return (
                "close refused by the server; nothing was stopped".to_owned(),
                1,
            )
        }
        Err(CloseFailure::Unknown) => {
            return (
                format!("close status unknown; pane {index} may still be running"),
                1,
            )
        }
        Err(CloseFailure::NotSent(_)) => {
            return ("close not sent; nothing was stopped".to_owned(), 1)
        }
    };
    let how = match result.outcome {
        CloseOutcome::Stopped(StopOutcome::Exited) => "exited",
        CloseOutcome::Stopped(StopOutcome::Escalated) => "escalated: SIGKILL",
        CloseOutcome::Stopped(StopOutcome::Unknown) => {
            return (
                format!("close status unknown; pane {index} may still be running"),
                1,
            )
        }
        CloseOutcome::Rejected(reason) => {
            let why = match reason {
                CloseRejection::StaleLayout => "the layout changed",
                CloseRejection::StaleTarget => "the pane changed",
                CloseRejection::AlreadyClosed => "the pane is already closed",
                CloseRejection::NoLayout => "the workspace has no panes",
                CloseRejection::NotAdmitted => "the server did not admit it",
            };
            return (
                format!("close rejected: {why}; run 'panelink panes' and retry"),
                1,
            );
        }
    };
    let mut message = format!("closed pane {index} ({how})");
    if last_pane {
        message.push_str("; the workspace has ended");
    } else if result.survivor_resize_pending {
        message.push_str("; the remaining pane's resize is pending");
    }
    (message, 0)
}

/// `panelink close INDEX`: explicitly stop one pane of the default workspace.
pub(crate) fn close(index: usize) -> i32 {
    let state = match running_workspace() {
        Ok(Some(state)) => state,
        Ok(None) => {
            eprintln!("panelink: {NO_WORKSPACE}");
            return 1;
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let mut link = match workspace_link(&state) {
        Ok(link) => link,
        Err(_) => {
            eprintln!("panelink: could not connect to the workspace");
            return 1;
        }
    };
    let reply = match link.panes() {
        Ok(reply) => reply,
        Err(_) => {
            eprintln!("panelink: could not read the workspace panes");
            return 1;
        }
    };
    let count = reply.panes.iter().flatten().count();
    let Some(pane) = reply.panes.iter().flatten().nth(index - 1) else {
        eprintln!("panelink: no pane {index}; run 'panelink panes'");
        return 1;
    };
    let result = link.close(&state.server, pane.target, reply.layout_revision);
    let (message, code) = close_message(index, count == 1, result);
    if code == 0 {
        println!("panelink: {message}");
    } else {
        eprintln!("panelink: {message}");
    }
    code
}

/// Whether storage at `server_dir` can be opened (diagnostic helper for tests).
#[cfg(test)]
fn storage_openable(server_dir: &Path) -> bool {
    panelink::server_storage::ServerStorage::open(server_dir).is_ok()
}

/// A concurrent child may briefly inherit the dropped flock descriptor
/// between fork and exec. Retry only that transient WouldBlock result.
#[cfg(test)]
fn wait_until_storage_openable(server_dir: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match panelink::server_storage::ServerStorage::open(server_dir) {
            Ok(storage) => {
                drop(storage);
                return true;
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
}

/// Read-only `panelink layout show [--previous]`: display the saved layout for
/// the requested slot, or exit with a fixed sanitized error.
pub(crate) fn layout_show(previous: bool) -> i32 {
    let result = layout_show_result(previous);
    match result {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(error) => {
            eprintln!("panelink: {error}");
            1
        }
    }
}

/// Read-only `panelink recovery view [--previous]` entry point.
pub(crate) fn recovery_view(previous: bool) -> i32 {
    use panelink::recovery_reader::{read_recovery, RecoveryReadError};
    use panelink::recovery_view::RecoverySlot;
    use std::io::IsTerminal;

    let state = match resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")) {
        Ok(state) => state,
        Err(_) => {
            eprintln!("panelink: recovery storage unavailable");
            return 1;
        }
    };
    let slot = if previous {
        RecoverySlot::Previous
    } else {
        RecoverySlot::Current
    };
    let view = match read_recovery(&state.server, slot) {
        Ok(view) => view,
        Err(RecoveryReadError::MissingLayout) => {
            eprintln!("panelink: no saved layout");
            return 1;
        }
        Err(RecoveryReadError::InvalidLayout) => {
            eprintln!("panelink: saved layout invalid");
            return 1;
        }
        Err(RecoveryReadError::Storage) => {
            eprintln!("panelink: recovery storage unavailable");
            return 1;
        }
    };

    if !std::io::stdout().is_terminal() {
        println!(
            "slot: {}\ngeneration: {}\nsequence: {}\nsize: {}x{}\norientation: {:?}",
            view.slot,
            view.metadata.generation(),
            view.metadata.snapshot_sequence(),
            view.saved_rows,
            view.saved_columns,
            view.axis
        );
        if view.panes.is_empty() {
            println!("workspace: empty");
        }
        for (index, pane) in view.panes.iter().enumerate() {
            let target = pane.historical_target;
            println!(
                "pane {}: target {}:{}:{}:{}:{}; rows {}; normal {}; alternate {}",
                index + 1,
                target.generation,
                target.workspace,
                target.pane,
                target.session,
                target.occupant_revision,
                availability_history(&pane.rows),
                availability_checkpoint(&pane.normal),
                availability_checkpoint(&pane.alternate)
            );
        }
        return 0;
    }

    match crate::ui_client::recovery::run(view) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("panelink: {message}");
            1
        }
    }
}

/// Explicitly start a fresh default-shell workspace using only the selected
/// saved layout's pane count and split topology.
pub(crate) fn recovery_relaunch(previous: bool) -> i32 {
    use panelink::recovery_links::{LinkOutcome, LinkStore, RelaunchLink};
    use panelink::recovery_reader::{read_recovery, RecoveryReadError};
    use panelink::recovery_view::RecoverySlot;
    use panelink::ui_protocol::planned_split_size;

    let state = match resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let slot = if previous {
        RecoverySlot::Previous
    } else {
        RecoverySlot::Current
    };
    let saved = match read_recovery(&state.server, slot) {
        Ok(view) => view,
        Err(RecoveryReadError::MissingLayout) => {
            eprintln!("panelink: no saved layout");
            return 1;
        }
        Err(RecoveryReadError::InvalidLayout) => {
            eprintln!("panelink: saved layout invalid");
            return 1;
        }
        Err(RecoveryReadError::Storage) => {
            eprintln!("panelink: saved layout invalid");
            return 1;
        }
    };
    if saved.panes.is_empty() {
        eprintln!("panelink: saved layout has no panes");
        return 1;
    }
    let saved_geometry = saved.pane_geometry;
    let saved_dividers = saved.dividers;
    let saved_topology = match relaunch_topology(
        saved.saved_rows,
        saved.saved_columns,
        &saved_geometry,
        &saved_dividers,
    ) {
        Some(topology) => topology,
        None => {
            eprintln!("panelink: saved layout invalid");
            return 1;
        }
    };
    let split_plan = relaunch_split_plan(&saved_topology);

    if relaunch_server_running(&state) {
        eprintln!("panelink: stop the running workspace before relaunching (panelink stop)");
        return 1;
    }

    let _lifecycle = match lifecycle_lock(&state, START_TIMEOUT + STOP_TIMEOUT) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    // A direct `serve` invocation does not take lifecycle.lock, so repeat the
    // read-only check before creating or changing any runtime path.
    if relaunch_server_running(&state) {
        eprintln!("panelink: stop the running workspace before relaunching (panelink stop)");
        return 1;
    }
    if let Err(error) = prepare_root(&state) {
        eprintln!("panelink: {}", error.message());
        return 1;
    }
    if let Err(error) = ensure_terminfo(&state).and_then(|()| ensure_relaunch_server(&state)) {
        eprintln!("panelink: {}", error.message());
        return 1;
    }

    let mut peer = match RelaunchPeer::connect(&state.server) {
        Ok(peer) => peer,
        Err(_) => {
            eprintln!("panelink: could not read the relaunched workspace");
            return 1;
        }
    };
    let mut layout = match query_relaunch_layout(&mut peer) {
        Ok(layout) if layout.panes.iter().flatten().count() == 1 => layout,
        _ => {
            eprintln!("panelink: could not read the relaunched workspace");
            return 1;
        }
    };
    let first = match layout.panes[0] {
        Some(pane) => pane.target,
        None => {
            eprintln!("panelink: could not read the relaunched workspace");
            return 1;
        }
    };
    let mut expected_targets = vec![None; saved.panes.len()];
    expected_targets[0] = Some(first);
    let mut live_saved_order = vec![0_usize];
    let mut split_outcomes = vec![None; saved.panes.len()];

    for (step_index, step) in split_plan.iter().enumerate() {
        let Some(source_index) = live_saved_order
            .iter()
            .position(|saved_index| *saved_index == step.source_saved_index)
        else {
            fill_unattempted_splits(
                &mut split_outcomes,
                &split_plan[step_index..],
                RelaunchSplitStatus::Unknown,
            );
            break;
        };
        let source = match layout.panes.get(source_index).copied().flatten() {
            Some(source) => source.target,
            None => {
                fill_unattempted_splits(
                    &mut split_outcomes,
                    &split_plan[step_index..],
                    RelaunchSplitStatus::Failed,
                );
                break;
            }
        };
        if expected_targets[step.source_saved_index] != Some(source) {
            eprintln!("panelink: relaunched, but current panes could not be verified");
            fill_unattempted_splits(
                &mut split_outcomes,
                &split_plan[step_index..],
                RelaunchSplitStatus::Unknown,
            );
            break;
        }
        let size = match planned_split_size(&layout, source, step.direction) {
            Ok(size) => size,
            Err(_) => {
                eprintln!("panelink: relaunched, but split failed");
                fill_unattempted_splits(
                    &mut split_outcomes,
                    &split_plan[step_index..],
                    RelaunchSplitStatus::Failed,
                );
                break;
            }
        };
        let mut scoped_peer;
        let split_peer = if source == first {
            &mut peer
        } else {
            scoped_peer = match RelaunchPeer::connect_scoped(&state.server, Some(source)) {
                Ok(scoped) => scoped,
                Err(_) => {
                    eprintln!("panelink: relaunched, but split failed");
                    fill_unattempted_splits(
                        &mut split_outcomes,
                        &split_plan[step_index..],
                        RelaunchSplitStatus::Failed,
                    );
                    break;
                }
            };
            &mut scoped_peer
        };
        let (status, new_target, committed_revision) =
            perform_relaunch_split(split_peer, source, layout.revision, step.direction, size);
        split_outcomes[step.new_saved_index] = Some(status);
        if status != RelaunchSplitStatus::Applied {
            fill_unattempted_splits(&mut split_outcomes, &split_plan[step_index + 1..], status);
            break;
        }
        let Some(new_target) = new_target else {
            split_outcomes[step.new_saved_index] = Some(RelaunchSplitStatus::Unknown);
            fill_unattempted_splits(
                &mut split_outcomes,
                &split_plan[step_index + 1..],
                RelaunchSplitStatus::Unknown,
            );
            break;
        };
        expected_targets[step.new_saved_index] = Some(new_target);
        let insertion = live_saved_order
            .iter()
            .position(|saved_index| *saved_index > step.new_saved_index)
            .unwrap_or(live_saved_order.len());
        live_saved_order.insert(insertion, step.new_saved_index);
        let observed = match query_relaunch_layout(&mut peer) {
            Ok(observed) if Some(observed.revision) == committed_revision => observed,
            _ => {
                split_outcomes[step.new_saved_index] = Some(RelaunchSplitStatus::Unknown);
                fill_unattempted_splits(
                    &mut split_outcomes,
                    &split_plan[step_index + 1..],
                    RelaunchSplitStatus::Unknown,
                );
                break;
            }
        };
        let observed_targets = observed
            .panes
            .iter()
            .flatten()
            .map(|pane| pane.target)
            .collect::<Vec<_>>();
        if observed_targets.len() != live_saved_order.len()
            || live_saved_order
                .iter()
                .enumerate()
                .any(|(index, saved_index)| {
                    expected_targets[*saved_index]
                        .is_none_or(|expected| observed_targets.get(index) != Some(&expected))
                })
        {
            split_outcomes[step.new_saved_index] = Some(RelaunchSplitStatus::Unknown);
            fill_unattempted_splits(
                &mut split_outcomes,
                &split_plan[step_index + 1..],
                RelaunchSplitStatus::Unknown,
            );
            break;
        }
        layout = observed;
    }
    drop(peer);

    // The status query is read-only. In particular, no uncertain split is
    // retransmitted under a fresh operation key.
    let current = match query_relaunch_panes(&state.server) {
        Ok(panes) => panes,
        Err(()) => {
            eprintln!("panelink: relaunched, but current panes could not be verified");
            return 1;
        }
    };
    let current_targets: Vec<_> = current
        .panes
        .iter()
        .flatten()
        .map(|pane| pane.target)
        .collect();
    let current_layout = query_layout_snapshot(&state.server);
    let current_matches = current_layout.as_ref().is_some_and(|layout_reply| {
        relaunch_tree_layout_matches(
            layout_reply,
            &current,
            &saved_topology,
            saved.panes.len(),
            &expected_targets,
            first,
            saved.metadata.generation(),
            saved.metadata.workspace(),
        )
    });
    let known_targets_match = current_layout
        .as_ref()
        .is_some_and(|layout_reply| current.layout_revision == layout_reply.revision)
        && current_targets.len() == expected_targets.iter().flatten().count()
        && expected_targets
            .iter()
            .flatten()
            .copied()
            .all(|expected| current_targets.contains(&expected))
        && current_targets.iter().all(|target| {
            target.generation != saved.metadata.generation()
                && target.workspace == saved.metadata.workspace()
        });

    let mut lines = Vec::with_capacity(saved.panes.len());
    for index in 0..saved.panes.len() {
        let new_target = expected_targets[index].filter(|target| {
            current
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == *target)
        });
        let pane_entry = new_target.and_then(|target| {
            current
                .panes
                .iter()
                .flatten()
                .find(|pane| pane.target == target)
                .copied()
        });
        let status = if index == 0 {
            if pane_entry.is_some_and(|pane| {
                pane.live
                    && Some(pane.target) == expected_targets[0]
                    && pane.target.generation != saved.metadata.generation()
                    && pane.target.workspace == saved.metadata.workspace()
            }) {
                RelaunchPaneStatus::Applied
            } else {
                RelaunchPaneStatus::Unknown
            }
        } else {
            match split_outcomes[index].unwrap_or(RelaunchSplitStatus::Failed) {
                RelaunchSplitStatus::Applied
                    if (known_targets_match
                        && (current_targets.len() < saved.panes.len() || current_matches))
                        && pane_entry.is_some_and(|pane| {
                            pane.live && Some(pane.target) == expected_targets[index]
                        }) =>
                {
                    RelaunchPaneStatus::Applied
                }
                RelaunchSplitStatus::Failed => RelaunchPaneStatus::Failed,
                RelaunchSplitStatus::Applied | RelaunchSplitStatus::Unknown => {
                    RelaunchPaneStatus::Unknown
                }
            }
        };
        lines.push((status, saved.panes[index].historical_target, new_target));
    }

    // The link store requires server-singleton ownership. This prototype writes
    // from the CLI under the per-root lifecycle lock after server readiness;
    // that is weaker than holding the server's singleton token and needs review.
    let mut link_failed = false;
    let links = lines
        .iter()
        .filter_map(|(status, old, new)| {
            let Some(new) = new else {
                return None;
            };
            let outcome = match status {
                RelaunchPaneStatus::Applied => Some(LinkOutcome::Applied),
                RelaunchPaneStatus::Unknown => Some(LinkOutcome::Unknown),
                RelaunchPaneStatus::Failed => None,
            };
            let outcome = outcome?;
            Some(RelaunchLink {
                source_generation: saved.metadata.generation(),
                source_sequence: saved.metadata.snapshot_sequence(),
                old_target: *old,
                new_target: *new,
                outcome,
                linked_at_unix_ms: unix_time_ms(),
            })
        })
        .collect::<Vec<_>>();
    if !links.is_empty() {
        if !server_lock_held(&state.server) || !server_ready(&state) {
            link_failed = true;
        } else if let Ok(store) = LinkStore::prepare(&state.server) {
            for link in links {
                if store.record(link, unix_time_ms()).is_err() {
                    link_failed = true;
                    break;
                }
            }
        } else {
            link_failed = true;
        }
    }

    for (index, (status, old, new)) in lines.iter().enumerate() {
        println!(
            "pane {}: old {} -> new {} ({})",
            index + 1,
            target_text(*old),
            new.map(target_text)
                .unwrap_or_else(|| "unavailable".to_owned()),
            status.label()
        );
    }
    if link_failed {
        println!("panelink: relaunched, but link not saved");
    }
    println!("attach with: panelink");
    println!("provider conversations are not resumed; pick them manually");

    if lines
        .iter()
        .all(|(status, _, _)| *status != RelaunchPaneStatus::Failed)
    {
        0
    } else {
        1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RelaunchTopology {
    Leaf(usize),
    Split {
        direction: SplitDirection,
        left: Box<RelaunchTopology>,
        right: Box<RelaunchTopology>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RelaunchSplitStep {
    /// Spatial index of the source pane in the then-current live layout.
    source_index: usize,
    /// Historical spatial index of the source pane represented by this leaf.
    source_saved_index: usize,
    direction: SplitDirection,
    /// Final saved spatial index represented by the newly created pane.
    new_saved_index: usize,
}

fn relaunch_topology(
    rows: u16,
    columns: u16,
    panes: &[PaneGeometry],
    dividers: &[Rect],
) -> Option<RelaunchTopology> {
    if panes.is_empty()
        || panes.len() > panelink::pane_layout::MAX_PANES
        || dividers.len() != panes.len().saturating_sub(1)
    {
        return None;
    }
    let root = Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    };
    if panes.len() == 1 {
        return (panes[0].rect == root).then_some(RelaunchTopology::Leaf(0));
    }
    let indices = (0..panes.len()).collect::<Vec<_>>();
    let mut result = None;
    for (root_divider_index, divider) in dividers.iter().enumerate() {
        let (direction, first_region, second_region) = match split_regions(root, *divider)? {
            Some(regions) => regions,
            None => continue,
        };
        let mut first = Vec::new();
        let mut second = Vec::new();
        for index in &indices {
            let rect = panes.get(*index)?.rect;
            if rect_is_within(rect, first_region) {
                first.push(*index);
            } else if rect_is_within(rect, second_region) {
                second.push(*index);
            } else {
                first.clear();
                second.clear();
                break;
            }
        }
        if first.is_empty() || second.is_empty() {
            continue;
        }
        if result.is_some() {
            return None;
        }
        let bit = 1_u8.checked_shl(u32::try_from(root_divider_index).ok()?)?;
        let left_tree = relaunch_topology_in_region(first_region, &first, panes, dividers, bit)?;
        let right_tree = relaunch_topology_in_region(second_region, &second, panes, dividers, bit)?;
        let (left, left_used) = left_tree;
        let (right, right_used) = right_tree;
        let used = left_used | right_used;
        if used.count_ones() as usize != dividers.len() {
            return None;
        }
        result = Some(RelaunchTopology::Split {
            direction,
            left: Box::new(left),
            right: Box::new(right),
        });
    }
    result
}

fn relaunch_topology_in_region(
    region: Rect,
    pane_indices: &[usize],
    panes: &[PaneGeometry],
    dividers: &[Rect],
    used: u8,
) -> Option<(RelaunchTopology, u8)> {
    if pane_indices.len() == 1 {
        let index = *pane_indices.first()?;
        return (panes.get(index)?.rect == region).then_some((RelaunchTopology::Leaf(index), used));
    }

    let mut candidate = None;
    for (divider_index, divider) in dividers.iter().enumerate() {
        let bit = 1_u8.checked_shl(u32::try_from(divider_index).ok()?)?;
        if used & bit != 0 {
            continue;
        }
        let (direction, first_region, second_region) = match split_regions(region, *divider)? {
            Some(regions) => regions,
            None => continue,
        };
        let mut first = Vec::new();
        let mut second = Vec::new();
        for index in pane_indices {
            let rect = panes.get(*index)?.rect;
            if rect_is_within(rect, first_region) {
                first.push(*index);
            } else if rect_is_within(rect, second_region) {
                second.push(*index);
            } else {
                first.clear();
                second.clear();
                break;
            }
        }
        if first.is_empty() || second.is_empty() {
            continue;
        }
        if candidate.is_some() {
            return None;
        }
        candidate = Some((
            divider_index,
            direction,
            first_region,
            second_region,
            first,
            second,
        ));
    }
    let (divider_index, direction, first_region, second_region, first, second) = candidate?;
    let bit = 1_u8.checked_shl(u32::try_from(divider_index).ok()?)?;
    let (left, left_used) =
        relaunch_topology_in_region(first_region, &first, panes, dividers, used | bit)?;
    let (right, right_used) =
        relaunch_topology_in_region(second_region, &second, panes, dividers, used | bit)?;
    let used = left_used | right_used;
    Some((
        RelaunchTopology::Split {
            direction,
            left: Box::new(left),
            right: Box::new(right),
        },
        used,
    ))
}

fn rect_is_within(rect: Rect, outer: Rect) -> bool {
    rect.x >= outer.x
        && rect.y >= outer.y
        && rect.x.checked_add(rect.columns).is_some_and(|right| {
            outer
                .x
                .checked_add(outer.columns)
                .is_some_and(|outer_right| right <= outer_right)
        })
        && rect.y.checked_add(rect.rows).is_some_and(|bottom| {
            outer
                .y
                .checked_add(outer.rows)
                .is_some_and(|outer_bottom| bottom <= outer_bottom)
        })
}

fn split_regions(region: Rect, divider: Rect) -> Option<Option<(SplitDirection, Rect, Rect)>> {
    if divider.columns == 1
        && divider.y == region.y
        && divider.rows == region.rows
        && divider.x > region.x
        && divider.x < region.x.checked_add(region.columns)?.checked_sub(1)?
    {
        let second_x = divider.x.checked_add(1)?;
        return Some(Some((
            SplitDirection::Right,
            Rect {
                columns: divider.x.checked_sub(region.x)?,
                ..region
            },
            Rect {
                x: second_x,
                columns: region
                    .x
                    .checked_add(region.columns)?
                    .checked_sub(second_x)?,
                ..region
            },
        )));
    }
    if divider.rows == 1
        && divider.x == region.x
        && divider.columns == region.columns
        && divider.y > region.y
        && divider.y < region.y.checked_add(region.rows)?.checked_sub(1)?
    {
        let second_y = divider.y.checked_add(1)?;
        return Some(Some((
            SplitDirection::Down,
            Rect {
                rows: divider.y.checked_sub(region.y)?,
                ..region
            },
            Rect {
                y: second_y,
                rows: region.y.checked_add(region.rows)?.checked_sub(second_y)?,
                ..region
            },
        )));
    }
    Some(None)
}

fn relaunch_split_plan(topology: &RelaunchTopology) -> Vec<RelaunchSplitStep> {
    fn leaf_count(topology: &RelaunchTopology) -> usize {
        match topology {
            RelaunchTopology::Leaf(_) => 1,
            RelaunchTopology::Split { left, right, .. } => leaf_count(left) + leaf_count(right),
        }
    }
    fn append(
        topology: &RelaunchTopology,
        saved_start: usize,
        live_order: &mut Vec<usize>,
        plan: &mut Vec<RelaunchSplitStep>,
    ) {
        match topology {
            RelaunchTopology::Leaf(_) => {}
            RelaunchTopology::Split {
                direction,
                left,
                right,
            } => {
                let left_count = leaf_count(left);
                let source_index = live_order
                    .iter()
                    .position(|saved_index| *saved_index == saved_start)
                    .expect("split source is an extant topology leaf");
                let new_saved_index = saved_start + left_count;
                plan.push(RelaunchSplitStep {
                    source_index,
                    source_saved_index: saved_start,
                    direction: *direction,
                    new_saved_index,
                });
                live_order.insert(source_index + 1, new_saved_index);
                append(left, saved_start, live_order, plan);
                append(right, new_saved_index, live_order, plan);
            }
        }
    }

    let mut plan = Vec::with_capacity(leaf_count(topology).saturating_sub(1));
    let mut live_order = vec![0];
    append(topology, 0, &mut live_order, &mut plan);
    plan
}

fn fill_unattempted_splits(
    outcomes: &mut [Option<RelaunchSplitStatus>],
    remaining: &[RelaunchSplitStep],
    status: RelaunchSplitStatus,
) {
    for step in remaining {
        outcomes[step.new_saved_index].get_or_insert(status);
    }
}

fn query_relaunch_layout(
    peer: &mut RelaunchPeer,
) -> Result<panelink::ui_protocol::LayoutReply, ()> {
    let bytes = peer
        .exchange(&panelink::ui_protocol::encode_layout_query())
        .map_err(|_| ())?;
    panelink::ui_protocol::decode_layout_reply(&bytes).map_err(|_| ())
}

fn query_layout_snapshot(server_directory: &Path) -> Option<panelink::ui_protocol::LayoutReply> {
    let mut peer = RelaunchPeer::connect(server_directory).ok()?;
    query_relaunch_layout(&mut peer).ok()
}

#[allow(clippy::too_many_arguments)]
fn relaunch_tree_layout_matches(
    current_layout: &panelink::ui_protocol::LayoutReply,
    current_panes: &panelink::multi_runtime::PaneReply,
    saved_topology: &RelaunchTopology,
    saved_count: usize,
    expected_targets: &[Option<TargetRef>],
    first: TargetRef,
    old_generation: u64,
    workspace: u64,
) -> bool {
    let geometry = current_layout
        .panes
        .iter()
        .flatten()
        .map(|pane| PaneGeometry {
            target: pane.target,
            rect: pane.rect,
        })
        .collect::<Vec<_>>();
    let dividers = current_layout
        .dividers
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let topology = relaunch_topology(
        current_layout.rows,
        current_layout.columns,
        &geometry,
        &dividers,
    );
    let targets = current_panes
        .panes
        .iter()
        .flatten()
        .map(|pane| pane.target)
        .collect::<Vec<_>>();
    current_panes.layout_revision == current_layout.revision
        && targets.len() == saved_count
        && targets.first() == Some(&first)
        && targets
            .iter()
            .all(|target| target.generation != old_generation && target.workspace == workspace)
        && expected_targets
            .iter()
            .zip(&targets)
            .all(|(expected, actual)| expected.is_some_and(|expected| expected == *actual))
        && topology.as_ref() == Some(saved_topology)
}
fn relaunch_server_running(state: &StateRoot) -> bool {
    server_lock_held(&state.server)
        || (endpoint_state(&state.socket_path()) == EndpointState::Socket && server_ready(state))
}

/// Caller holds the per-root lifecycle lock, and has already refused an
/// existing live workspace.
fn ensure_relaunch_server(state: &StateRoot) -> Result<(), OpenError> {
    let endpoint = endpoint_state(&state.socket_path());
    match decide_endpoint(endpoint, server_lock_held(&state.server)) {
        EndpointAction::Refuse => return Err(OpenError::StaleEndpointUnsafe),
        EndpointAction::UseRunning => return Err(OpenError::StopFailed),
        EndpointAction::RemoveStaleThenStart => {
            fs::remove_file(state.socket_path()).map_err(|_| OpenError::StaleEndpointUnsafe)?;
        }
        EndpointAction::Start => {}
    }
    let child = spawn_server(state)?;
    wait_ready(state, Some(child))
}

fn perform_relaunch_split(
    peer: &mut RelaunchPeer,
    target: TargetRef,
    layout_revision: u64,
    direction: panelink::pane_layout::SplitDirection,
    (rows, columns): (u16, u16),
) -> (RelaunchSplitStatus, Option<TargetRef>, Option<u64>) {
    use panelink::managed_launch_protocol::LaunchRequest;
    use panelink::multi_startup::environment_entries;
    use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
    use std::os::unix::ffi::OsStrExt;

    let context = match capture_relaunch_context() {
        Ok(context) => context,
        Err(()) => {
            eprintln!("panelink: relaunched, but split failed");
            return (RelaunchSplitStatus::Failed, None, None);
        }
    };
    let key = operation_key(peer.context.context());
    let cwd = context.working_directory.as_os_str().as_bytes();
    let environment = environment_entries(&context.environment);
    let shell = default_shell(std::env::var_os("SHELL"));
    let launch = LaunchRequest {
        key,
        rows,
        columns,
        requested_executable: shell.as_os_str().as_bytes(),
        requested_working_directory: cwd,
        client_working_directory: cwd,
        arguments: &[],
        environment: &environment,
    };
    let mut payload = Vec::new();
    let encoded = encode_split_request(
        &SplitRequest {
            key,
            target,
            expected_layout_revision: layout_revision,
            direction,
            launch: &launch,
        },
        &mut payload,
    );
    drop(context);
    if encoded.is_err() {
        eprintln!("panelink: relaunched, but split failed");
        return (RelaunchSplitStatus::Failed, None, None);
    }
    match peer.context.reserve() {
        Ok(reserved) if reserved == key => {}
        _ => {
            eprintln!("panelink: relaunched, but split failed");
            return (RelaunchSplitStatus::Failed, None, None);
        }
    }
    let reply = peer.exchange(&payload);
    drop(payload);
    match reply.and_then(|bytes| decode_split_result(&bytes).map_err(|_| "response_invalid")) {
        Ok(result) if result.key == key && result.existing_target == target => {
            let _ = peer.context.ack(key);
            let status = match result.outcome {
                panelink::protocol::Outcome::Applied if result.new_target.is_some() => {
                    RelaunchSplitStatus::Applied
                }
                panelink::protocol::Outcome::Failed => RelaunchSplitStatus::Failed,
                _ => RelaunchSplitStatus::Unknown,
            };
            (
                status,
                result.new_target,
                (status == RelaunchSplitStatus::Applied)
                    .then_some(result.committed_layout_revision),
            )
        }
        _ => (RelaunchSplitStatus::Unknown, None, None),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RelaunchSplitStatus {
    Applied,
    Unknown,
    Failed,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RelaunchPaneStatus {
    Applied,
    Unknown,
    Failed,
}

impl RelaunchPaneStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
        }
    }
}

fn target_text(target: TargetRef) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

#[cfg(test)]
mod relaunch_topology_tests {
    use super::*;

    fn rect(x: u16, y: u16, rows: u16, columns: u16) -> Rect {
        Rect {
            x,
            y,
            rows,
            columns,
        }
    }

    fn panes(rects: &[Rect]) -> Vec<PaneGeometry> {
        rects
            .iter()
            .enumerate()
            .map(|(index, rect)| PaneGeometry {
                target: TargetRef {
                    generation: 1,
                    workspace: 1,
                    pane: index as u64 + 1,
                    session: index as u64 + 1,
                    occupant_revision: 1,
                },
                rect: *rect,
            })
            .collect()
    }

    #[test]
    fn split_plan_covers_single_two_three_and_nested_t_junction_topologies() {
        let single = relaunch_topology(24, 80, &panes(&[rect(0, 0, 24, 80)]), &[]).unwrap();
        assert!(relaunch_split_plan(&single).is_empty());

        let two = relaunch_topology(
            24,
            80,
            &panes(&[rect(0, 0, 24, 39), rect(40, 0, 24, 40)]),
            &[rect(39, 0, 24, 1)],
        )
        .unwrap();
        assert_eq!(
            relaunch_split_plan(&two),
            vec![RelaunchSplitStep {
                source_index: 0,
                source_saved_index: 0,
                direction: SplitDirection::Right,
                new_saved_index: 1,
            }]
        );

        let three = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 24, 39),
                rect(40, 0, 11, 40),
                rect(40, 12, 12, 40),
            ]),
            &[rect(39, 0, 24, 1), rect(40, 11, 1, 40)],
        )
        .unwrap();
        assert_eq!(
            relaunch_split_plan(&three),
            vec![
                RelaunchSplitStep {
                    source_index: 0,
                    source_saved_index: 0,
                    direction: SplitDirection::Right,
                    new_saved_index: 1,
                },
                RelaunchSplitStep {
                    source_index: 1,
                    source_saved_index: 1,
                    direction: SplitDirection::Down,
                    new_saved_index: 2,
                },
            ]
        );

        // A nested T junction: split the root horizontally, then split only
        // the right-hand child vertically.
        let nested_t = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 24, 39),
                rect(40, 0, 11, 40),
                rect(40, 12, 12, 40),
            ]),
            &[rect(39, 0, 24, 1), rect(40, 11, 1, 40)],
        )
        .unwrap();
        let plan = relaunch_split_plan(&nested_t);
        assert_eq!(
            plan.iter()
                .map(|step| (step.source_index, step.direction))
                .collect::<Vec<_>>(),
            vec![(0, SplitDirection::Right), (1, SplitDirection::Down)]
        );

        // Four leaves nested by splitting the top-left and bottom-left
        // quadrants after the root horizontal divider.
        let four = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 11, 39),
                rect(40, 0, 11, 40),
                rect(0, 12, 12, 39),
                rect(40, 12, 12, 40),
            ]),
            &[rect(0, 11, 1, 80), rect(39, 0, 11, 1), rect(39, 12, 12, 1)],
        )
        .unwrap();
        let plan = relaunch_split_plan(&four);
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan.iter()
                .map(|step| (step.source_index, step.direction, step.new_saved_index))
                .collect::<Vec<_>>(),
            vec![
                (0, SplitDirection::Down, 2),
                (0, SplitDirection::Right, 1),
                (2, SplitDirection::Right, 3),
            ]
        );
    }

    #[test]
    fn topology_comparison_ignores_split_proportions_but_preserves_tree() {
        let saved = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 24, 39),
                rect(40, 0, 11, 40),
                rect(40, 12, 12, 40),
            ]),
            &[rect(39, 0, 24, 1), rect(40, 11, 1, 40)],
        )
        .unwrap();
        let resized_proportion = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 24, 31),
                rect(32, 0, 11, 48),
                rect(32, 12, 12, 48),
            ]),
            &[rect(31, 0, 24, 1), rect(32, 11, 1, 48)],
        )
        .unwrap();
        assert_eq!(saved, resized_proportion);

        let different_topology = relaunch_topology(
            24,
            80,
            &panes(&[
                rect(0, 0, 24, 39),
                rect(40, 0, 11, 40),
                rect(40, 12, 12, 40),
            ]),
            &[rect(39, 0, 24, 1), rect(40, 11, 1, 40)],
        )
        .unwrap();
        let another_topology = relaunch_topology(
            24,
            80,
            &panes(&[rect(0, 0, 11, 39), rect(40, 0, 11, 40), rect(0, 12, 12, 80)]),
            &[rect(0, 11, 1, 80), rect(39, 0, 11, 1)],
        )
        .unwrap();
        assert_ne!(different_topology, another_topology);
    }
}

fn operation_key(
    context: panelink::client_context::ClientContext,
) -> panelink::protocol::OperationKey {
    panelink::protocol::OperationKey {
        generation: context.generation(),
        lease: context.lease(),
        sequence: context.next_sequence(),
    }
}

fn capture_relaunch_context() -> Result<panelink::launch::LaunchContext, ()> {
    use std::os::unix::ffi::OsStrExt;

    let working_directory = std::env::current_dir().map_err(|_| ())?;
    let mut environment = panelink::launch::Environment::new();
    let mut total_bytes = 0_usize;
    for (name, value) in std::env::vars_os() {
        if name.as_bytes().starts_with(b"PANELINK_") {
            continue;
        }
        total_bytes = total_bytes
            .checked_add(name.as_bytes().len())
            .and_then(|total| total.checked_add(value.as_bytes().len()))
            .and_then(|total| total.checked_add(6))
            .filter(|total| *total <= 48 * 1024)
            .ok_or(())?;
        if environment.len() >= 1_024 {
            return Err(());
        }
        environment.insert(name, value);
    }
    Ok(panelink::launch::LaunchContext {
        working_directory,
        environment,
    })
}

fn query_relaunch_panes(server_directory: &Path) -> Result<panelink::multi_runtime::PaneReply, ()> {
    let mut peer = RelaunchPeer::connect(server_directory).map_err(|_| ())?;
    let payload = panelink::multi_runtime::encode_pane_query();
    let bytes = peer.exchange(&payload).map_err(|_| ())?;
    panelink::multi_runtime::decode_pane_reply(&bytes).map_err(|_| ())
}

struct RelaunchPeer {
    stream: AuthenticatedStream,
    context: panelink::client_context::ContextFile,
    next_request: u64,
}

impl RelaunchPeer {
    fn connect(server_directory: &Path) -> Result<Self, &'static str> {
        Self::connect_scoped(server_directory, None)
    }

    /// Connect with a lease scoped to `target`. The server pins every new
    /// lease to one pane (the initial pane when no target is named), and a
    /// split may only name the caller's own pane, so splitting a pane created
    /// by an earlier relaunch step needs a connection scoped to that pane.
    fn connect_scoped(
        server_directory: &Path,
        target: Option<TargetRef>,
    ) -> Result<Self, &'static str> {
        use panelink::handshake_protocol::{
            validate_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ,
        };
        use panelink::protocol_wire::{Frame, MessageKind};
        use panelink::session_protocol::{encode_session_hello, SessionHello};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::UNIX_EPOCH;

        static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
        let mut stream =
            AuthenticatedStream::connect(server_directory, Instant::now() + Duration::from_secs(2))
                .map_err(|_| "transport_unavailable")?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let nonce = (time ^ (u64::from(std::process::id()) << 32))
            .wrapping_add(NEXT_NONCE.fetch_add(1, Ordering::Relaxed))
            .max(1);
        let hello = Hello {
            nonce,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT,
        };
        let mut payload = encode_session_hello(&SessionHello {
            hello,
            resume: None,
        })
        .map_err(|_| "handshake_invalid")?;
        if let Some(target) = target {
            let trailer = panelink::ui_protocol::encode_attach_target(target)
                .map_err(|_| "request_invalid")?;
            payload.extend_from_slice(&trailer);
        }
        let frame = Frame::new(MessageKind::Hello, 0, &payload).map_err(|_| "handshake_invalid")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        stream
            .write_frame(&frame, deadline)
            .map_err(|_| "transport_failed")?;
        let reply = stream
            .read_frame(deadline)
            .map_err(|_| "transport_failed")?;
        if reply.request_id() != 0 {
            return Err("handshake_invalid");
        }
        if reply.kind() != MessageKind::Welcome {
            return Err("server_rejected");
        }
        let welcome = panelink::handshake_protocol::decode_welcome(reply.payload())
            .map_err(|_| "handshake_invalid")?;
        validate_welcome(&hello, &welcome).map_err(|_| "handshake_invalid")?;
        if welcome.selected_capabilities & CAPABILITY_INPUT == 0 {
            return Err("split_unavailable");
        }
        let context_path = relaunch_context_path(welcome.generation, welcome.lease);
        let context = panelink::client_context::ContextFile::create(
            &context_path,
            welcome.generation,
            welcome.lease,
            Instant::now() + Duration::from_secs(2),
        )
        .map_err(|_| "context_storage_failed")?;
        Ok(Self {
            stream,
            context,
            next_request: 1,
        })
    }

    fn exchange(&mut self, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
        use panelink::protocol_wire::{Frame, MessageKind};

        let request_id = self.next_request;
        self.next_request = self.next_request.checked_add(1).ok_or("request_invalid")?;
        let frame =
            Frame::new(MessageKind::Request, request_id, payload).map_err(|_| "request_invalid")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        self.stream
            .write_frame(&frame, deadline)
            .map_err(|_| "transport_failed")?;
        let reply = self
            .stream
            .read_frame(deadline)
            .map_err(|_| "transport_failed")?;
        if reply.request_id() != request_id {
            return Err("response_invalid");
        }
        match reply.kind() {
            MessageKind::Response => Ok(reply.payload().to_vec()),
            MessageKind::Error => Err("server_rejected"),
            _ => Err("response_invalid"),
        }
    }
}

fn relaunch_context_path(generation: u64, lease: u64) -> PathBuf {
    static NEXT_CONTEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let unique = NEXT_CONTEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::var_os("PANELINK_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Application Support/Panelink"))
        })
        .unwrap_or_default();
    root.join("contexts").join(format!(
        "relaunch-{}-{generation:x}-{lease:x}-{now:x}-{unique:x}.ctx",
        std::process::id()
    ))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(1)
}

fn availability_history(source: &panelink::recovery_view::HistoryAvailability) -> &'static str {
    use panelink::recovery_view::HistoryAvailability;
    match source {
        HistoryAvailability::Available { .. } => "available",
        HistoryAvailability::Missing => "missing",
        HistoryAvailability::Invalid => "invalid",
    }
}

fn availability_checkpoint(
    source: &panelink::recovery_view::CheckpointAvailability,
) -> &'static str {
    use panelink::recovery_view::CheckpointAvailability;
    match source {
        CheckpointAvailability::Available { .. } => "available",
        CheckpointAvailability::Missing => "missing",
        CheckpointAvailability::Invalid => "invalid",
    }
}

fn layout_show_result(previous: bool) -> Result<String, &'static str> {
    let state = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))
        .map_err(|_| "layout directory unavailable")?;
    // Absence is "no saved layout"; an unsafe/unreadable directory is not.
    let store = match LayoutStore::open(&state.server.join("layout")) {
        Ok(store) => store,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err("no saved layout"),
        Err(_) => return Err("saved layout invalid"),
    };
    let slot = if previous {
        LayoutSlot::Previous
    } else {
        LayoutSlot::Current
    };
    let layout = store.load(slot).map_err(|_| "saved layout invalid")?;
    let layout = layout.ok_or("no saved layout")?;
    let mut output = format_layout_output(&layout, previous);
    output.push_str("history: rows/screens are separate read-only sources; use `panelink history show TARGET`\n");
    Ok(output)
}

fn format_layout_output(layout: &SavedLayout, previous: bool) -> String {
    let mut text = String::new();
    let slot_label = if previous { "previous" } else { "current" };
    text.push_str(&format!("saved layout: {slot_label}\n"));
    let meta = layout.metadata();
    text.push_str(&format!("generation: {}\n", meta.generation()));
    text.push_str(&format!("workspace: {}\n", meta.workspace()));
    text.push_str(&format!(
        "snapshot sequence: {}\n",
        meta.snapshot_sequence()
    ));
    text.push_str(&format!("layout revision: {}\n", meta.layout_revision()));
    text.push_str(&format!(
        "observed at unix ms: {}\n",
        meta.observed_at_unix_ms()
    ));
    match layout.pane_count() {
        0 => {
            text.push_str("panes: none (empty workspace snapshot)\n");
        }
        _ => {
            text.push_str(&format!("size: {}x{}\n", layout.rows(), layout.columns()));
            let axis_label = match layout.axis() {
                LayoutAxis::Single => "single",
                LayoutAxis::SideBySide => "side-by-side",
                LayoutAxis::Stacked => "stacked",
            };
            text.push_str(&format!("orientation: {axis_label}\n"));
            for (index, maybe_pane) in layout.panes().iter().enumerate() {
                if let Some(target) = maybe_pane {
                    text.push_str(&format!(
                        "pane {}: {}:{}:{}:{}:{}\n",
                        index + 1,
                        target.generation,
                        target.workspace,
                        target.pane,
                        target.session,
                        target.occupant_revision
                    ));
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelink::workspace_index::store::{StoreError, WorkspaceIndexStore};
    use std::cell::Cell;
    use std::sync::mpsc;

    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().canonicalize().unwrap();
        let path = base.join(format!(
            "pl-open-{name}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }

    #[test]
    fn registry_read_retry_is_locked_only_cancellable_and_deadline_bounded() {
        let start = Instant::now();
        let clock = Cell::new(start);
        let mut attempts = 0;
        let mut waits = Vec::new();
        let mut operation = || {
            attempts += 1;
            if attempts < 3 {
                Err(StoreError::Locked)
            } else {
                Ok("read")
            }
        };
        let read = retry_registry_read_with(
            &mut operation,
            start + REGISTRY_READ_TIMEOUT,
            || clock.get(),
            |delay| {
                waits.push(delay);
                clock.set(clock.get() + delay);
            },
            || false,
        );
        assert_eq!(read, Ok("read"));
        assert_eq!(attempts, 3);
        assert_eq!(waits.as_slice(), &[REGISTRY_READ_BACKOFF; 2]);

        let attempts = Cell::new(0);
        let mut corrupt = || {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(StoreError::CorruptRecord)
        };
        assert_eq!(
            retry_registry_read_with(
                &mut corrupt,
                start + REGISTRY_READ_TIMEOUT,
                || start,
                |_| panic!("corrupt records must not retry"),
                || false,
            ),
            Err(StoreError::CorruptRecord)
        );
        assert_eq!(attempts.get(), 1);

        let absent_attempts = Cell::new(0);
        let mut absent = || {
            absent_attempts.set(absent_attempts.get() + 1);
            Ok(None::<()>)
        };
        assert_eq!(
            retry_registry_read_with(
                &mut absent,
                start + REGISTRY_READ_TIMEOUT,
                || start,
                |_| panic!("missing records must not retry"),
                || false,
            ),
            Ok(None)
        );
        assert_eq!(absent_attempts.get(), 1);

        let clock = Cell::new(start);
        let attempts = Cell::new(0);
        let mut locked = || {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(StoreError::Locked)
        };
        assert_eq!(
            retry_registry_read_with(
                &mut locked,
                start + Duration::from_millis(20),
                || clock.get(),
                |delay| clock.set(clock.get() + delay),
                || false,
            ),
            Err(StoreError::Locked)
        );
        assert!(clock.get() <= start + Duration::from_millis(20));
        assert!(attempts.get() <= 5);

        let mut never_attempt =
            || -> Result<(), StoreError> { panic!("canceled reads must not reach the store") };
        assert_eq!(
            retry_registry_read_with(
                &mut never_attempt,
                start + REGISTRY_READ_TIMEOUT,
                || start,
                |_| panic!("canceled reads must not wait"),
                || true,
            ),
            Err(StoreError::Locked)
        );
    }

    #[test]
    fn workspace_catalog_retries_real_lock_contention_without_mutating_registry() {
        let root = temp_dir("catalog-lock");
        let mut locked_store = WorkspaceIndexStore::open(&root).unwrap();
        locked_store.initialize_legacy().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(locked_store);
        });
        ready_rx.recv().unwrap();

        let (locked_tx, locked_rx) = mpsc::channel();
        let reader_root = root.clone();
        let reader = thread::spawn(move || {
            workspace_catalog_with_loader(
                &reader_root,
                1,
                1,
                || false,
                || {
                    let result = load_workspace_record(&reader_root);
                    if matches!(&result, Err(StoreError::Locked)) {
                        let _ = locked_tx.send(());
                    }
                    result
                },
            )
        });
        locked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("catalog reader observes the held registry lock");
        release_tx.send(()).unwrap();

        let (index, route) = reader.join().unwrap().unwrap();
        owner.join().unwrap();
        assert_eq!((route.project.get(), route.tab.get()), (1, 1));
        assert!(index.project(route.project).is_ok());
        let check = WorkspaceIndexStore::open(&root).unwrap();
        assert_eq!(check.load().unwrap().unwrap().revision(), 1);
        drop(check);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_prefers_panelink_home_then_library() {
        let state = resolve_root(Some("/tmp/x".into()), Some("/Users/u".into())).unwrap();
        assert_eq!(state.root, PathBuf::from("/tmp/x"));
        assert_eq!(state.server, PathBuf::from("/tmp/x/server"));
        let state = resolve_root(None, Some("/Users/u".into())).unwrap();
        assert_eq!(
            state.root,
            PathBuf::from("/Users/u/Library/Application Support/Panelink")
        );
        assert_eq!(resolve_root(None, None), Err(OpenError::HomeUnavailable));
        assert_eq!(
            resolve_root(Some("relative".into()), None),
            Err(OpenError::StateRootUnavailable)
        );
    }

    #[test]
    fn overlong_socket_path_is_rejected() {
        let long = format!("/{}", "a".repeat(100));
        assert_eq!(
            resolve_root(Some(long.into()), None),
            Err(OpenError::SocketPathTooLong)
        );
    }

    #[test]
    fn private_directory_permissions_are_enforced() {
        let dir = temp_dir("perm");
        let sub = dir.join("root");
        ensure_private_dir(&sub).unwrap();
        assert_eq!(fs::metadata(&sub).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(ensure_private_dir(&sub), Err(OpenError::StateRootUnsafe));
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&sub, &link).unwrap();
        assert_eq!(ensure_private_dir(&link), Err(OpenError::StateRootUnsafe));
        let file = dir.join("file");
        fs::write(&file, b"x").unwrap();
        assert_eq!(ensure_private_dir(&file), Err(OpenError::StateRootUnsafe));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn endpoint_decisions_never_touch_a_live_server_or_foreign_entry() {
        use EndpointAction::*;
        use EndpointState::*;
        assert_eq!(decide_endpoint(Socket, true), UseRunning);
        assert_eq!(decide_endpoint(Absent, true), UseRunning);
        assert_eq!(decide_endpoint(Absent, false), Start);
        assert_eq!(decide_endpoint(Socket, false), RemoveStaleThenStart);
        assert_eq!(decide_endpoint(Foreign, false), Refuse);
        assert_eq!(decide_endpoint(Foreign, true), Refuse);
    }

    #[test]
    fn endpoint_state_classifies_files_and_sockets() {
        let dir = temp_dir("ep");
        assert_eq!(endpoint_state(&dir.join("none")), EndpointState::Absent);
        fs::write(dir.join("file"), b"x").unwrap();
        assert_eq!(endpoint_state(&dir.join("file")), EndpointState::Foreign);
        let socket = dir.join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(endpoint_state(&socket), EndpointState::Socket);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lock_probe_detects_a_held_storage_lock() {
        let dir = temp_dir("lock");
        let server = dir.join("server");
        assert!(!server_lock_held(&server));
        ensure_private_dir(&server).unwrap();
        let storage = panelink::server_storage::ServerStorage::initialize(&server).unwrap();
        assert!(server_lock_held(&server));
        assert!(!storage_openable(&server));
        drop(storage);
        assert!(!server_lock_held(&server));
        assert!(wait_until_storage_openable(&server));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn terminfo_compile_decision_tracks_source_fingerprint() {
        let dir = temp_dir("ti");
        assert!(terminfo_needs_compile(&dir, "a"));
        fs::create_dir_all(dir.join("p")).unwrap();
        fs::write(dir.join("p/panelink-probe"), b"x").unwrap();
        assert!(terminfo_needs_compile(&dir, "a"));
        fs::write(dir.join("source.fingerprint"), terminfo_fingerprint("a")).unwrap();
        assert!(!terminfo_needs_compile(&dir, "a"));
        assert!(terminfo_needs_compile(&dir, "b"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn embedded_terminfo_compiles() {
        let dir = temp_dir("tic");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        ensure_terminfo(&state).unwrap();
        assert!(!terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_terminfo_compiles_both_succeed() {
        let dir = temp_dir("tic-race");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        // Distinct staging per thread mirrors distinct processes.
        let results: Vec<_> = (0..4)
            .map(|index| {
                let state = state.clone();
                thread::spawn(move || {
                    let staging = state.root.join(format!("terminfo.staging.t{index}"));
                    ensure_private_dir(&staging).unwrap();
                    let result = compile_and_publish(&state, &staging);
                    let _ = fs::remove_dir_all(&staging);
                    result
                })
            })
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert!(!terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identity_record_parsing_is_strict() {
        assert_eq!(parse_identity("123 456\n"), Some((123, 456)));
        assert_eq!(parse_identity("1 456"), None);
        assert_eq!(parse_identity("123"), None);
        assert_eq!(parse_identity("123 0"), None);
        assert_eq!(parse_identity("123 456 7"), None);
        assert_eq!(parse_identity("-5 456"), None);
    }

    #[test]
    fn process_identity_matches_only_the_same_live_process() {
        let me = i32::try_from(std::process::id()).unwrap();
        let start = process_start_time(me).expect("own start time");
        assert_eq!(process_start_time(me), Some(start));
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        child.wait().unwrap();
        assert_eq!(
            process_start_time(pid),
            None,
            "reaped process has no identity"
        );
        // launchd (pid 1) belongs to root, not this user.
        assert_eq!(process_start_time(1), None);
    }

    #[test]
    fn ancestors_must_not_be_writable_by_others() {
        let dir = temp_dir("anc");
        let open = dir.join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
        assert_eq!(
            ancestors_ok(&open.join("root"), current_uid()),
            Err(OpenError::StateRootUnsafe)
        );
        assert_eq!(ancestors_ok(&dir.join("root"), current_uid()), Ok(()));
        assert_eq!(
            ancestors_ok(Path::new("/tmp/x/root"), current_uid()),
            Ok(())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_is_exclusive_until_dropped() {
        let dir = temp_dir("life");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        let held = lifecycle_lock(&state, Duration::from_millis(50)).unwrap();
        assert!(lifecycle_lock(&state, Duration::from_millis(100)).is_err());
        drop(held);
        assert!(lifecycle_lock(&state, Duration::from_millis(100)).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pane_index_accepts_only_small_positive_decimals() {
        for (text, expected) in [("1", Some(1)), ("2", Some(2)), ("0042", Some(42))] {
            assert_eq!(parse_pane_index(std::ffi::OsStr::new(text)), expected);
        }
        for text in ["", "0", "-1", "+1", "1.0", "x", "12345", " 1"] {
            assert_eq!(
                parse_pane_index(std::ffi::OsStr::new(text)),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn close_messages_and_exit_codes_cover_every_outcome() {
        use crate::p3_cli::CloseFailure;
        use panelink::close_protocol::{CloseOutcome, CloseRejection, CloseResult, StopOutcome};
        use panelink::control_protocol::TargetRef;
        use panelink::protocol::OperationKey;
        let result = |outcome, pending| {
            Ok(CloseResult {
                key: OperationKey {
                    generation: 1,
                    lease: 2,
                    sequence: 3,
                },
                outcome,
                target: Some(TargetRef {
                    generation: 1,
                    workspace: 1,
                    pane: 2,
                    session: 2,
                    occupant_revision: 1,
                }),
                layout_revision: 4,
                survivor_resize_pending: pending,
            })
        };
        let exited = CloseOutcome::Stopped(StopOutcome::Exited);
        let escalated = CloseOutcome::Stopped(StopOutcome::Escalated);
        assert_eq!(
            close_message(2, false, result(exited, false)),
            ("closed pane 2 (exited)".to_owned(), 0)
        );
        assert_eq!(
            close_message(2, false, result(escalated, false)),
            ("closed pane 2 (escalated: SIGKILL)".to_owned(), 0)
        );
        assert_eq!(
            close_message(1, true, result(exited, false)),
            (
                "closed pane 1 (exited); the workspace has ended".to_owned(),
                0
            )
        );
        assert_eq!(close_message(1, false, result(exited, true)).1, 0);
        let (message, code) = close_message(
            2,
            false,
            result(CloseOutcome::Stopped(StopOutcome::Unknown), false),
        );
        assert!(message.starts_with("close status unknown"), "{message}");
        assert_eq!(code, 1);
        let (message, code) = close_message(
            2,
            false,
            result(CloseOutcome::Rejected(CloseRejection::StaleLayout), false),
        );
        assert!(message.starts_with("close rejected"), "{message}");
        assert_eq!(code, 1);
        for failure in [
            CloseFailure::Refused,
            CloseFailure::Unknown,
            CloseFailure::NotSent("transport_failed"),
        ] {
            let (message, code) = close_message(2, false, Err(failure));
            assert_eq!(code, 1);
            assert!(!message.contains("transport_failed"), "{message}");
        }
        assert!(close_message(2, false, Err(CloseFailure::Unknown))
            .0
            .starts_with("close status unknown"));
    }

    #[test]
    fn default_shell_requires_absolute_executable() {
        assert_eq!(default_shell(None), PathBuf::from("/bin/zsh"));
        assert_eq!(default_shell(Some("zsh".into())), PathBuf::from("/bin/zsh"));
        assert_eq!(
            default_shell(Some("/nonexistent/sh".into())),
            PathBuf::from("/bin/zsh")
        );
        assert_eq!(
            default_shell(Some("/bin/sh".into())),
            PathBuf::from("/bin/sh")
        );
    }
}
