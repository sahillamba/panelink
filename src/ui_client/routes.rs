//! Bounded client-side preparation and persistence for workspace switching.
//!
//! A prepared `Net` remains owned by the route thread until the UI takes its
//! matching handoff slot. Cancellation and controller drop never drop a raw
//! `Net` on the host thread.
//!
//! This slice uses one `Ready` result instead of separate `Prepared` and
//! `Ready` signals. The route worker performs its final registry revalidation
//! after connect and before publishing the offer, so there is no host-visible
//! adoption phase between the two states.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use panelink::workspace_index::{ProjectId, TabPresentationSnapshot};
use panelink::workspace_tabs::WorkspaceRouteId;

use crate::workspace_lifecycle::{
    CloseOutcome, CreateIntentId, CreateShellTab, ExpectedRuntimeIdentity, LifecycleError,
    LifecycleSession, ProjectLaunchContext, RenameOutcome,
};

use super::net::Net;
use super::state::App;

const POLL: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RouteRequest {
    pub(crate) request_id: u64,
    pub(crate) destination: WorkspaceRouteId,
    pub(crate) host_size: (u16, u16),
}

pub(crate) struct PreparedAttachment {
    pub(crate) request_id: u64,
    pub(crate) route: WorkspaceRouteId,
    pub(crate) app: App,
    pub(crate) net: Net,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RouteResult {
    Ready {
        request_id: u64,
    },
    Failed {
        request_id: u64,
        label: &'static str,
    },
}

enum Command {
    Prepare(RouteRequest),
}

enum MetadataCommand {
    Wake,
    Mutation(MetadataMutation),
}

enum MetadataMutation {
    Create {
        request_id: MetadataRequestId,
        project: ProjectId,
        intent_id: CreateIntentId,
        launch: ProjectLaunchContext,
    },
    Rename {
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        name: String,
        expected_revision: u64,
    },
    Close {
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        expected: ExpectedRuntimeIdentity,
    },
}

trait MetadataBackend: Send + 'static {
    fn save_selection(&mut self, route: WorkspaceRouteId) -> Result<(), ()>;
    fn load_catalog(&mut self) -> Result<Option<(TabPresentationSnapshot, u64)>, ()>;

    fn create_shell_tab(
        &mut self,
        _project: ProjectId,
        _intent: CreateIntentId,
        _session: &mut LifecycleSession,
        _launch: &ProjectLaunchContext,
    ) -> Result<CreateShellTab, LifecycleError> {
        Err(LifecycleError::MetadataUnavailable)
    }

    fn rename_tab(
        &mut self,
        _route: WorkspaceRouteId,
        _name: &str,
        _expected_revision: u64,
    ) -> Result<RenameOutcome, LifecycleError> {
        Err(LifecycleError::MetadataUnavailable)
    }

    fn close_tab(
        &mut self,
        _route: WorkspaceRouteId,
        _expected: &ExpectedRuntimeIdentity,
    ) -> CloseOutcome {
        CloseOutcome::Unknown {
            reason: "workspace metadata unavailable",
        }
    }

    fn presentation_snapshot(&mut self) -> Result<(TabPresentationSnapshot, u64), LifecycleError> {
        Err(LifecycleError::MetadataUnavailable)
    }
}

struct StoreMetadataBackend {
    root: PathBuf,
}

impl MetadataBackend for StoreMetadataBackend {
    fn save_selection(&mut self, route: WorkspaceRouteId) -> Result<(), ()> {
        crate::default_open::save_workspace_selection(&self.root, route).map_err(|_| ())
    }

    fn load_catalog(&mut self) -> Result<Option<(TabPresentationSnapshot, u64)>, ()> {
        let Some(index) =
            crate::default_open::load_workspace_index_snapshot(&self.root).map_err(|_| ())?
        else {
            return Ok(None);
        };
        let (snapshot, revision) =
            crate::workspace_lifecycle::presentation_snapshot(&self.root).map_err(|_| ())?;
        if snapshot.projects.iter().any(|project| {
            index.project(project.id).ok().is_none_or(|registered| {
                project.tabs.iter().any(|tab| {
                    !registered
                        .tabs()
                        .any(|catalog_tab| catalog_tab.id == tab.route.tab)
                })
            })
        }) {
            return Err(());
        }
        Ok(Some((snapshot, revision)))
    }

    fn create_shell_tab(
        &mut self,
        project: ProjectId,
        intent: CreateIntentId,
        session: &mut LifecycleSession,
        launch: &ProjectLaunchContext,
    ) -> Result<CreateShellTab, LifecycleError> {
        crate::workspace_lifecycle::create_shell_tab(&self.root, project, intent, session, launch)
    }

    fn rename_tab(
        &mut self,
        route: WorkspaceRouteId,
        name: &str,
        expected_revision: u64,
    ) -> Result<RenameOutcome, LifecycleError> {
        crate::workspace_lifecycle::rename_tab(&self.root, route, name, expected_revision)
    }

    fn close_tab(
        &mut self,
        route: WorkspaceRouteId,
        expected: &ExpectedRuntimeIdentity,
    ) -> CloseOutcome {
        crate::workspace_lifecycle::close_tab(&self.root, route, expected)
    }

    fn presentation_snapshot(&mut self) -> Result<(TabPresentationSnapshot, u64), LifecycleError> {
        crate::workspace_lifecycle::presentation_snapshot(&self.root)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MetadataRequestId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataQueueError {
    Busy,
    Unavailable,
    IdentifierExhausted,
}

pub(crate) enum MetadataResult {
    Catalog {
        source_route: Option<WorkspaceRouteId>,
        snapshot: TabPresentationSnapshot,
        observed_revision: u64,
    },
    CatalogError,
    SaveError(&'static str),
    Created {
        request_id: MetadataRequestId,
        intent_id: CreateIntentId,
        project: ProjectId,
        result: Result<CreateShellTab, LifecycleError>,
        observed_revision: Option<u64>,
        snapshot: Option<TabPresentationSnapshot>,
    },
    Renamed {
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        expected_revision: u64,
        result: Result<RenameOutcome, LifecycleError>,
        observed_revision: Option<u64>,
        snapshot: Option<TabPresentationSnapshot>,
    },
    Closed {
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        expected: ExpectedRuntimeIdentity,
        outcome: CloseOutcome,
        observed_revision: Option<u64>,
        snapshot: Option<TabPresentationSnapshot>,
    },
}

pub(crate) struct RouteController {
    commands: SyncSender<Command>,
    results: Receiver<RouteResult>,
    handoff: Arc<Mutex<Option<PreparedAttachment>>>,
    canceled: Arc<AtomicU64>,
    prepare_busy: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    save_commands: Option<SyncSender<MetadataCommand>>,
    save_results: Receiver<MetadataResult>,
    pending_save: Arc<Mutex<Option<WorkspaceRouteId>>>,
    refresh_pending: Arc<AtomicBool>,
    pending_refresh_route: Arc<Mutex<Option<Option<WorkspaceRouteId>>>>,
    save_busy: Arc<AtomicBool>,
    mutation_busy: Arc<AtomicBool>,
    next_metadata_id: AtomicU64,
}

struct MetadataWorkerSignals {
    pending_save: Arc<Mutex<Option<WorkspaceRouteId>>>,
    refresh_pending: Arc<AtomicBool>,
    pending_refresh_route: Arc<Mutex<Option<Option<WorkspaceRouteId>>>>,
    save_busy: Arc<AtomicBool>,
    mutation_busy: Arc<AtomicBool>,
}

impl RouteController {
    pub(crate) fn start(app_root: PathBuf) -> Self {
        Self::start_with_gate(app_root, |_| {})
    }

    fn start_with_gate(app_root: PathBuf, gate: impl Fn(RouteRequest) + Send + 'static) -> Self {
        Self::start_with_gate_and_backend(
            app_root.clone(),
            gate,
            Box::new(StoreMetadataBackend { root: app_root }),
        )
    }

    fn start_with_gate_and_backend(
        app_root: PathBuf,
        gate: impl Fn(RouteRequest) + Send + 'static,
        metadata_backend: Box<dyn MetadataBackend>,
    ) -> Self {
        let (commands, receiver) = mpsc::sync_channel(1);
        let (result_sender, results) = mpsc::sync_channel(1);
        let handoff = Arc::new(Mutex::new(None));
        let canceled = Arc::new(AtomicU64::new(0));
        let prepare_busy = Arc::new(AtomicBool::new(false));
        let thread_handoff = Arc::clone(&handoff);
        let thread_canceled = Arc::clone(&canceled);
        let thread_prepare_busy = Arc::clone(&prepare_busy);
        let worker_root = app_root.clone();
        let route_thread = thread::Builder::new()
            .name("panelink-route-prepare".into())
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Prepare(request) => {
                            gate(request);
                            let result = prepare(&worker_root, request, &thread_canceled);
                            match result {
                                Ok(prepared) => {
                                    let request_id = prepared.request_id;
                                    if is_canceled(&thread_canceled, request_id) {
                                        drop(prepared);
                                        thread_prepare_busy.store(false, Ordering::Release);
                                        continue;
                                    }
                                    match thread_handoff.try_lock() {
                                        Ok(mut slot) if slot.is_none() => *slot = Some(prepared),
                                        Ok(_) | Err(TryLockError::WouldBlock) => {
                                            drop(prepared);
                                            let _ = result_sender.try_send(RouteResult::Failed {
                                                request_id,
                                                label: "workspace switch busy",
                                            });
                                            thread_prepare_busy.store(false, Ordering::Release);
                                            continue;
                                        }
                                        Err(TryLockError::Poisoned(_)) => {
                                            drop(prepared);
                                            thread_prepare_busy.store(false, Ordering::Release);
                                            continue;
                                        }
                                    }
                                    let _ =
                                        result_sender.try_send(RouteResult::Ready { request_id });
                                    loop {
                                        if is_canceled(&thread_canceled, request_id) {
                                            let mut slot = match thread_handoff.lock() {
                                                Ok(slot) => slot,
                                                Err(poisoned) => poisoned.into_inner(),
                                            };
                                            if slot
                                                .as_ref()
                                                .is_some_and(|item| item.request_id == request_id)
                                            {
                                                drop(slot.take());
                                            }
                                            thread_prepare_busy.store(false, Ordering::Release);
                                            break;
                                        }
                                        let still_owned = match thread_handoff.lock() {
                                            Ok(slot) => slot
                                                .as_ref()
                                                .is_some_and(|item| item.request_id == request_id),
                                            Err(poisoned) => poisoned
                                                .into_inner()
                                                .as_ref()
                                                .is_some_and(|item| item.request_id == request_id),
                                        };
                                        if !still_owned {
                                            break;
                                        }
                                        thread::sleep(POLL);
                                    }
                                }
                                Err(label) => {
                                    if is_canceled(&thread_canceled, request.request_id) {
                                        thread_prepare_busy.store(false, Ordering::Release);
                                        continue;
                                    }
                                    let _ = result_sender.try_send(RouteResult::Failed {
                                        request_id: request.request_id,
                                        label,
                                    });
                                    thread_prepare_busy.store(false, Ordering::Release);
                                }
                            }
                        }
                    }
                }
            })
            .expect("route preparation thread creation failed");

        // Metadata work has one bounded wake slot, one admitted save slot, and
        // a coalesced refresh bit. A stalled fsync cannot hold the host or the
        // Ctrl-X path, and a refresh always has a queued wake or a running one.
        let (save_sender, save_receiver) = mpsc::sync_channel(1);
        let (save_result_sender, save_results) = mpsc::sync_channel(1);
        let pending_save = Arc::new(Mutex::new(None));
        let worker_pending_save = Arc::clone(&pending_save);
        let refresh_pending = Arc::new(AtomicBool::new(false));
        let worker_refresh_pending = Arc::clone(&refresh_pending);
        let pending_refresh_route = Arc::new(Mutex::new(None));
        let worker_pending_refresh_route = Arc::clone(&pending_refresh_route);
        let save_busy = Arc::new(AtomicBool::new(false));
        let worker_save_busy = Arc::clone(&save_busy);
        let mutation_busy = Arc::new(AtomicBool::new(false));
        let worker_mutation_busy = Arc::clone(&mutation_busy);
        let _ = thread::Builder::new()
            .name("panelink-route-metadata".into())
            .spawn(move || {
                metadata_loop(
                    metadata_backend,
                    save_receiver,
                    save_result_sender,
                    MetadataWorkerSignals {
                        pending_save: worker_pending_save,
                        refresh_pending: worker_refresh_pending,
                        pending_refresh_route: worker_pending_refresh_route,
                        save_busy: worker_save_busy,
                        mutation_busy: worker_mutation_busy,
                    },
                )
            });

        Self {
            commands,
            results,
            handoff,
            canceled,
            prepare_busy,
            thread: Some(route_thread),
            save_commands: Some(save_sender),
            save_results,
            pending_save,
            refresh_pending,
            pending_refresh_route,
            save_busy,
            mutation_busy,
            next_metadata_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn prepare(&self, request: RouteRequest) -> Result<(), &'static str> {
        if self
            .prepare_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("workspace switch busy");
        }
        match self.commands.try_send(Command::Prepare(request)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.prepare_busy.store(false, Ordering::Release);
                Err("workspace switch busy")
            }
            Err(TrySendError::Disconnected(_)) => {
                self.prepare_busy.store(false, Ordering::Release);
                Err("workspace switch unavailable")
            }
        }
    }

    pub(crate) fn try_result(&self) -> Option<RouteResult> {
        self.results.try_recv().ok()
    }

    pub(crate) fn cancel(&self, request_id: u64) {
        self.canceled.fetch_max(request_id, Ordering::AcqRel);
    }

    pub(crate) fn try_take_prepared(&self, request_id: u64) -> Option<PreparedAttachment> {
        let prepared = try_take_matching(&self.handoff, request_id, |prepared| prepared.request_id);
        if prepared.is_some() {
            self.prepare_busy.store(false, Ordering::Release);
        }
        prepared
    }

    pub(crate) fn queue_selection_save(&self, route: WorkspaceRouteId) -> Result<(), &'static str> {
        let Some(sender) = self.save_commands.as_ref() else {
            return Err("selection not saved");
        };
        if self
            .save_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("workspace metadata busy");
        }
        *lock_recover(&self.pending_save) = Some(route);
        match sender.try_send(MetadataCommand::Wake) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                *lock_recover(&self.pending_save) = None;
                self.save_busy.store(false, Ordering::Release);
                Err("workspace metadata busy")
            }
        }
    }

    /// Request one asynchronous metadata snapshot. Duplicate requests coalesce
    /// in the pending bit while a read is pending or running.
    pub(crate) fn queue_catalog_refresh(
        &self,
        source_route: Option<WorkspaceRouteId>,
    ) -> Result<(), &'static str> {
        let Some(sender) = self.save_commands.as_ref() else {
            return Err("workspace catalog unavailable");
        };
        *lock_recover(&self.pending_refresh_route) = Some(source_route);
        self.refresh_pending.store(true, Ordering::Release);
        match sender.try_send(MetadataCommand::Wake) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err("workspace catalog unavailable"),
        }
    }

    pub(crate) fn try_metadata_result(&self) -> Option<MetadataResult> {
        self.save_results.try_recv().ok()
    }

    pub(crate) fn queue_create(
        &self,
        project: ProjectId,
        intent_id: CreateIntentId,
        launch: ProjectLaunchContext,
    ) -> Result<MetadataRequestId, MetadataQueueError> {
        self.queue_mutation(|request_id| MetadataMutation::Create {
            request_id,
            project,
            intent_id,
            launch,
        })
    }

    pub(crate) fn queue_rename(
        &self,
        route: WorkspaceRouteId,
        name: String,
        expected_revision: u64,
    ) -> Result<MetadataRequestId, MetadataQueueError> {
        self.queue_mutation(|request_id| MetadataMutation::Rename {
            request_id,
            route,
            name,
            expected_revision,
        })
    }

    pub(crate) fn queue_close(
        &self,
        route: WorkspaceRouteId,
        expected: ExpectedRuntimeIdentity,
    ) -> Result<MetadataRequestId, MetadataQueueError> {
        self.queue_mutation(|request_id| MetadataMutation::Close {
            request_id,
            route,
            expected,
        })
    }

    fn queue_mutation(
        &self,
        build: impl FnOnce(MetadataRequestId) -> MetadataMutation,
    ) -> Result<MetadataRequestId, MetadataQueueError> {
        let sender = self
            .save_commands
            .as_ref()
            .ok_or(MetadataQueueError::Unavailable)?;
        if self
            .mutation_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(MetadataQueueError::Busy);
        }
        let raw_id = self
            .next_metadata_id
            .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1));
        let request_id = match raw_id {
            Ok(id) => MetadataRequestId(id),
            Err(_) => {
                self.mutation_busy.store(false, Ordering::Release);
                return Err(MetadataQueueError::IdentifierExhausted);
            }
        };
        match sender.try_send(MetadataCommand::Mutation(build(request_id))) {
            Ok(()) => Ok(request_id),
            Err(TrySendError::Full(_)) => {
                self.mutation_busy.store(false, Ordering::Release);
                Err(MetadataQueueError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.mutation_busy.store(false, Ordering::Release);
                Err(MetadataQueueError::Unavailable)
            }
        }
    }
}

fn try_take_matching<T>(
    handoff: &Mutex<Option<T>>,
    request_id: u64,
    id_of: impl FnOnce(&T) -> u64,
) -> Option<T> {
    let mut slot = match handoff.try_lock() {
        Ok(slot) => slot,
        Err(TryLockError::WouldBlock) => return None,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
    };
    if slot
        .as_ref()
        .is_some_and(|value| id_of(value) == request_id)
    {
        slot.take()
    } else {
        None
    }
}

impl Drop for RouteController {
    fn drop(&mut self) {
        self.canceled.store(u64::MAX, Ordering::Release);
        self.save_commands.take();
        // Closing the request channel releases an idle worker. Do not join:
        // server connect and metadata fsync are not host cleanup operations.
        drop(self.thread.take());
    }
}

fn metadata_loop(
    mut backend: Box<dyn MetadataBackend>,
    receiver: Receiver<MetadataCommand>,
    results: SyncSender<MetadataResult>,
    signals: MetadataWorkerSignals,
) {
    let mut lifecycle_session = LifecycleSession::default();
    while let Ok(command) = receiver.recv() {
        if let MetadataCommand::Mutation(mutation) = command {
            let result = execute_metadata_mutation(&mut *backend, &mut lifecycle_session, mutation);
            let _ = results.send(result);
            signals.mutation_busy.store(false, Ordering::Release);
        }
        process_metadata_wake(&mut *backend, &results, &signals);
    }
}

fn process_metadata_wake(
    backend: &mut dyn MetadataBackend,
    results: &SyncSender<MetadataResult>,
    signals: &MetadataWorkerSignals,
) {
    loop {
        let route = lock_recover(&signals.pending_save).take();
        if let Some(route) = route {
            if backend.save_selection(route).is_err() {
                let _ = results.send(MetadataResult::SaveError("selection not saved"));
            }
            signals.save_busy.store(false, Ordering::Release);
        }
        if signals.refresh_pending.swap(false, Ordering::AcqRel) {
            let source_route = lock_recover(&signals.pending_refresh_route)
                .take()
                .unwrap_or(None);
            refresh_catalog(backend, results, source_route);
            continue;
        }
        if lock_recover(&signals.pending_save).is_none() {
            break;
        }
    }
}

fn execute_metadata_mutation(
    backend: &mut dyn MetadataBackend,
    lifecycle_session: &mut LifecycleSession,
    mutation: MetadataMutation,
) -> MetadataResult {
    match mutation {
        MetadataMutation::Create {
            request_id,
            project,
            intent_id,
            launch,
        } => {
            let result = backend.create_shell_tab(project, intent_id, lifecycle_session, &launch);
            let (snapshot, observed_revision) = observe_presentation(backend);
            MetadataResult::Created {
                request_id,
                intent_id,
                project,
                result,
                observed_revision,
                snapshot,
            }
        }
        MetadataMutation::Rename {
            request_id,
            route,
            name,
            expected_revision,
        } => {
            let result = backend.rename_tab(route, &name, expected_revision);
            let (snapshot, observed_revision) = observe_presentation(backend);
            MetadataResult::Renamed {
                request_id,
                route,
                expected_revision,
                result,
                observed_revision,
                snapshot,
            }
        }
        MetadataMutation::Close {
            request_id,
            route,
            expected,
        } => {
            let outcome = backend.close_tab(route, &expected);
            let (snapshot, observed_revision) = observe_presentation(backend);
            MetadataResult::Closed {
                request_id,
                route,
                expected,
                outcome,
                observed_revision,
                snapshot,
            }
        }
    }
}

fn observe_presentation(
    backend: &mut dyn MetadataBackend,
) -> (Option<TabPresentationSnapshot>, Option<u64>) {
    match backend.presentation_snapshot() {
        Ok((snapshot, revision)) => (Some(snapshot), Some(revision)),
        Err(_) => (None, None),
    }
}

fn refresh_catalog(
    backend: &mut dyn MetadataBackend,
    results: &SyncSender<MetadataResult>,
    source_route: Option<WorkspaceRouteId>,
) {
    match backend.load_catalog() {
        Ok(Some((snapshot, observed_revision))) => {
            let _ = results.send(MetadataResult::Catalog {
                source_route,
                snapshot,
                observed_revision,
            });
        }
        Ok(None) | Err(_) => {
            let _ = results.send(MetadataResult::CatalogError);
        }
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn prepare(
    app_root: &std::path::Path,
    request: RouteRequest,
    canceled: &AtomicU64,
) -> Result<PreparedAttachment, &'static str> {
    let (state, catalog) = crate::default_open::resolve_workspace_route_cancellable(
        app_root,
        request.destination,
        || is_canceled(canceled, request.request_id),
    )?;
    if is_canceled(canceled, request.request_id) {
        return Err("workspace switch canceled");
    }
    crate::default_open::ensure_workspace_route_cancellable(
        app_root,
        &state,
        request.destination,
        &catalog,
        || is_canceled(canceled, request.request_id),
    )?;
    if is_canceled(canceled, request.request_id) {
        return Err("workspace switch canceled");
    }
    let checked_route = request.destination;
    let mut app = App::new(request.host_size.0, request.host_size.1)
        .map_err(|_| "terminal frame unavailable")?;
    app.install_workspace_index(catalog.clone(), checked_route);
    let net = Net::connect(state.server, state.contexts, &mut app, Instant::now())?;
    if is_canceled(canceled, request.request_id) {
        drop(net);
        return Err("workspace switch canceled");
    }
    let (_, checked_route) = crate::default_open::workspace_catalog_cancellable(
        app_root,
        request.destination.project.get(),
        request.destination.tab.get(),
        || is_canceled(canceled, request.request_id),
    )?;
    if is_canceled(canceled, request.request_id) {
        drop(net);
        return Err("workspace switch canceled");
    }
    let (snapshot, revision) = crate::workspace_lifecycle::presentation_snapshot(app_root)
        .map_err(|_| "workspace metadata unavailable")?;
    if is_canceled(canceled, request.request_id) {
        drop(net);
        return Err("workspace switch canceled");
    }
    if !app.install_workspace_tabs(snapshot, Some(checked_route)) {
        drop(net);
        return Err("workspace route unavailable");
    }
    app.set_workspace_revision(revision);
    Ok(PreparedAttachment {
        request_id: request.request_id,
        route: checked_route,
        app,
        net,
    })
}

fn is_canceled(canceled: &AtomicU64, request_id: u64) -> bool {
    let value = canceled.load(Ordering::Acquire);
    value == request_id || value == u64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum BackendCall {
        Save(WorkspaceRouteId),
        LoadCatalog,
    }

    struct GatedBackend {
        calls: SyncSender<BackendCall>,
        release: Receiver<()>,
        completed: Option<SyncSender<()>>,
        count: Arc<AtomicUsize>,
        fail_save: bool,
    }

    impl MetadataBackend for GatedBackend {
        fn save_selection(&mut self, route: WorkspaceRouteId) -> Result<(), ()> {
            self.count.fetch_add(1, Ordering::AcqRel);
            let _ = self.calls.send(BackendCall::Save(route));
            let _ = self.release.recv();
            if let Some(completed) = self.completed.take() {
                let _ = completed.send(());
            }
            if self.fail_save {
                Err(())
            } else {
                Ok(())
            }
        }

        fn load_catalog(&mut self) -> Result<Option<(TabPresentationSnapshot, u64)>, ()> {
            self.count.fetch_add(1, Ordering::AcqRel);
            let _ = self.calls.send(BackendCall::LoadCatalog);
            let _ = self.release.recv();
            Ok(None)
        }
    }

    fn route(id: u64) -> WorkspaceRouteId {
        WorkspaceRouteId {
            project: panelink::workspace_index::ProjectId::new(id).unwrap(),
            tab: panelink::workspace_index::WorkspaceTabId::new(id).unwrap(),
        }
    }

    fn controller_with_backend(backend: impl MetadataBackend) -> RouteController {
        RouteController::start_with_gate_and_backend(
            PathBuf::from("/definitely-not-a-panelink-state-root"),
            |_| {},
            Box::new(backend),
        )
    }

    struct GatedCreateBackend {
        call: SyncSender<(ProjectId, CreateIntentId)>,
        release: Receiver<()>,
    }

    impl MetadataBackend for GatedCreateBackend {
        fn save_selection(&mut self, _route: WorkspaceRouteId) -> Result<(), ()> {
            Ok(())
        }

        fn load_catalog(&mut self) -> Result<Option<(TabPresentationSnapshot, u64)>, ()> {
            Ok(None)
        }

        fn create_shell_tab(
            &mut self,
            project: ProjectId,
            intent: CreateIntentId,
            _session: &mut LifecycleSession,
            _launch: &ProjectLaunchContext,
        ) -> Result<CreateShellTab, LifecycleError> {
            self.call.send((project, intent)).unwrap();
            self.release.recv().unwrap();
            Ok(CreateShellTab {
                route: WorkspaceRouteId {
                    project,
                    tab: panelink::workspace_index::WorkspaceTabId::new(77).unwrap(),
                },
                name: "Shell 1".to_owned(),
                status: crate::workspace_lifecycle::CreateStatus::Ready,
            })
        }
    }

    fn project_launch_context() -> ProjectLaunchContext {
        let cwd = PathBuf::from("/private/tmp/panelink-route-metadata-test");
        ProjectLaunchContext {
            cwd: cwd.clone(),
            context: panelink::launch::LaunchContext {
                working_directory: cwd,
                environment: [("PATH".into(), "/usr/bin:/bin".into())]
                    .into_iter()
                    .collect::<panelink::launch::Environment>(),
            },
        }
    }

    fn gated_backend(
        release: Receiver<()>,
    ) -> (GatedBackend, Receiver<BackendCall>, Arc<AtomicUsize>) {
        let (call_sender, call_receiver) = mpsc::sync_channel(4);
        let count = Arc::new(AtomicUsize::new(0));
        (
            GatedBackend {
                calls: call_sender,
                release,
                completed: None,
                count: Arc::clone(&count),
                fail_save: false,
            },
            call_receiver,
            count,
        )
    }

    #[test]
    fn bounded_request_channel_rejects_growth() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let request = RouteRequest {
            request_id: 1,
            destination: WorkspaceRouteId {
                project: panelink::workspace_index::ProjectId::new(1).unwrap(),
                tab: panelink::workspace_index::WorkspaceTabId::new(1).unwrap(),
            },
            host_size: (24, 80),
        };
        assert!(sender.try_send(request).is_ok());
        assert!(matches!(
            sender.try_send(request),
            Err(TrySendError::Full(_))
        ));
        assert!(matches!(receiver.try_recv(), Ok(found) if found == request));
    }

    #[test]
    fn stale_handoff_is_preserved_and_matching_handoff_is_taken_once() {
        let handoff = Mutex::new(Some((7_u64, "prepared")));
        assert_eq!(try_take_matching(&handoff, 8, |value| value.0), None);
        assert_eq!(
            try_take_matching(&handoff, 7, |value| value.0),
            Some((7, "prepared"))
        );
        assert_eq!(try_take_matching(&handoff, 7, |value| value.0), None);
    }

    #[test]
    fn handoff_lock_contention_is_nonblocking() {
        let handoff = Mutex::new(Some((3_u64, "prepared")));
        let held = handoff.lock().unwrap();
        assert_eq!(try_take_matching(&handoff, 3, |value| value.0), None);
        drop(held);
        assert_eq!(
            try_take_matching(&handoff, 3, |value| value.0),
            Some((3, "prepared"))
        );
    }

    #[test]
    fn cancellation_is_request_scoped_with_controller_shutdown_sentinel() {
        let canceled = AtomicU64::new(0);
        assert!(!is_canceled(&canceled, 9));
        canceled.store(9, Ordering::Release);
        assert!(is_canceled(&canceled, 9));
        assert!(!is_canceled(&canceled, 10));
        canceled.store(u64::MAX, Ordering::Release);
        assert!(is_canceled(&canceled, 10));
    }

    #[test]
    fn controller_keeps_canceled_request_admission_until_worker_reclaims_it() {
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let controller = RouteController::start_with_gate(
            PathBuf::from("/definitely-not-a-panelink-state-root"),
            move |request| {
                if request.request_id == 41 {
                    entered_sender.send(()).unwrap();
                    release_receiver.recv().unwrap();
                }
            },
        );
        let first = RouteRequest {
            request_id: 41,
            destination: WorkspaceRouteId {
                project: panelink::workspace_index::ProjectId::new(1).unwrap(),
                tab: panelink::workspace_index::WorkspaceTabId::new(1).unwrap(),
            },
            host_size: (24, 80),
        };
        assert_eq!(controller.prepare(first), Ok(()));
        entered_receiver.recv().unwrap();
        controller.cancel(first.request_id);
        let rejected = RouteRequest {
            request_id: 42,
            ..first
        };
        assert_eq!(controller.prepare(rejected), Err("workspace switch busy"));
        assert_eq!(controller.canceled.load(Ordering::Acquire), 41);

        release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.prepare_busy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(!controller.prepare_busy.load(Ordering::Acquire));
        assert_eq!(controller.prepare(rejected), Ok(()));
        assert_eq!(controller.canceled.load(Ordering::Acquire), 41);
    }

    #[test]
    fn stalled_save_admits_no_second_selection_mutation() {
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (backend, calls, count) = gated_backend(release_receiver);
        let controller = controller_with_backend(backend);

        assert_eq!(controller.queue_selection_save(route(1)), Ok(()));
        assert_eq!(calls.recv().unwrap(), BackendCall::Save(route(1)));
        assert_eq!(
            controller.queue_selection_save(route(2)),
            Err("workspace metadata busy")
        );
        assert_eq!(count.load(Ordering::Acquire), 1);

        release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.save_busy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(!controller.save_busy.load(Ordering::Acquire));
        assert_eq!(count.load(Ordering::Acquire), 1);
        drop(controller);
    }

    #[test]
    fn metadata_worker_admits_one_create_while_lifecycle_adapter_is_running() {
        let (call_sender, call_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let controller = controller_with_backend(GatedCreateBackend {
            call: call_sender,
            release: release_receiver,
        });
        let project = panelink::workspace_index::ProjectId::new(3).unwrap();
        let first_intent = CreateIntentId(1);
        let request_id = controller
            .queue_create(project, first_intent, project_launch_context())
            .unwrap();
        assert_eq!(call_receiver.recv().unwrap(), (project, first_intent));

        assert_eq!(
            controller.queue_create(project, CreateIntentId(2), project_launch_context()),
            Err(MetadataQueueError::Busy)
        );
        release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let result = loop {
            if let Some(result) = controller.try_metadata_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "create result timed out");
            thread::yield_now();
        };
        assert!(matches!(
            result,
            MetadataResult::Created {
                request_id: found,
                intent_id: found_intent,
                project: found_project,
                result: Ok(CreateShellTab { status: crate::workspace_lifecycle::CreateStatus::Ready, .. }),
                ..
            } if found == request_id && found_intent == first_intent && found_project == project
        ));
        assert!(call_receiver.try_recv().is_err());
    }

    #[test]
    fn refreshes_coalesce_and_wake_while_metadata_worker_is_busy() {
        let (release_sender, release_receiver) = mpsc::sync_channel(2);
        let (backend, calls, count) = gated_backend(release_receiver);
        let controller = controller_with_backend(backend);

        assert_eq!(controller.queue_catalog_refresh(Some(route(1))), Ok(()));
        assert_eq!(calls.recv().unwrap(), BackendCall::LoadCatalog);
        for _ in 0..5 {
            assert_eq!(controller.queue_catalog_refresh(Some(route(1))), Ok(()));
        }
        release_sender.send(()).unwrap();

        // The worker is still inside the first backend call when the refresh
        // bit is set. The capacity-one wake remains queued until it resumes.
        assert_eq!(
            calls.recv_timeout(Duration::from_secs(1)).unwrap(),
            BackendCall::LoadCatalog
        );
        release_sender.send(()).unwrap();
        assert!(calls.recv_timeout(Duration::from_millis(40)).is_err());
        assert_eq!(count.load(Ordering::Acquire), 2);
        drop(controller);
    }

    #[test]
    fn controller_drop_does_not_wait_for_stalled_metadata_backend() {
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (finished_sender, finished_receiver) = mpsc::sync_channel(1);
        let (mut backend, calls, _) = gated_backend(release_receiver);
        backend.completed = Some(finished_sender);
        let controller = controller_with_backend(backend);

        assert_eq!(controller.queue_selection_save(route(3)), Ok(()));
        assert_eq!(calls.recv().unwrap(), BackendCall::Save(route(3)));
        let drop_started = Instant::now();
        drop(controller);
        assert!(drop_started.elapsed() < Duration::from_millis(100));

        release_sender.send(()).unwrap();
        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn save_failure_exposes_only_fixed_status_text() {
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (mut backend, _calls, _) = gated_backend(release_receiver);
        backend.fail_save = true;
        let controller = controller_with_backend(backend);

        assert_eq!(controller.queue_selection_save(route(4)), Ok(()));
        release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let result = loop {
            if let Some(result) = controller.try_metadata_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "metadata result timed out");
            thread::yield_now();
        };
        assert!(matches!(
            result,
            MetadataResult::SaveError("selection not saved")
        ));
        drop(controller);
    }

    #[test]
    fn canceled_preinstall_route_never_persists_selection() {
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let (route_release_sender, route_release_receiver) = mpsc::sync_channel(1);
        let (metadata_release_sender, metadata_release_receiver) = mpsc::sync_channel(1);
        let (backend, _calls, count) = gated_backend(metadata_release_receiver);
        let controller = RouteController::start_with_gate_and_backend(
            PathBuf::from("/definitely-not-a-panelink-state-root"),
            move |_| {
                entered_sender.send(()).unwrap();
                route_release_receiver.recv().unwrap();
            },
            Box::new(backend),
        );
        let request = RouteRequest {
            request_id: 51,
            destination: route(1),
            host_size: (24, 80),
        };
        assert_eq!(controller.prepare(request), Ok(()));
        entered_receiver.recv().unwrap();
        controller.cancel(request.request_id);
        route_release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.prepare_busy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(!controller.prepare_busy.load(Ordering::Acquire));
        assert_eq!(count.load(Ordering::Acquire), 0);
        assert_eq!(controller.try_result(), None);

        // The save API admits only a route ID; terminal input or key payloads
        // have no path into this metadata backend.
        drop(controller);
        drop(metadata_release_sender);
    }
}
