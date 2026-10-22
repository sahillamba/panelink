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
use panelink::terminal::compose::Compositor;
use rustix::fs::{open, Mode, OFlags};
use rustix::termios::{
    isatty, tcflush, tcgetattr, tcgetwinsize, tcsetattr, OptionalActions, QueueSelector, Termios,
};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

use super::net::{Net, NetView};
use super::state::{App, Flow};
use super::worker::{ClientResult, WorkerHandle};

/// Enter: alternate screen, hide cursor, bracketed paste, button mouse
/// reporting (1000) with SGR coordinates (1006).
pub(crate) const ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[?1000h\x1b[?1006h";
/// Restore in reverse: mouse off first, then p2_probe's restore sequence.
pub(crate) const RESTORE_SEQUENCE: &[u8] =
    b"\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?7h\x1b[?2004l\x1b[?25h\x1b[?1049l";
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
enum HostError {
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
    fn message(self) -> &'static str {
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

struct HostGuard {
    tty: File,
    original: Termios,
    restore_needed: bool,
    restored: bool,
}

impl HostGuard {
    fn open() -> Result<Self, HostError> {
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

    fn size(&self) -> Result<(u16, u16), HostError> {
        let size = tcgetwinsize(&self.tty).map_err(|_| HostError::Query)?;
        Ok((size.ws_row, size.ws_col))
    }

    fn enter(&mut self) -> Result<(), HostError> {
        let mut raw = self.original.clone();
        raw.make_raw();
        self.restore_needed = true;
        tcsetattr(&self.tty, OptionalActions::Now, &raw).map_err(|_| HostError::RawMode)?;
        write_control(&mut self.tty, ENTER_SEQUENCE).map_err(|_| HostError::Setup)
    }

    fn restore(&mut self) -> Result<(), HostError> {
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
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn write_control(tty: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
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
struct Output {
    bytes: Box<[u8]>,
    cursor: usize,
    len: usize,
    last_progress: Instant,
}

impl Output {
    fn new() -> Result<Self, HostError> {
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

    fn pending(&self) -> bool {
        self.cursor < self.len
    }

    /// Write once; returns `true` when the frame completed on this call.
    fn write_once(&mut self, tty: &mut File) -> Result<bool, HostError> {
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
        run_inner(directory, context_dir)
    })) {
        Ok(code) => code,
        Err(_) => fail("internal error (terminal restored)"),
    }
}

fn run_inner(directory: PathBuf, context_dir: PathBuf) -> i32 {
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
    let mut compositor = match Compositor::try_new() {
        Ok(compositor) => compositor,
        Err(_) => return fail(HostError::Frame.message()),
    };
    let mut output = match Output::new() {
        Ok(output) => output,
        Err(error) => return fail(error.message()),
    };
    let mut decoder = Decoder::new();
    let net = match Net::connect(directory, context_dir, &mut app, Instant::now()) {
        Ok(net) => net,
        Err(label) => return fail(label),
    };
    let mut net = Some(WorkerHandle::start(net));
    let result = host.enter().and_then(|()| {
        event_loop(
            &mut host,
            &mut app,
            &mut net,
            &mut compositor,
            &mut output,
            &mut decoder,
            &stop,
        )
    });
    let abandoned_keys = app.abandon_pending_keys();
    // Restore-first: the host terminal leaves raw/mouse/alternate-screen mode
    // before any further network work, so a stalled server can never hold
    // the user's terminal. (`HostGuard::drop` also restores on unwind.)
    let restored = restore_then_detach(
        || host.restore(),
        || {
            if let Some(worker) = net.take() {
                worker.detach_after_restore();
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
    report(code, &mut host.tty);
    1
}

/// Exit ordering: restore the host terminal first, then do the bounded
/// network release work (children keep running either way).
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
        report(message, &mut File::from(fd));
    }
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
fn report(message: &str, tty: &mut File) {
    let mut line = [0_u8; 160];
    let mut length = 0;
    for part in [b"error: ".as_slice(), message.as_bytes(), b"\r\n"] {
        let take = part.len().min(line.len() - length);
        line[length..length + take].copy_from_slice(&part[..take]);
        length += take;
    }
    let _ = write_control(tty, &line[..length]);
}

fn event_loop(
    host: &mut HostGuard,
    app: &mut App,
    worker_slot: &mut Option<WorkerHandle>,
    compositor: &mut Compositor,
    output: &mut Output,
    decoder: &mut Decoder,
    stop: &AtomicBool,
) -> Result<i32, HostError> {
    let Some(worker) = worker_slot.as_mut() else {
        return Err(HostError::Setup);
    };
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
        while start < end {
            let byte = chunk[start];
            start += 1;
            let event = match decoder.advance(byte, start < end) {
                Ok(event) => event,
                Err(_) => return Err(HostError::InputDecode),
            };
            if let Some(event) = event {
                last_key_at = Some(Instant::now());
                if app.handle_event(&event, Instant::now(), worker) == Flow::Quit {
                    quit = true;
                    break;
                }
            }
            if decoder.pending_bytes() > 0 {
                last_input_at = Some(Instant::now());
            }
            if work_started.elapsed() >= INPUT_WORK_BUDGET {
                break;
            }
        }
        if quit {
            break Ok(0);
        }
        if start == end && decoder.pending_bytes() > 0 {
            if let Some(last) = last_input_at {
                if decoder.pending_bytes() == 1 && last.elapsed() >= ESCAPE_TIMEOUT {
                    match decoder.finish_escape() {
                        Ok(Some(event)) => {
                            if app.handle_event(&event, Instant::now(), worker) == Flow::Quit {
                                break Ok(0);
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
            app.set_host(rows, columns);
        }
        if stop.load(Ordering::Relaxed) {
            break Ok(130);
        }
        worker.poll(NetView {
            focus: app.focus(),
            desired_root: app.desired_root(),
        });
        let mut server_ended = false;
        while let Some(envelope) = worker.try_result() {
            match worker.release_result(envelope) {
                ClientResult::Keys(result) | ClientResult::Paste(result) => {
                    if let Err(label) = result {
                        app.report_worker_status(label, now);
                    }
                }
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
                ClientResult::Poll(update) => {
                    let present = update.server_present;
                    app.apply_net_update(update, now);
                    if !present {
                        server_ended = true;
                    }
                }
                ClientResult::Detached => {}
            }
        }
        if app.retry_pending_keys() {
            app.flush(now, worker);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_restores_the_terminal_before_any_network_detach_work() {
        let order = std::cell::RefCell::new(Vec::new());
        let result = restore_then_detach(
            || {
                order.borrow_mut().push("restore");
                7
            },
            || order.borrow_mut().push("detach"),
        );
        assert_eq!(result, 7);
        assert_eq!(*order.borrow(), ["restore", "detach"]);
    }

    #[test]
    fn restore_sequence_disables_every_mode_enter_enabled() {
        for (enable, disable) in [
            (&b"\x1b[?1049h"[..], &b"\x1b[?1049l"[..]),
            (b"\x1b[?25l", b"\x1b[?25h"),
            (b"\x1b[?2004h", b"\x1b[?2004l"),
            (b"\x1b[?1000h", b"\x1b[?1000l"),
            (b"\x1b[?1006h", b"\x1b[?1006l"),
        ] {
            assert!(ENTER_SEQUENCE.windows(enable.len()).any(|w| w == enable));
            assert!(RESTORE_SEQUENCE
                .windows(disable.len())
                .any(|w| w == disable));
        }
        // Mouse reporting stops before the alternate screen is left.
        let mouse = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1000l")
            .unwrap();
        let screen = RESTORE_SEQUENCE
            .windows(8)
            .position(|w| w == b"\x1b[?1049l")
            .unwrap();
        assert!(mouse < screen);
    }
}
