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

use panelink::workspace_index::WorkspaceIndex;
use panelink::workspace_tabs::WorkspaceRouteId;

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
}

trait MetadataBackend: Send + 'static {
    fn save_selection(&mut self, route: WorkspaceRouteId) -> Result<(), ()>;
    fn load_catalog(&mut self) -> Result<Option<WorkspaceIndex>, ()>;
}

struct StoreMetadataBackend {
    root: PathBuf,
}

impl MetadataBackend for StoreMetadataBackend {
    fn save_selection(&mut self, route: WorkspaceRouteId) -> Result<(), ()> {
        crate::default_open::save_workspace_selection(&self.root, route).map_err(|_| ())
    }

    fn load_catalog(&mut self) -> Result<Option<WorkspaceIndex>, ()> {
        crate::default_open::load_workspace_index_snapshot(&self.root).map_err(|_| ())
    }
}

pub(crate) enum MetadataResult {
    Catalog(WorkspaceIndex),
    CatalogError,
    SaveError(&'static str),
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
    save_busy: Arc<AtomicBool>,
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
        let save_busy = Arc::new(AtomicBool::new(false));
        let worker_save_busy = Arc::clone(&save_busy);
        let _ = thread::Builder::new()
            .name("panelink-route-metadata".into())
            .spawn(move || {
                metadata_loop(
                    metadata_backend,
                    save_receiver,
                    save_result_sender,
                    worker_pending_save,
                    worker_refresh_pending,
                    worker_save_busy,
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
            save_busy,
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
    pub(crate) fn queue_catalog_refresh(&self) -> Result<(), &'static str> {
        let Some(sender) = self.save_commands.as_ref() else {
            return Err("workspace catalog unavailable");
        };
        self.refresh_pending.store(true, Ordering::Release);
        match sender.try_send(MetadataCommand::Wake) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err("workspace catalog unavailable"),
        }
    }

    pub(crate) fn try_metadata_result(&self) -> Option<MetadataResult> {
        self.save_results.try_recv().ok()
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
    pending_save: Arc<Mutex<Option<WorkspaceRouteId>>>,
    refresh_pending: Arc<AtomicBool>,
    save_busy: Arc<AtomicBool>,
) {
    while receiver.recv().is_ok() {
        loop {
            let route = lock_recover(&pending_save).take();
            if let Some(route) = route {
                if backend.save_selection(route).is_err() {
                    let _ = results.send(MetadataResult::SaveError("selection not saved"));
                }
                save_busy.store(false, Ordering::Release);
            }
            if refresh_pending.swap(false, Ordering::AcqRel) {
                refresh_catalog(&mut *backend, &results);
                continue;
            }
            if lock_recover(&pending_save).is_none() {
                break;
            }
        }
    }
}

fn refresh_catalog(backend: &mut dyn MetadataBackend, results: &SyncSender<MetadataResult>) {
    match backend.load_catalog() {
        Ok(Some(index)) => {
            let _ = results.send(MetadataResult::Catalog(index));
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
    app.install_workspace_tabs(catalog.clone(), checked_route);
    let net = Net::connect(state.server, state.contexts, &mut app, Instant::now())?;
    if is_canceled(canceled, request.request_id) {
        drop(net);
        return Err("workspace switch canceled");
    }
    let (catalog, checked_route) = crate::default_open::workspace_catalog_cancellable(
        app_root,
        request.destination.project.get(),
        request.destination.tab.get(),
        || is_canceled(canceled, request.request_id),
    )?;
    app.install_workspace_tabs(catalog.clone(), checked_route);
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

        fn load_catalog(&mut self) -> Result<Option<WorkspaceIndex>, ()> {
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
    fn refreshes_coalesce_and_wake_while_metadata_worker_is_busy() {
        let (release_sender, release_receiver) = mpsc::sync_channel(2);
        let (backend, calls, count) = gated_backend(release_receiver);
        let controller = controller_with_backend(backend);

        assert_eq!(controller.queue_catalog_refresh(), Ok(()));
        assert_eq!(calls.recv().unwrap(), BackendCall::LoadCatalog);
        for _ in 0..5 {
            assert_eq!(controller.queue_catalog_refresh(), Ok(()));
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
