//! Host terminal ownership for the attach client, mirroring `p2_probe`'s
//! `HostGuard`: a separately opened nonblocking `/dev/tty`, raw mode, the
//! alternate screen, hidden cursor, bracketed paste and SGR mouse reporting.
//! Restoration runs first on every exit path (return, error, panic unwind via
//! `Drop`, and SIGINT/SIGTERM/SIGHUP via a stop flag).

use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm_input::Decoder;
use panelink::layout_store::{LayoutSlot, LayoutStore};
use panelink::split_menu::{Availability, DisabledReason};
use panelink::terminal::compose::Compositor;
use rustix::fs::{open, Mode, OFlags};
use rustix::termios::{
    isatty, tcflush, tcgetattr, tcgetwinsize, tcsetattr, OptionalActions, QueueSelector, Termios,
};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

use super::clipboard::{ClipboardStatus, ClipboardWriter};
use super::net::{Net, NetView};
use super::routes::{
    MetadataQueueError, MetadataResult, PreparedAttachment, RouteController, RouteRequest,
    RouteResult,
};
use super::state::{App, Flow, WorkspaceMetadataMutation};
use super::worker::{ClientResult, WorkerHandle};
use crate::workspace_lifecycle::{CreateIntentId, ProjectLaunchContext};
use panelink::recovery_view::RecoverySlot;
use panelink::workspace_index::{TabPresentationSnapshot, WorkspaceIndex};
use panelink::workspace_tabs::WorkspaceRouteId;

/// Enter: alternate screen, hide cursor, bracketed paste, button-motion mouse
/// reporting (1002) with SGR coordinates (1006).
pub(crate) const ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[?1002h\x1b[?1006h";
/// Restore in reverse: mouse off first, then p2_probe's visual restore.
pub(crate) const RESTORE_SEQUENCE: &[u8] =
    b"\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[?7h\x1b[?2004l\x1b[?25h\x1b[?1049l";
const INPUT_CHUNK_BYTES: usize = 1024;
const INPUT_WORK_BUDGET: Duration = Duration::from_millis(5);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(50);
const INCOMPLETE_INPUT_TIMEOUT: Duration = Duration::from_secs(2);
const RESIZE_INTERVAL: Duration = Duration::from_millis(100);
const RENDER_INTERVAL: Duration = Duration::from_millis(16);
const KEY_FLUSH_DELAY: Duration = Duration::from_millis(4);
const OUTPUT_STALL_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_millis(100);
/// Exit code reported when the server ended (every shell exited or it was
/// stopped); the client then prints a short notice instead of an error.
pub(crate) const SERVER_ENDED_EXIT: i32 = 3;
const FRAME_BYTES: usize = 4 * 1024 * 1024 + 64 * 1024;
const MAX_OUTPUT_WRITE: usize = 64 * 1024;

enum WorkspaceEntry {
    Attached {
        app_root: PathBuf,
        index: WorkspaceIndex,
        current: WorkspaceRouteId,
    },
    Empty {
        app_root: PathBuf,
        snapshot: TabPresentationSnapshot,
    },
}

#[allow(clippy::large_enum_variant)] // Keep the exact App/Net ownership handoff without heap allocation.
pub(crate) enum PreparedWorkspace {
    Attached(PreparedAttachment),
    Empty(App),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostError {
    NotTty,
    Open,
    Query,
    Signals,
    RawMode,
    Setup,
    InputRead,
    InputClosed,
    InputDecode,
    InputTimeout,
    Frame,
    OutputWrite,
    OutputStalled,
    Restore,
}

impl HostError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotTty => "attach needs an interactive terminal",
            Self::Open => "could not open the controlling terminal",
            Self::Query => "could not query the controlling terminal",
            Self::Signals => "could not register stop signals",
            Self::RawMode => "could not enter raw mode",
            Self::Setup => "could not initialize terminal modes",
            Self::InputRead => "could not read terminal input",
            Self::InputClosed => "terminal input closed",
            Self::InputDecode => "terminal input was malformed or oversized",
            Self::InputTimeout => "terminal input sequence timed out",
            Self::Frame => "could not compose the frame",
            Self::OutputWrite => "could not write the frame",
            Self::OutputStalled => "terminal output made no progress",
            Self::Restore => "could not restore the terminal",
        }
    }
}

pub(crate) struct HostGuard {
    pub(crate) tty: File,
    original: Termios,
    restore_needed: bool,
    restored: bool,
}

impl HostGuard {
    pub(crate) fn open() -> Result<Self, HostError> {
        if !isatty(io::stdin()) || !isatty(io::stdout()) {
            return Err(HostError::NotTty);
        }
        let fd = open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| HostError::Open)?;
        let tty = File::from(fd);
        let original = tcgetattr(&tty).map_err(|_| HostError::Query)?;
        Ok(Self {
            tty,
            original,
            restore_needed: false,
            restored: false,
        })
    }

    pub(crate) fn size(&self) -> Result<(u16, u16), HostError> {
        let size = tcgetwinsize(&self.tty).map_err(|_| HostError::Query)?;
        Ok((size.ws_row, size.ws_col))
    }

    pub(crate) fn enter(&mut self) -> Result<(), HostError> {
        let mut raw = self.original.clone();
        raw.make_raw();
        self.restore_needed = true;
        tcsetattr(&self.tty, OptionalActions::Now, &raw).map_err(|_| HostError::RawMode)?;
        write_control(&mut self.tty, ENTER_SEQUENCE).map_err(|_| HostError::Setup)
    }

    pub(crate) fn restore(&mut self) -> Result<(), HostError> {
        if self.restored || !self.restore_needed {
            self.restored = true;
            return Ok(());
        }
        self.restored = true;
        let mut failed = write_control(&mut self.tty, RESTORE_SEQUENCE).is_err();
        failed |= tcflush(&self.tty, QueueSelector::IFlush).is_err();
        failed |= tcsetattr(&self.tty, OptionalActions::Now, &self.original).is_err();
        failed |= tcflush(&self.tty, QueueSelector::IFlush).is_err();
        if failed {
            Err(HostError::Restore)
        } else {
            Ok(())
        }
    }

    fn discard_pending_input(&self) -> Result<(), HostError> {
        tcflush(&self.tty, QueueSelector::IFlush).map_err(|_| HostError::InputRead)
    }

    fn restore_and_drop(&mut self) -> Result<(), HostError> {
        self.restore()?;
        self.discard_pending_input()?;
        let replacement = open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| HostError::Open)?;
        let inert = Self {
            tty: File::from(replacement),
            original: self.original.clone(),
            restore_needed: false,
            restored: true,
        };
        drop(std::mem::replace(self, inert));
        Ok(())
    }
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub(crate) fn write_control(tty: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + CONTROL_WRITE_TIMEOUT;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match tty.write(&bytes[cursor..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => cursor += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Preallocated output buffer; the compositor commits only after the whole
/// frame was accepted by the host.
pub(crate) struct Output {
    bytes: Box<[u8]>,
    cursor: usize,
    len: usize,
    last_progress: Instant,
}

impl Output {
    pub(crate) fn new() -> Result<Self, HostError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(FRAME_BYTES)
            .map_err(|_| HostError::Frame)?;
        bytes.resize(FRAME_BYTES, 0);
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            cursor: 0,
            len: 0,
            last_progress: Instant::now(),
        })
    }

    pub(crate) fn pending(&self) -> bool {
        self.cursor < self.len
    }

    fn discard(&mut self) {
        self.cursor = 0;
        self.len = 0;
    }

    /// Write once; returns `true` when the frame completed on this call.
    pub(crate) fn write_once(&mut self, tty: &mut File) -> Result<bool, HostError> {
        if !self.pending() {
            return Ok(false);
        }
        let end = self.len.min(self.cursor + MAX_OUTPUT_WRITE);
        match tty.write(&self.bytes[self.cursor..end]) {
            Ok(0) => Err(HostError::OutputWrite),
            Ok(count) => {
                self.cursor += count;
                self.last_progress = Instant::now();
                Ok(!self.pending())
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                if self.last_progress.elapsed() >= OUTPUT_STALL_TIMEOUT {
                    Err(HostError::OutputStalled)
                } else {
                    Ok(false)
                }
            }
            Err(_) => Err(HostError::OutputWrite),
        }
    }
}

/// Entry point: returns the process exit code. Diagnostics are fixed text.
///
/// The default panic hook writes to inherited stderr *before* unwinding
/// (and so before `HostGuard::drop` restores the terminal); a full stderr
/// pipe would block there. Attach replaces it with a silent hook, lets the
/// unwind restore the terminal, then reports a fixed message on `/dev/tty`.
pub(crate) fn run(directory: PathBuf, context_dir: PathBuf) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inner(directory, context_dir, None)
    })) {
        Ok(code) => code,
        Err(_) => fail("internal error (terminal restored)"),
    }
}

/// Run the workspace-aware client. The initial catalog and route are already
/// validated by `default_open`; raw `run` remains the legacy attachment API.
pub(crate) fn run_workspace(
    directory: PathBuf,
    context_dir: PathBuf,
    app_root: PathBuf,
    index: WorkspaceIndex,
    current: WorkspaceRouteId,
) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inner(
            directory,
            context_dir,
            Some(WorkspaceEntry::Attached {
                app_root,
                index,
                current,
            }),
        )
    })) {
        Ok(code) => code,
        Err(_) => fail("internal error (terminal restored)"),
    }
}

/// Run the client-only zero-visible-tab state. No Net connection is opened;
/// only a later explicit New tab action can submit a lifecycle create intent.
#[allow(dead_code)] // The no-argument dispatcher remains owned by UX-05b.
pub(crate) fn run_workspace_empty(
    directory: PathBuf,
    context_dir: PathBuf,
    app_root: PathBuf,
    snapshot: TabPresentationSnapshot,
) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inner(
            directory,
            context_dir,
            Some(WorkspaceEntry::Empty { app_root, snapshot }),
        )
    })) {
        Ok(code) => code,
        Err(_) => fail("internal error (terminal restored)"),
    }
}

fn run_inner(directory: PathBuf, context_dir: PathBuf, workspace: Option<WorkspaceEntry>) -> i32 {
    // Declared before HostGuard so unwind drop order restores the terminal
    // before either worker handle can perform blocking cleanup.
    let mut clipboard = ClipboardWriter::new().ok();
    let mut net: Option<WorkerHandle>;
    let mut host = match HostGuard::open() {
        Ok(host) => host,
        Err(error) => return fail(error.message()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        if signal_hook::flag::register(signal, Arc::clone(&stop)).is_err() {
            return fail(HostError::Signals.message());
        }
    }
    let (rows, columns) = match host.size() {
        Ok(size) => size,
        Err(error) => return fail(error.message()),
    };
    // Every bounded buffer exists before the host enters raw mode.
    let mut app = match workspace.as_ref() {
        Some(WorkspaceEntry::Attached { index, current, .. }) => {
            let mut app = match App::new(rows, columns) {
                Ok(app) => app,
                Err(_) => return fail(HostError::Frame.message()),
            };
            app.install_workspace_index(index.clone(), *current);
            app
        }
        Some(WorkspaceEntry::Empty { snapshot, .. }) => {
            match App::new_workspace_empty(rows, columns, snapshot.clone()) {
                Ok(app) => app,
                Err(_) => return fail(HostError::Frame.message()),
            }
        }
        None => match App::new(rows, columns) {
            Ok(app) => app,
            Err(_) => return fail(HostError::Frame.message()),
        },
    };
    let mut compositor = match Compositor::try_new() {
        Ok(compositor) => compositor,
        Err(_) => return fail(HostError::Frame.message()),
    };
    let reduced_control_color = std::env::var_os("NO_COLOR").is_some()
        || std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    compositor.set_reduced_control_color(reduced_control_color);
    let mut output = match Output::new() {
        Ok(output) => output,
        Err(error) => return fail(error.message()),
    };
    let mut decoder = Decoder::new();
    if clipboard.is_none() {
        app.report_clipboard_unavailable(Instant::now());
    }
    let empty_workspace = matches!(workspace.as_ref(), Some(WorkspaceEntry::Empty { .. }));
    net = if empty_workspace {
        None
    } else {
        let connection = match Net::connect(directory, context_dir, &mut app, Instant::now()) {
            Ok(net) => net,
            Err(label) => return fail(label),
        };
        Some(WorkerHandle::start(connection))
    };
    let routes = workspace.as_ref().map(|entry| {
        let root = match entry {
            WorkspaceEntry::Attached { app_root, .. } | WorkspaceEntry::Empty { app_root, .. } => {
                app_root
            }
        };
        RouteController::start(root.clone())
    });
    let launch_context = if workspace.is_some() {
        super::net::capture_launch_context()
            .ok()
            .map(|context| ProjectLaunchContext {
                cwd: context.working_directory.clone(),
                context,
            })
    } else {
        None
    };
    let mut route_state = RouteLoopState {
        controller: routes.as_ref(),
        pending: None,
        ready: None,
        next_request_id: 1,
        host_size: (rows, columns),
        launch_context,
        next_create_intent: 1,
        remembered_focus: Vec::with_capacity(
            panelink::workspace_index::MAX_PROJECTS
                * panelink::workspace_index::MAX_TABS_PER_PROJECT,
        ),
    };
    if let Some(controller) = route_state.controller {
        if controller
            .queue_catalog_refresh(app.current_route())
            .is_err()
        {
            app.set_status("workspace catalog unavailable", Instant::now());
        }
    }
    let result = host.enter().and_then(|()| {
        event_loop(
            &mut host,
            &mut app,
            &mut net,
            (&mut compositor, &mut output, &mut decoder),
            clipboard.as_ref(),
            &stop,
            &mut route_state,
        )
    });
    app.cancel_for_detach();
    let abandoned_keys = app.abandon_pending_keys();
    // Cancellation is just an atomic flag and queue close. Restore the host
    // before joining the clipboard worker or doing network cleanup.
    let restored = restore_then_cleanup(
        &mut clipboard,
        |clipboard| {
            if let Some(writer) = clipboard.as_mut() {
                writer.request_shutdown();
            }
        },
        || host.restore(),
        |clipboard| {
            if let Some(writer) = clipboard.as_mut() {
                writer.shutdown();
            }
            if let Some(worker) = net.take() {
                if routes.is_some() {
                    let mut worker = worker;
                    worker.try_detach_nonblocking();
                } else {
                    worker.detach_after_restore();
                }
            }
        },
    );
    if abandoned_keys {
        report_notice(
            "[panelink: unsent queued keys were discarded on detach]",
            &mut host.tty,
        );
    }
    let code = match (result, restored) {
        (Ok(SERVER_ENDED_EXIT), Ok(())) => {
            report_notice(
                "[panelink: all shells exited; workspace closed]",
                &mut host.tty,
            );
            return 0;
        }
        (Ok(code), Ok(())) => return code,
        (Err(error), _) | (_, Err(error)) => error.message(),
    };
    if !report(code, &mut host.tty) {
        report_stderr_fallback(code);
    }
    1
}

/// Request prompt cancellation, restore the host, then run potentially
/// blocking cleanup such as joining the clipboard writer.
fn restore_then_cleanup<C, R>(
    context: &mut C,
    request_cancel: impl FnOnce(&mut C),
    restore: impl FnOnce() -> R,
    cleanup: impl FnOnce(&mut C),
) -> R {
    request_cancel(context);
    restore_then_detach(restore, || cleanup(context))
}

/// Restore terminal modes before any potentially blocking worker release.
pub(crate) fn restore_then_detach<R>(restore: impl FnOnce() -> R, detach: impl FnOnce()) -> R {
    let restored = restore();
    detach();
    restored
}

/// Fixed-text diagnostic before the host terminal was taken over.
///
/// Never uses inherited stderr (a full pipe would block indefinitely): it
/// writes through an independently opened, nonblocking `/dev/tty` with a
/// write deadline, like `p2_probe::report_error`, and gives up silently.
fn fail(message: &str) -> i32 {
    if let Ok(fd) = open(
        "/dev/tty",
        OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        if report(message, &mut File::from(fd)) {
            return 1;
        }
    }
    report_stderr_fallback(message);
    1
}

/// Bounded plain notice line to an already nonblocking terminal.
fn report_notice(message: &str, tty: &mut File) {
    let mut line = [0_u8; 160];
    let mut length = 0;
    for part in [message.as_bytes(), b"\r\n"] {
        let take = part.len().min(line.len() - length);
        line[length..length + take].copy_from_slice(&part[..take]);
        length += take;
    }
    let _ = write_control(tty, &line[..length]);
}

/// Bounded `error: MESSAGE\n` write to an already nonblocking terminal.
fn report(message: &str, tty: &mut impl Write) -> bool {
    let mut line = [0_u8; 160];
    let mut length = 0;
    for part in [b"error: ".as_slice(), message.as_bytes(), b"\r\n"] {
        let take = part.len().min(line.len() - length);
        line[length..length + take].copy_from_slice(&part[..take]);
        length += take;
    }
    write_control(tty, &line[..length]).is_ok()
}

/// If the controlling terminal is unavailable or stopped accepting writes,
/// make one bounded best-effort write to the inherited stderr endpoint. Open
/// `/dev/fd/2` separately with NONBLOCK so a full parent pipe cannot hold
/// attach shutdown. The message is always one of this module's fixed labels.
fn report_stderr_fallback(message: &str) {
    if let Ok(fd) = open(
        "/dev/fd/2",
        OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        let _ = report(message, &mut File::from(fd));
    }
}

fn previous_layout_availability(server_dir: &std::path::Path) -> Availability {
    match LayoutStore::open(&server_dir.join("layout"))
        .and_then(|store| store.load_bytes(LayoutSlot::Previous))
    {
        Ok(Some(_)) => Availability::Enabled,
        Ok(None) | Err(_) => Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
    }
}

struct RouteLoopState<'a> {
    controller: Option<&'a RouteController>,
    pending: Option<u64>,
    ready: Option<u64>,
    next_request_id: u64,
    host_size: (u16, u16),
    launch_context: Option<ProjectLaunchContext>,
    next_create_intent: u64,
    remembered_focus: Vec<(WorkspaceRouteId, panelink::control_protocol::TargetRef)>,
}

impl RouteLoopState<'_> {
    fn remember_focus(
        &mut self,
        route: WorkspaceRouteId,
        target: panelink::control_protocol::TargetRef,
    ) {
        if let Some((_, existing)) = self
            .remembered_focus
            .iter_mut()
            .find(|(stored_route, _)| *stored_route == route)
        {
            *existing = target;
        } else if self.remembered_focus.len()
            < panelink::workspace_index::MAX_PROJECTS
                * panelink::workspace_index::MAX_TABS_PER_PROJECT
        {
            self.remembered_focus.push((route, target));
        }
    }

    fn focus_for(&self, route: WorkspaceRouteId) -> Option<panelink::control_protocol::TargetRef> {
        self.remembered_focus
            .iter()
            .find(|(stored_route, _)| *stored_route == route)
            .map(|(_, target)| *target)
    }
}

/// Transfer a prepared state only when it belongs to the exact in-flight
/// route request. Stale attachments/empty states are dropped, never installed.
fn install_prepared_route(
    routes: &RouteLoopState<'_>,
    request_id: u64,
    prepared: PreparedWorkspace,
) -> Option<PreparedWorkspace> {
    if routes.pending != Some(request_id) || routes.ready != Some(request_id) {
        return None;
    }
    match &prepared {
        PreparedWorkspace::Attached(attachment) if attachment.request_id == request_id => {
            Some(prepared)
        }
        PreparedWorkspace::Empty(_) => Some(prepared),
        _ => None,
    }
}

fn handle_host_event(
    app: &mut App,
    event: &crossterm_input::Event,
    now: Instant,
    availability: Availability,
    worker_slot: &mut Option<WorkerHandle>,
) -> Result<Flow, HostError> {
    match worker_slot.as_mut() {
        Some(worker) => Ok(app.handle_event(event, now, availability, worker)),
        None => app
            .handle_empty_workspace_event(event, now)
            .ok_or(HostError::Setup),
    }
}

/// Resolve event-specific recovery availability, handle the event, then drain
/// its queued metadata and navigation intents in their established order.
fn dispatch_host_event(
    host: &HostGuard,
    app: &mut App,
    event: &crossterm_input::Event,
    worker_slot: &mut Option<WorkerHandle>,
    routes: &mut RouteLoopState<'_>,
    server_dir: Option<&std::path::Path>,
    now: Instant,
) -> Result<Flow, HostError> {
    let needs_check = app.needs_recovery_availability(event) && server_dir.is_some();
    let availability = if needs_check {
        server_dir.map_or(
            Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
            previous_layout_availability,
        )
    } else {
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
    };
    let flow = handle_host_event(app, event, now, availability, worker_slot)?;
    if flow != Flow::Continue {
        return Ok(flow);
    }

    let now = Instant::now();
    queue_pending_create(app, routes, now);
    queue_pending_workspace_mutation(app, routes, now);
    if let Some(destination) = app.take_route_request() {
        prepare_route_destination(host, app, routes, destination, Instant::now())?;
    }
    if app.take_catalog_refresh_request() {
        if let Some(controller) = routes.controller {
            if controller
                .queue_catalog_refresh(app.current_route())
                .is_err()
            {
                app.set_status("workspace catalog busy", Instant::now());
            }
        }
    }
    Ok(flow)
}

fn queue_pending_create(app: &mut App, routes: &mut RouteLoopState<'_>, now: Instant) {
    let Some((project, reconcile)) = app.take_create_request() else {
        return;
    };
    let Some(controller) = routes.controller else {
        app.reject_create_request("workspace metadata unavailable", now);
        return;
    };
    let Some(launch) = routes.launch_context.as_ref() else {
        app.reject_create_request("launch context unavailable", now);
        return;
    };
    // Reconciliation reuses the uncertain intent; only a fresh intent
    // advances the counter.
    let (intent_id, next_intent) = match reconcile {
        Some(intent) => (intent, routes.next_create_intent),
        None => match routes.next_create_intent.checked_add(1) {
            Some(next) => (CreateIntentId(routes.next_create_intent), next),
            None => {
                app.reject_create_request("workspace metadata unavailable", now);
                return;
            }
        },
    };
    match controller.queue_create(project, intent_id, launch.clone()) {
        Ok(request_id) => {
            routes.next_create_intent = next_intent;
            if !app.record_create_request(request_id, intent_id) {
                app.reject_create_request("workspace changed; create result ignored", now);
            }
        }
        Err(MetadataQueueError::Busy) => app.reject_create_request("workspace metadata busy", now),
        Err(MetadataQueueError::Unavailable | MetadataQueueError::IdentifierExhausted) => {
            app.reject_create_request("workspace metadata unavailable", now)
        }
    }
}

fn queue_pending_workspace_mutation(app: &mut App, routes: &RouteLoopState<'_>, now: Instant) {
    let Some(mutation) = app.take_workspace_metadata_mutation() else {
        return;
    };
    let Some(controller) = routes.controller else {
        app.reject_workspace_metadata_mutation("workspace metadata unavailable", now);
        return;
    };
    let result = match mutation {
        WorkspaceMetadataMutation::Rename {
            route,
            name,
            expected_revision,
        } => controller
            .queue_rename(route, name, expected_revision)
            .map(|request_id| (request_id, route, expected_revision)),
        WorkspaceMetadataMutation::Close {
            route,
            expected,
            expected_revision,
        } => controller
            .queue_close(route, expected)
            .map(|request_id| (request_id, route, expected_revision)),
    };
    match result {
        Ok((request_id, route, expected_revision)) => {
            app.track_metadata_request(request_id, route, expected_revision);
        }
        Err(MetadataQueueError::Busy) => {
            app.reject_workspace_metadata_mutation("workspace metadata busy", now)
        }
        Err(MetadataQueueError::Unavailable | MetadataQueueError::IdentifierExhausted) => {
            app.reject_workspace_metadata_mutation("workspace metadata unavailable", now)
        }
    }
}

fn prepare_route_destination(
    host: &HostGuard,
    app: &mut App,
    routes: &mut RouteLoopState<'_>,
    destination: WorkspaceRouteId,
    now: Instant,
) -> Result<(), HostError> {
    routes.host_size = host.size()?;
    app.set_host(routes.host_size.0, routes.host_size.1);
    let Some(controller) = routes.controller else {
        app.finish_route_switch(true, now);
        return Ok(());
    };
    let Some(next_request_id) = routes.next_request_id.checked_add(1) else {
        app.finish_route_switch(true, now);
        app.set_status("workspace switch unavailable", now);
        return Ok(());
    };
    let request_id = routes.next_request_id;
    routes.next_request_id = next_request_id;
    match controller.prepare(RouteRequest {
        request_id,
        destination,
        host_size: routes.host_size,
    }) {
        Ok(()) => routes.pending = Some(request_id),
        Err(label) => {
            app.finish_route_switch(true, now);
            app.set_status(label, now);
        }
    }
    Ok(())
}

fn install_empty_workspace(
    host: &mut HostGuard,
    app: &mut App,
    worker_slot: &mut Option<WorkerHandle>,
    resources: EmptyInstallResources<'_>,
    routes: &mut RouteLoopState<'_>,
    snapshot: TabPresentationSnapshot,
) -> Result<bool, HostError> {
    if routes.pending.is_some() || app.route_switching() {
        return Ok(false);
    }
    let (rows, columns) = host.size()?;
    let empty = App::new_workspace_empty(rows, columns, snapshot).map_err(|_| HostError::Frame)?;
    let request_id = routes.next_request_id;
    let Some(next_request_id) = request_id.checked_add(1) else {
        return Err(HostError::Setup);
    };
    routes.next_request_id = next_request_id;
    routes.pending = Some(request_id);
    routes.ready = Some(request_id);
    let prepared = install_prepared_route(routes, request_id, PreparedWorkspace::Empty(empty))
        .ok_or(HostError::Setup)?;
    let PreparedWorkspace::Empty(next_app) = prepared else {
        return Err(HostError::Setup);
    };

    app.cancel_for_detach();
    let _abandoned_keys = app.abandon_pending_keys();
    resources.output.discard();
    host.discard_pending_input()?;
    *resources.decoder = Decoder::new();
    resources.compositor.invalidate();
    if let Some(mut worker) = worker_slot.take() {
        worker.try_detach_nonblocking();
    }
    *app = next_app;
    *resources.server_dir = None;
    routes.host_size = (rows, columns);
    routes.pending = None;
    routes.ready = None;
    app.mark_dirty();
    Ok(true)
}

struct EmptyInstallResources<'a> {
    compositor: &'a mut Compositor,
    output: &'a mut Output,
    decoder: &'a mut Decoder,
    server_dir: &'a mut Option<PathBuf>,
}

fn event_loop(
    host: &mut HostGuard,
    app: &mut App,
    worker_slot: &mut Option<WorkerHandle>,
    frame: (&mut Compositor, &mut Output, &mut Decoder),
    clipboard: Option<&ClipboardWriter>,
    stop: &AtomicBool,
    routes: &mut RouteLoopState<'_>,
) -> Result<i32, HostError> {
    let (compositor, output, decoder) = frame;
    let mut server_dir = worker_slot
        .as_ref()
        .map(|worker| worker.server_directory().to_path_buf());
    let mut chunk = [0_u8; INPUT_CHUNK_BYTES];
    let (mut start, mut end) = (0_usize, 0_usize);
    let mut last_input_at: Option<Instant> = None;
    let mut last_key_at: Option<Instant> = None;
    let mut last_resize_check = Instant::now();
    let mut last_render: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(130);
        }
        let now = Instant::now();
        if start == end {
            start = 0;
            end = 0;
            match host.tty.read(&mut chunk) {
                Ok(0) => break Err(HostError::InputClosed),
                Ok(count) => {
                    end = count;
                    last_input_at = Some(now);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break Err(HostError::InputRead),
            }
        }
        let work_started = Instant::now();
        let mut quit = false;
        let mut view_recovery = false;
        while start < end {
            let byte = chunk[start];
            start += 1;
            let event = match decoder.advance(byte, start < end) {
                Ok(event) => event,
                Err(_) => return Err(HostError::InputDecode),
            };
            if let Some(event) = event {
                last_key_at = Some(Instant::now());
                match dispatch_host_event(
                    host,
                    app,
                    &event,
                    worker_slot,
                    routes,
                    server_dir.as_deref(),
                    Instant::now(),
                )? {
                    Flow::Quit => {
                        quit = true;
                        break;
                    }
                    Flow::ViewRecovery => {
                        view_recovery = true;
                        break;
                    }
                    Flow::Continue => {}
                }
            }
            if decoder.pending_bytes() > 0 {
                last_input_at = Some(Instant::now());
            }
            if work_started.elapsed() >= INPUT_WORK_BUDGET {
                break;
            }
        }
        if view_recovery {
            let Some(worker) = worker_slot.as_mut() else {
                return Err(HostError::Setup);
            };
            let Some(server_dir) = server_dir.as_deref() else {
                return Err(HostError::Setup);
            };
            app.flush(Instant::now(), worker);
            output.discard();
            compositor.invalidate();
            *decoder = Decoder::new();
            start = 0;
            end = 0;
            host.discard_pending_input()?;
            match panelink::recovery_reader::read_recovery(server_dir, RecoverySlot::Previous) {
                Ok(view) => {
                    if let Err(label) = super::recovery::run_in_workspace(&view, host, stop) {
                        app.set_status(label, Instant::now());
                    }
                }
                Err(error) => app.set_status(
                    match error {
                        panelink::recovery_reader::RecoveryReadError::Storage => {
                            "recovery storage is unavailable or unsafe"
                        }
                        panelink::recovery_reader::RecoveryReadError::MissingLayout => {
                            "No previous layout saved"
                        }
                        panelink::recovery_reader::RecoveryReadError::InvalidLayout => {
                            "saved layout invalid"
                        }
                    },
                    Instant::now(),
                ),
            }
            host.discard_pending_input()?;
            *decoder = Decoder::new();
            compositor.invalidate();
            app.mark_dirty();
            last_render = None;
            last_input_at = None;
            last_key_at = None;
            continue;
        }
        if quit {
            break Ok(0);
        }
        if start == end && decoder.pending_bytes() > 0 {
            if let Some(last) = last_input_at {
                if decoder.pending_bytes() == 1 && last.elapsed() >= ESCAPE_TIMEOUT {
                    match decoder.finish_escape() {
                        Ok(Some(event)) => {
                            match dispatch_host_event(
                                host,
                                app,
                                &event,
                                worker_slot,
                                routes,
                                server_dir.as_deref(),
                                Instant::now(),
                            )? {
                                Flow::Quit => break Ok(0),
                                // A lone Esc never opens the viewer; ignore.
                                Flow::Continue | Flow::ViewRecovery => {}
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break Err(HostError::InputDecode),
                    }
                } else if last.elapsed() >= INCOMPLETE_INPUT_TIMEOUT {
                    break Err(HostError::InputTimeout);
                }
            }
        }
        let now = Instant::now();
        // Typed keys are batched briefly, then sent once (never resent).
        if last_key_at.is_some_and(|at| now.duration_since(at) >= KEY_FLUSH_DELAY) {
            if let Some(worker) = worker_slot.as_mut() {
                app.flush(now, worker);
            }
            last_key_at = None;
        }
        if now.duration_since(last_resize_check) >= RESIZE_INTERVAL {
            last_resize_check = now;
            let (rows, columns) = host.size()?;
            routes.host_size = (rows, columns);
            app.set_host(rows, columns);
        }
        if stop.load(Ordering::Relaxed) {
            break Ok(130);
        }
        let mut server_ended = false;
        if let Some(worker) = worker_slot.as_mut() {
            worker.poll(NetView {
                desired_root: if app.route_switching() || app.pane_mutations_blocked() {
                    None
                } else {
                    app.desired_root()
                },
            });
            while let Some(envelope) = worker.try_result() {
                match worker.release_result(envelope) {
                    ClientResult::Keys(result) | ClientResult::Paste(result) => {
                        if let Err(label) = result {
                            app.report_worker_status(label, now);
                        }
                    }
                    ClientResult::Scroll(update) => app.apply_scroll_update(update, now),
                    ClientResult::Split {
                        id,
                        focus_generation,
                        target,
                        result,
                    } => {
                        app.complete_split(id, focus_generation, target, result, now);
                    }
                    ClientResult::Close { id, target, result } => {
                        app.complete_close(id, target, result, now);
                    }
                    ClientResult::PaneOperation { id, action, result } => {
                        app.complete_pane_operation(id, action, result.map(|update| *update), now)
                    }
                    ClientResult::Poll(update) => {
                        let present = update.server_present;
                        app.apply_net_update(*update, now);
                        if !present {
                            server_ended = true;
                        }
                    }
                    ClientResult::Detached => {}
                }
            }
            app.flush_scroll(now, worker);
            if app.retry_pending_keys() {
                app.flush(now, worker);
            }
        }
        if let Some(clipboard) = clipboard {
            while let Some(result) = clipboard.poll_result_for() {
                app.report_clipboard_status(result.target, result.request_id, result.status, now);
            }
        }
        if let Some((target, request_id, text)) = app.take_copy_request() {
            match clipboard {
                Some(clipboard) => {
                    if let Err(status) = clipboard.try_send_request(target, request_id, text) {
                        app.report_clipboard_status(Some(target), Some(request_id), status, now);
                    }
                }
                None => app.report_clipboard_status(
                    Some(target),
                    Some(request_id),
                    ClipboardStatus::Unavailable,
                    now,
                ),
            }
        }
        if let Some(controller) = routes.controller {
            while let Some(result) = controller.try_metadata_result() {
                match result {
                    MetadataResult::Catalog {
                        source_route,
                        snapshot,
                        observed_revision,
                    } => {
                        app.update_workspace_presentation(
                            source_route,
                            snapshot,
                            observed_revision,
                        );
                    }
                    MetadataResult::CatalogError => {
                        app.set_status("workspace catalog unavailable", now);
                    }
                    MetadataResult::SaveError(label) => app.set_status(label, now),
                    created @ MetadataResult::Created { .. } => {
                        if let Some(destination) = app.complete_create_result(created, now) {
                            app.begin_route_switch(now);
                            prepare_route_destination(host, app, routes, destination, now)?;
                        }
                    }
                    MetadataResult::Renamed {
                        request_id,
                        route,
                        expected_revision,
                        result,
                        observed_revision,
                        snapshot,
                    } => {
                        let needs_refresh = observed_revision.is_none();
                        let metadata = MetadataResult::Renamed {
                            request_id,
                            route,
                            expected_revision,
                            result,
                            observed_revision,
                            snapshot,
                        };
                        if app.complete_rename_metadata(metadata, now)
                            && needs_refresh
                            && controller
                                .queue_catalog_refresh(app.current_route())
                                .is_err()
                        {
                            app.set_status("tab rename outcome unknown; refresh unavailable", now);
                        }
                    }
                    MetadataResult::Closed {
                        request_id,
                        route,
                        expected,
                        outcome,
                        observed_revision,
                        snapshot,
                    } => {
                        if expected.route != route {
                            continue;
                        }
                        let metadata = MetadataResult::Closed {
                            request_id,
                            route,
                            expected,
                            outcome,
                            observed_revision,
                            snapshot: snapshot.clone(),
                        };
                        if !app.complete_close_metadata(metadata, now) {
                            continue;
                        }
                        if outcome == crate::workspace_lifecycle::CloseOutcome::Stopped
                            && app.current_route() == Some(route)
                        {
                            if let (Some(snapshot), Some(_revision)) = (snapshot, observed_revision)
                            {
                                if let Some(destination) = snapshot.active_route {
                                    if destination != route {
                                        app.begin_route_switch(now);
                                        prepare_route_destination(
                                            host,
                                            app,
                                            routes,
                                            destination,
                                            now,
                                        )?;
                                    } else {
                                        app.set_status("tab close metadata is inconsistent", now);
                                    }
                                } else if snapshot
                                    .projects
                                    .iter()
                                    .all(|project| project.tabs.is_empty())
                                    && install_empty_workspace(
                                        host,
                                        app,
                                        worker_slot,
                                        EmptyInstallResources {
                                            compositor,
                                            output,
                                            decoder,
                                            server_dir: &mut server_dir,
                                        },
                                        routes,
                                        snapshot,
                                    )?
                                {
                                    start = 0;
                                    end = 0;
                                    last_render = None;
                                    last_input_at = None;
                                    last_key_at = None;
                                } else {
                                    app.set_status("tab close metadata is inconsistent", now);
                                }
                            } else if controller.queue_catalog_refresh(Some(route)).is_err() {
                                app.set_status("tab stopped; workspace metadata unavailable", now);
                            }
                        }
                    }
                }
            }
        }
        app.expire_status(now);
        // Closing the selected workspace tab intentionally ends its current
        // server while the client prepares the deterministic adjacent route.
        // Do not mistake that expected old-worker EOF for the selected
        // destination's server ending. Empty installation also drops the old
        // worker before reaching this check. Transient connection loss keeps
        // the client open with its Disconnected indicator; a confirmed server
        // end still exits.
        if server_ended
            && worker_slot.is_some()
            && !app.route_switching()
            && !app.tab_close_pending()
            && !app.keep_tab_visible_on_server_end()
        {
            break Ok(SERVER_ENDED_EXIT);
        }

        if !output.pending()
            && last_render.is_none_or(|at| now.duration_since(at) >= RENDER_INTERVAL)
            && app.take_dirty()
        {
            let mut cursor = Cursor::new(&mut output.bytes[..]);
            match app.render(compositor, &mut cursor, now) {
                Ok(()) => {
                    output.cursor = 0;
                    output.len = cursor.position() as usize;
                    output.last_progress = now;
                    last_render = Some(now);
                    if output.len == 0 {
                        compositor.commit();
                    }
                }
                Err(_) => {
                    compositor.invalidate();
                    app.mark_dirty();
                }
            }
        }
        if output.pending() {
            match output.write_once(&mut host.tty) {
                Ok(true) => {
                    compositor.commit();
                }
                Ok(false) => {}
                Err(error) => break Err(error),
            }
        }
        thread::sleep(Duration::from_millis(1));
        if let Some(controller) = routes.controller {
            while let Some(result) = controller.try_result() {
                match result {
                    RouteResult::Ready { request_id } if routes.pending == Some(request_id) => {
                        routes.ready = Some(request_id);
                    }
                    RouteResult::Failed { request_id, label }
                        if routes.pending == Some(request_id) =>
                    {
                        routes.pending = None;
                        routes.ready = None;
                        app.finish_route_switch(true, Instant::now());
                        app.set_status(label, Instant::now());
                    }
                    RouteResult::Ready { request_id } | RouteResult::Failed { request_id, .. } => {
                        controller.cancel(request_id);
                    }
                }
            }
            if let Some(request_id) = routes.ready {
                if routes.pending == Some(request_id) {
                    if let Some(prepared) = controller.try_take_prepared(request_id) {
                        let Some(prepared) = install_prepared_route(
                            routes,
                            request_id,
                            PreparedWorkspace::Attached(prepared),
                        ) else {
                            controller.cancel(request_id);
                            routes.pending = None;
                            routes.ready = None;
                            continue;
                        };
                        let now = Instant::now();
                        // Move raw Net ownership directly onto its IO
                        // worker before any fallible host transition.
                        let (next_route, mut next_app, next_worker) = match prepared {
                            PreparedWorkspace::Attached(prepared) => (
                                Some(prepared.route),
                                prepared.app,
                                Some(WorkerHandle::start(prepared.net)),
                            ),
                            PreparedWorkspace::Empty(app) => (None, app, None),
                        };
                        if let (Some(route), Some(target)) =
                            (app.current_route(), app.focused_target())
                        {
                            routes.remember_focus(route, target);
                        }
                        if let Some(route) = next_route {
                            if let Some(target) = routes.focus_for(route) {
                                next_app.prefer_focus_when_valid(target);
                            }
                        }
                        let mut fresh_compositor =
                            Compositor::try_new().map_err(|_| HostError::Frame)?;
                        fresh_compositor
                            .set_reduced_control_color(compositor.reduced_control_color());
                        let fresh_output = Output::new()?;
                        app.cancel_for_detach();
                        let abandoned = app.abandon_pending_keys();
                        let fresh_host = ordered_route_transition(
                            || host.restore_and_drop(),
                            || {
                                if let Some(mut old_worker) = worker_slot.take() {
                                    old_worker.try_detach_nonblocking();
                                }
                            },
                            || {
                                *compositor = fresh_compositor;
                                *output = fresh_output;
                                *decoder = Decoder::new();
                                start = 0;
                                end = 0;
                                Ok(())
                            },
                            || {
                                let mut fresh = HostGuard::open()?;
                                fresh.enter()?;
                                Ok(fresh)
                            },
                        )?;
                        *host = fresh_host;
                        *app = next_app;
                        *worker_slot = next_worker;
                        server_dir = worker_slot
                            .as_ref()
                            .map(|worker| worker.server_directory().to_path_buf());
                        app.finish_route_switch(false, now);
                        if let Some(next_route) = next_route {
                            if controller.queue_selection_save(next_route).is_err() {
                                app.set_status("selection not saved", now);
                            }
                            // Selection persistence advances the catalog CAS
                            // revision. Refresh after the save so subsequent
                            // rename/close controls capture that revision.
                            if controller.queue_catalog_refresh(Some(next_route)).is_err() {
                                app.set_status("workspace catalog refresh unavailable", now);
                            }
                        }
                        if abandoned {
                            app.set_status("unsent queued keys discarded", now);
                        }
                        routes.pending = None;
                        routes.ready = None;
                        last_render = None;
                        last_input_at = None;
                        last_key_at = None;
                        app.mark_dirty();
                        continue;
                    }
                }
            }
        }
    }
}

fn ordered_route_transition<R, T, E>(
    restore: impl FnOnce() -> Result<R, E>,
    detach_old: impl FnOnce(),
    reset_client: impl FnOnce() -> Result<(), E>,
    open_destination: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let _restored = restore()?;
    detach_old();
    reset_client()?;
    open_destination()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_diagnostic_is_bounded_plain_text() {
        let mut output = Vec::new();
        assert!(report("terminal output made no progress", &mut output));
        assert_eq!(output, b"error: terminal output made no progress\r\n");
    }

    #[test]
    fn route_transition_restores_resets_then_opens_destination() {
        let order = std::cell::RefCell::new(Vec::new());
        let result = ordered_route_transition(
            || {
                order.borrow_mut().push("restore old guard");
                Ok::<_, ()>(())
            },
            || order.borrow_mut().push("nonblocking old worker detach"),
            || {
                order.borrow_mut().push("replace output decoder compositor");
                Ok(())
            },
            || {
                order.borrow_mut().push("open and enter fresh guard");
                Ok(7)
            },
        );
        assert_eq!(result, Ok(7));
        assert_eq!(
            *order.borrow(),
            [
                "restore old guard",
                "nonblocking old worker detach",
                "replace output decoder compositor",
                "open and enter fresh guard"
            ]
        );
    }

    #[test]
    fn failed_old_guard_restore_never_enters_destination_transition() {
        let order = std::cell::RefCell::new(Vec::new());
        let result = ordered_route_transition(
            || {
                order.borrow_mut().push("restore old guard");
                Err::<(), _>(HostError::Restore)
            },
            || order.borrow_mut().push("detach old worker"),
            || {
                order.borrow_mut().push("reset");
                Ok(())
            },
            || {
                order.borrow_mut().push("open destination guard");
                Ok(())
            },
        );
        assert!(matches!(result, Err(HostError::Restore)));
        assert_eq!(*order.borrow(), ["restore old guard"]);
    }

    #[test]
    fn exit_restores_the_terminal_before_any_network_detach_work() {
        let order = std::cell::RefCell::new(Vec::new());
        let result = restore_then_cleanup(
            &mut (),
            |_| order.borrow_mut().push("cancel"),
            || {
                order.borrow_mut().push("restore");
                7
            },
            |_| order.borrow_mut().push("clipboard shutdown and detach"),
        );
        assert_eq!(result, 7);
        assert_eq!(
            *order.borrow(),
            ["cancel", "restore", "clipboard shutdown and detach"]
        );
    }

    #[test]
    fn restore_sequence_disables_every_mode_enter_enabled() {
        for (enable, disable) in [
            (&b"\x1b[?1049h"[..], &b"\x1b[?1049l"[..]),
            (b"\x1b[?25l", b"\x1b[?25h"),
            (b"\x1b[?2004h", b"\x1b[?2004l"),
            (b"\x1b[?1002h", b"\x1b[?1002l"),
            (b"\x1b[?1006h", b"\x1b[?1006l"),
        ] {
            assert!(ENTER_SEQUENCE.windows(enable.len()).any(|w| w == enable));
            assert!(RESTORE_SEQUENCE
                .windows(disable.len())
                .any(|w| w == disable));
        }
        // Mouse reporting stops before the alternate screen is left.
        let mouse_1006 = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1006l")
            .unwrap();
        let mouse_1002 = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1002l")
            .unwrap();
        let conservative_mouse = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1000l")
            .unwrap();
        let screen = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1049l")
            .unwrap();
        assert!(mouse_1006 < mouse_1002);
        assert!(mouse_1002 < conservative_mouse);
        assert!(conservative_mouse < screen);
        assert!(!ENTER_SEQUENCE.windows(8).any(|w| w == b"\x1b[?1000h"));
        assert!(!ENTER_SEQUENCE.windows(8).any(|w| w == b"\x1b[?1003h"));
    }

    #[test]
    fn prepared_empty_workspace_requires_the_matching_request_id() {
        let snapshot = TabPresentationSnapshot::new(&WorkspaceIndex::new(), false);
        let empty = App::new_workspace_empty(24, 80, snapshot).unwrap();
        let routes = RouteLoopState {
            controller: None,
            pending: Some(7),
            ready: Some(7),
            next_request_id: 8,
            host_size: (24, 80),
            launch_context: None,
            next_create_intent: 1,
            remembered_focus: Vec::new(),
        };
        assert!(install_prepared_route(&routes, 8, PreparedWorkspace::Empty(empty)).is_none());

        let snapshot = TabPresentationSnapshot::new(&WorkspaceIndex::new(), false);
        let empty = App::new_workspace_empty(24, 80, snapshot).unwrap();
        assert!(matches!(
            install_prepared_route(&routes, 7, PreparedWorkspace::Empty(empty)),
            Some(PreparedWorkspace::Empty(_))
        ));
    }
}
