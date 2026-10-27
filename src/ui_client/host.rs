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
use super::routes::{MetadataResult, RouteController, RouteRequest, RouteResult};
use super::state::{App, Flow};
use super::worker::{ClientResult, WorkerHandle};
use panelink::recovery_view::RecoverySlot;
use panelink::workspace_index::WorkspaceIndex;
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
        run_inner(directory, context_dir, Some((app_root, index, current)))
    })) {
        Ok(code) => code,
        Err(_) => fail("internal error (terminal restored)"),
    }
}

fn run_inner(
    directory: PathBuf,
    context_dir: PathBuf,
    workspace: Option<(PathBuf, WorkspaceIndex, WorkspaceRouteId)>,
) -> i32 {
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
    let mut app = match App::new(rows, columns) {
        Ok(app) => app,
        Err(_) => return fail(HostError::Frame.message()),
    };
    if let Some((_, index, current)) = workspace.as_ref() {
        app.install_workspace_tabs(index.clone(), *current);
    }
    let mut compositor = match Compositor::try_new() {
        Ok(compositor) => compositor,
        Err(_) => return fail(HostError::Frame.message()),
    };
    let mut output = match Output::new() {
        Ok(output) => output,
        Err(error) => return fail(error.message()),
    };
    let mut decoder = Decoder::new();
    if clipboard.is_none() {
        app.report_clipboard_unavailable(Instant::now());
    }
    let connection = match Net::connect(directory, context_dir, &mut app, Instant::now()) {
        Ok(net) => net,
        Err(label) => return fail(label),
    };
    net = Some(WorkerHandle::start(connection));
    let routes = workspace
        .as_ref()
        .map(|(root, _, _)| RouteController::start(root.clone()));
    let mut route_state = RouteLoopState {
        controller: routes.as_ref(),
        pending: None,
        ready: None,
        next_request_id: 1,
        host_size: (rows, columns),
    };
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
    let Some(initial_worker) = worker_slot.as_ref() else {
        return Err(HostError::Setup);
    };
    let mut server_dir = initial_worker.server_directory().to_path_buf();
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
        let Some(worker) = worker_slot.as_mut() else {
            return Err(HostError::Setup);
        };
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
                let needs_check = app.needs_recovery_availability(&event);
                let availability = if needs_check {
                    previous_layout_availability(&server_dir)
                } else {
                    Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
                };
                match app.handle_event(&event, Instant::now(), availability, worker) {
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
                if let Some(destination) = app.take_route_request() {
                    routes.host_size = host.size()?;
                    app.set_host(routes.host_size.0, routes.host_size.1);
                    if let Some(controller) = routes.controller {
                        match routes.next_request_id.checked_add(1) {
                            Some(next) => {
                                let request_id = routes.next_request_id;
                                routes.next_request_id = next;
                                match controller.prepare(RouteRequest {
                                    request_id,
                                    destination,
                                    host_size: routes.host_size,
                                }) {
                                    Ok(()) => routes.pending = Some(request_id),
                                    Err(label) => {
                                        app.finish_route_switch(true, Instant::now());
                                        app.set_status(label, Instant::now());
                                    }
                                }
                            }
                            None => {
                                app.finish_route_switch(true, Instant::now());
                                app.set_status("workspace switch unavailable", Instant::now());
                            }
                        }
                    } else {
                        app.finish_route_switch(true, Instant::now());
                    }
                }
                if app.take_catalog_refresh_request() {
                    if let Some(controller) = routes.controller {
                        if controller.queue_catalog_refresh().is_err() {
                            app.set_status("workspace catalog busy", Instant::now());
                        }
                    }
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
            app.flush(Instant::now(), worker);
            output.discard();
            compositor.invalidate();
            *decoder = Decoder::new();
            start = 0;
            end = 0;
            host.discard_pending_input()?;
            match panelink::recovery_reader::read_recovery(&server_dir, RecoverySlot::Previous) {
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
                            let needs_check = app.needs_recovery_availability(&event);
                            let availability = if needs_check {
                                previous_layout_availability(&server_dir)
                            } else {
                                Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
                            };
                            match app.handle_event(&event, Instant::now(), availability, worker) {
                                Flow::Quit => break Ok(0),
                                // A lone Esc never opens the viewer; ignore.
                                Flow::Continue | Flow::ViewRecovery => {}
                            }
                            if let Some(destination) = app.take_route_request() {
                                routes.host_size = host.size()?;
                                app.set_host(routes.host_size.0, routes.host_size.1);
                                if let Some(controller) = routes.controller {
                                    let request_id = routes.next_request_id;
                                    if let Some(next) = request_id.checked_add(1) {
                                        routes.next_request_id = next;
                                        match controller.prepare(RouteRequest {
                                            request_id,
                                            destination,
                                            host_size: routes.host_size,
                                        }) {
                                            Ok(()) => routes.pending = Some(request_id),
                                            Err(label) => {
                                                app.finish_route_switch(true, Instant::now());
                                                app.set_status(label, Instant::now());
                                            }
                                        }
                                    } else {
                                        app.finish_route_switch(true, Instant::now());
                                        app.set_status(
                                            "workspace switch unavailable",
                                            Instant::now(),
                                        );
                                    }
                                }
                            }
                            if app.take_catalog_refresh_request() {
                                if let Some(controller) = routes.controller {
                                    if controller.queue_catalog_refresh().is_err() {
                                        app.set_status("workspace catalog busy", Instant::now());
                                    }
                                }
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
            app.flush(now, worker);
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
        worker.poll(NetView {
            desired_root: if app.route_switching() || app.pane_mutations_blocked() {
                None
            } else {
                app.desired_root()
            },
        });
        let mut server_ended = false;
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
        if let Some(clipboard) = clipboard {
            while let Some(status) = clipboard.poll_result() {
                app.report_worker_status(status.label(), now);
            }
        }
        if let Some(text) = app.take_copy_text() {
            match clipboard {
                Some(clipboard) => {
                    if let Err(status) = clipboard.try_send(text) {
                        app.report_worker_status(status.label(), now);
                    }
                }
                None => app.report_worker_status(ClipboardStatus::Unavailable.label(), now),
            }
        }
        if app.retry_pending_keys() {
            app.flush(now, worker);
        }
        if let Some(controller) = routes.controller {
            while let Some(result) = controller.try_metadata_result() {
                match result {
                    MetadataResult::Catalog(index) => {
                        app.update_workspace_catalog(index);
                    }
                    MetadataResult::CatalogError => {
                        app.set_status("workspace catalog unavailable", now);
                    }
                    MetadataResult::SaveError(label) => app.set_status(label, now),
                }
            }
        }
        if server_ended {
            break Ok(SERVER_ENDED_EXIT);
        }
        app.expire_status(now);

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
                        let now = Instant::now();
                        // Move raw Net ownership directly onto its IO
                        // worker before any fallible host transition.
                        let next_route = prepared.route;
                        let next_worker = WorkerHandle::start(prepared.net);
                        let next_app = prepared.app;
                        let fresh_compositor =
                            Compositor::try_new().map_err(|_| HostError::Frame)?;
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
                        *worker_slot = Some(next_worker);
                        server_dir = worker_slot
                            .as_ref()
                            .map(|worker| worker.server_directory().to_path_buf())
                            .ok_or(HostError::Setup)?;
                        app.finish_route_switch(false, now);
                        if controller.queue_selection_save(next_route).is_err() {
                            app.set_status("selection not saved", now);
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
}
