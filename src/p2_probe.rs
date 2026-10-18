//! One-shot, bounded host-terminal adapter for the Phase 2 feasibility probe.
//!
//! The probe owns a separately opened `/dev/tty`, a pure byte decoder, one
//! bounded terminal core, and one bounded renderer frame. It is deliberately
//! binary-private: signal handlers live until this process exits.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm_input::{Decoder, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::terminal::{
    Key, PumpOutcome, ScreenSnapshot, TerminalError, TerminalSession, TerminalSize,
};
use rustix::fs::{open, Mode, OFlags};
use rustix::termios::{
    isatty, tcflush, tcgetattr, tcgetwinsize, tcsetattr, OptionalActions, QueueSelector, Termios,
};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;

const MIN_HOST_ROWS: u16 = 5;
const MIN_HOST_COLUMNS: u16 = 20;
const MAX_CORE_ROWS: u16 = 80;
const MAX_CORE_COLUMNS: u16 = 240;
const MAX_ENVIRONMENT_VARIABLES: usize = 4_096;
const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const INPUT_CHUNK_BYTES: usize = 1024;
const INPUT_WORK_BUDGET: Duration = Duration::from_millis(5);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(50);
const INCOMPLETE_INPUT_TIMEOUT: Duration = Duration::from_secs(2);
const RESIZE_INTERVAL: Duration = Duration::from_millis(100);
const RENDER_INTERVAL: Duration = Duration::from_millis(50);
const OUTPUT_STALL_TIMEOUT: Duration = Duration::from_secs(1);
const FINAL_FRAME_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_millis(100);
const FRAME_BYTES: usize = 3 * 1024 * 1024;
const MAX_OUTPUT_WRITE: usize = 64 * 1024;
const RESTORE_SEQUENCE: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?2004l\x1b[?25h\x1b[?1049l";
const ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h";

static RUN_STARTED: AtomicBool = AtomicBool::new(false);

/// The result of the disposable child session.
pub(crate) enum ProbeExit {
    Child(std::process::ExitStatus),
    Quit,
    Interrupted,
}

/// Stable host-probe errors. Variants intentionally carry no caller data.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProbeError {
    AlreadyRun,
    InvalidLaunchProfile,
    HostNotTty,
    HostOpen,
    HostQuery,
    HostSize,
    SignalSetup,
    ChildSpawn,
    RawMode,
    HostSetupOutput,
    InputRead,
    InputClosed,
    InputDecode,
    InputTimeout,
    InputRejected,
    PasteRejected,
    CorePump,
    Resize,
    FrameOverflow,
    OutputWrite,
    OutputStalled,
    FinalFrameTimeout,
    HostRestore,
    ChildCleanup,
}

impl fmt::Debug for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ProbeError {}

impl ProbeError {
    fn message(self) -> &'static str {
        match self {
            Self::AlreadyRun => "host probe is one-shot",
            Self::InvalidLaunchProfile => "launch profile exceeds the probe limits",
            Self::HostNotTty => "host input and output must be terminals",
            Self::HostOpen => "could not open the controlling terminal",
            Self::HostQuery => "could not query the controlling terminal",
            Self::HostSize => "host terminal dimensions are outside the supported range",
            Self::SignalSetup => "could not register stop-signal flags",
            Self::ChildSpawn => "could not start the probe child",
            Self::RawMode => "could not enter host raw mode",
            Self::HostSetupOutput => "could not initialize host terminal modes",
            Self::InputRead => "could not read host terminal input",
            Self::InputClosed => "host terminal input closed",
            Self::InputDecode => "host terminal input was malformed or oversized",
            Self::InputTimeout => "host terminal input sequence timed out",
            Self::InputRejected => "terminal input was rejected by the child",
            Self::PasteRejected => "host paste exceeded the child input limit",
            Self::CorePump => "terminal core operation failed",
            Self::Resize => "could not apply host terminal dimensions",
            Self::FrameOverflow => "rendered frame exceeded its fixed buffer",
            Self::OutputWrite => "could not write the rendered host frame",
            Self::OutputStalled => "host terminal output made no progress",
            Self::FinalFrameTimeout => "final host frame exceeded its deadline",
            Self::HostRestore => "could not restore host terminal state",
            Self::ChildCleanup => "could not complete bounded child cleanup",
        }
    }
}

/// Best-effort, bounded diagnostic for a failure after host I/O begins.
/// It opens the controlling terminal independently and never falls back to
/// inherited stderr, whose descriptor may be blocking or stalled.
pub(crate) fn report_error(error: &ProbeError) {
    let Ok(fd) = open(
        "/dev/tty",
        OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return;
    };
    let mut bytes = [0_u8; 128];
    let end = error_line(*error, &mut bytes);
    let mut tty = File::from(fd);
    let _ = write_control(&mut tty, &bytes[..end], CONTROL_WRITE_TIMEOUT);
}

fn error_line(error: ProbeError, bytes: &mut [u8; 128]) -> usize {
    const PREFIX: &[u8] = b"error: ";
    let message = error.message().as_bytes();
    let end = PREFIX.len() + message.len() + 1;
    debug_assert!(end <= bytes.len());
    bytes[..PREFIX.len()].copy_from_slice(PREFIX);
    bytes[PREFIX.len()..PREFIX.len() + message.len()].copy_from_slice(message);
    bytes[end - 1] = b'\n';
    end
}

struct HostSize {
    rows: u16,
    columns: u16,
}

impl HostSize {
    fn query(fd: &File) -> Result<Self, ProbeError> {
        let size = tcgetwinsize(fd).map_err(|_| ProbeError::HostQuery)?;
        if size.ws_row == 0
            || size.ws_col == 0
            || size.ws_row < MIN_HOST_ROWS
            || size.ws_col < MIN_HOST_COLUMNS
        {
            return Err(ProbeError::HostSize);
        }
        Ok(Self {
            rows: size.ws_row,
            columns: size.ws_col,
        })
    }

    fn core_size(&self) -> Result<TerminalSize, ProbeError> {
        TerminalSize::new(
            self.rows.min(MAX_CORE_ROWS),
            self.columns.min(MAX_CORE_COLUMNS),
        )
        .map_err(|_| ProbeError::HostSize)
    }
}

struct HostGuard {
    tty: Option<File>,
    original: Termios,
    restore_needed: bool,
    restoration: Option<Result<(), ProbeError>>,
}

impl HostGuard {
    fn open() -> Result<(Self, HostSize), ProbeError> {
        if !isatty(std::io::stdin()) || !isatty(std::io::stdout()) {
            return Err(ProbeError::HostNotTty);
        }
        let fd = open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| ProbeError::HostOpen)?;
        let tty = File::from(fd);
        let original = tcgetattr(&tty).map_err(|_| ProbeError::HostQuery)?;
        let size = HostSize::query(&tty)?;
        Ok((
            Self {
                tty: Some(tty),
                original,
                restore_needed: false,
                restoration: None,
            },
            size,
        ))
    }

    fn enter_raw(&mut self) -> Result<(), ProbeError> {
        let mut raw = self.original.clone();
        raw.make_raw();
        // Set the obligation before tcsetattr: even a failing ioctl may have
        // changed part of the terminal state.
        self.restore_needed = true;
        tcsetattr(
            self.tty.as_ref().ok_or(ProbeError::HostOpen)?,
            OptionalActions::Now,
            &raw,
        )
        .map_err(|_| ProbeError::RawMode)
    }

    fn enter_visual_modes(&mut self) -> Result<(), ProbeError> {
        write_control(
            self.tty.as_mut().ok_or(ProbeError::HostOpen)?,
            ENTER_SEQUENCE,
            CONTROL_WRITE_TIMEOUT,
        )
        .map_err(|_| ProbeError::HostSetupOutput)
    }

    fn restore(&mut self) -> Result<(), ProbeError> {
        if let Some(result) = self.restoration {
            return result;
        }

        let mut failed = false;
        if self.restore_needed {
            if let Some(tty) = self.tty.as_mut() {
                if write_control(tty, RESTORE_SEQUENCE, CONTROL_WRITE_TIMEOUT).is_err() {
                    failed = true;
                }
                // Discard any bytes that arrived while raw input was enabled
                // before returning control to the outer terminal's shell.
                if tcflush(&*tty, QueueSelector::IFlush).is_err() {
                    failed = true;
                }
                // Always attempt the exact saved termios after visual cleanup,
                // even when the nonblocking output attempt failed.
                if tcsetattr(&*tty, OptionalActions::Now, &self.original).is_err() {
                    failed = true;
                }
                if tcflush(&*tty, QueueSelector::IFlush).is_err() {
                    failed = true;
                }
            } else {
                failed = true;
            }
        }
        let result = if failed {
            Err(ProbeError::HostRestore)
        } else {
            Ok(())
        };
        self.restoration = Some(result);
        result
    }

    fn close_after_child(&mut self) -> Result<(), ProbeError> {
        let mut failed = false;
        if self.restore_needed {
            if let Some(tty) = self.tty.as_ref() {
                if tcflush(tty, QueueSelector::IFlush).is_err() {
                    failed = true;
                }
            } else {
                failed = true;
            }
        }
        self.tty.take();
        if failed {
            Err(ProbeError::HostRestore)
        } else {
            Ok(())
        }
    }
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.restore();
        let _ = self.close_after_child();
    }
}

struct Resources {
    host: HostGuard,
    session: TerminalSession,
    cleanup: Option<Result<(), ProbeError>>,
}

impl Resources {
    fn cleanup(&mut self) -> Result<(), ProbeError> {
        if let Some(result) = self.cleanup {
            return result;
        }
        // Restoration is deliberately first so child cleanup never holds the
        // host terminal in raw mode while waiting through its bounded windows.
        let mut host_failed = self.host.restore().is_err();
        let child_result = self.session.shutdown();
        if self.host.close_after_child().is_err() {
            host_failed = true;
        }
        let result = if host_failed {
            Err(ProbeError::HostRestore)
        } else if child_result.is_err() {
            Err(ProbeError::ChildCleanup)
        } else {
            Ok(())
        };
        self.cleanup = Some(result);
        result
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

struct InputChunk {
    bytes: [u8; INPUT_CHUNK_BYTES],
    start: usize,
    end: usize,
}

impl InputChunk {
    fn new() -> Self {
        Self {
            bytes: [0; INPUT_CHUNK_BYTES],
            start: 0,
            end: 0,
        }
    }

    fn has_suffix(&self) -> bool {
        self.start < self.end
    }

    fn read_once(&mut self, tty: &mut File) -> Result<Option<Instant>, ProbeError> {
        debug_assert!(!self.has_suffix());
        self.start = 0;
        self.end = 0;
        match tty.read(&mut self.bytes) {
            Ok(0) => Err(ProbeError::InputClosed),
            Ok(count) => {
                self.end = count;
                Ok(Some(Instant::now()))
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(_) => Err(ProbeError::InputRead),
        }
    }
}

struct Frame {
    bytes: Box<[u8]>,
    cursor: usize,
    len: usize,
    last_progress: Option<Instant>,
    displayed: ScreenSnapshot,
    target: ScreenSnapshot,
    displayed_valid: bool,
}

impl Frame {
    fn new() -> Result<Self, ProbeError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(FRAME_BYTES)
            .map_err(|_| ProbeError::FrameOverflow)?;
        bytes.resize(FRAME_BYTES, 0);
        let size = TerminalSize::new(MAX_CORE_ROWS, MAX_CORE_COLUMNS)
            .map_err(|_| ProbeError::FrameOverflow)?;
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            cursor: 0,
            len: 0,
            last_progress: None,
            displayed: ScreenSnapshot::try_new(size).map_err(|_| ProbeError::FrameOverflow)?,
            target: ScreenSnapshot::try_new(size).map_err(|_| ProbeError::FrameOverflow)?,
            displayed_valid: false,
        })
    }

    fn pending(&self) -> bool {
        self.cursor < self.len
    }

    fn discard(&mut self) {
        self.clear_output();
        // A discarded suffix may already have painted part of its target.
        // The next frame must rebuild the display, even for a clamped resize.
        self.displayed_valid = false;
    }

    fn clear_output(&mut self) {
        self.cursor = 0;
        self.len = 0;
        self.last_progress = None;
    }

    fn render(&mut self, session: &TerminalSession, host: &HostSize) -> Result<(), ProbeError> {
        self.render_snapshot(session.snapshot(), host)
    }

    fn render_snapshot(
        &mut self,
        snapshot: &ScreenSnapshot,
        host: &HostSize,
    ) -> Result<(), ProbeError> {
        debug_assert!(!self.pending());
        self.clear_output();
        self.target.copy_from(snapshot);
        let mut output = Cursor::new(&mut self.bytes[..]);
        panelink::terminal::render::render_diff(
            &self.target,
            self.displayed_valid.then_some(&self.displayed),
            host.rows,
            host.columns,
            &mut output,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::WriteZero {
                ProbeError::FrameOverflow
            } else {
                ProbeError::OutputWrite
            }
        })?;
        let len = usize::try_from(output.position()).map_err(|_| ProbeError::FrameOverflow)?;
        if len > FRAME_BYTES {
            return Err(ProbeError::FrameOverflow);
        }
        self.len = len;
        if self.pending() {
            self.last_progress = Some(Instant::now());
        } else {
            self.commit_target();
        }
        Ok(())
    }

    fn commit_target(&mut self) {
        std::mem::swap(&mut self.displayed, &mut self.target);
        self.displayed_valid = true;
    }

    fn write_once(&mut self, tty: &mut impl Write) -> Result<(), ProbeError> {
        if !self.pending() {
            return Ok(());
        }
        let end = self.len.min(self.cursor.saturating_add(MAX_OUTPUT_WRITE));
        match tty.write(&self.bytes[self.cursor..end]) {
            Ok(0) => Err(ProbeError::OutputWrite),
            Ok(count) => {
                self.cursor += count;
                self.last_progress = Some(Instant::now());
                if !self.pending() {
                    // Only fully accepted output becomes the diff baseline.
                    // The core may have advanced since this target was built.
                    self.commit_target();
                }
                Ok(())
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::Interrupted =>
            {
                if self
                    .last_progress
                    .is_some_and(|last| last.elapsed() >= OUTPUT_STALL_TIMEOUT)
                {
                    Err(ProbeError::OutputStalled)
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(ProbeError::OutputWrite),
        }
    }
}

enum EventResult {
    Continue,
    Quit,
}

fn dispatch_event(session: &mut TerminalSession, event: Event) -> Result<EventResult, ProbeError> {
    match event {
        Event::Key(key) => dispatch_key(session, key),
        Event::Paste(text) => match session.paste(&text) {
            Ok(()) => Ok(EventResult::Continue),
            Err(TerminalError::InputDisabled) => Ok(EventResult::Continue),
            Err(_) => Err(ProbeError::PasteRejected),
        },
        Event::FocusGained | Event::FocusLost | Event::Mouse(_) | Event::Resize(_, _) => {
            Ok(EventResult::Continue)
        }
    }
}

fn dispatch_key(session: &mut TerminalSession, key: KeyEvent) -> Result<EventResult, ProbeError> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return Ok(EventResult::Continue);
    }
    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char('c' | 'C') => {
                queue_key(session, Key::Interrupt)?;
                Ok(EventResult::Continue)
            }
            KeyCode::Char('x' | 'X' | 'q' | 'Q') => Ok(EventResult::Quit),
            _ => Ok(EventResult::Continue),
        };
    }

    let key = match key.code {
        KeyCode::Char(character)
            if (key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
                && printable_character(character) =>
        {
            Key::Char(character)
        }
        KeyCode::Enter if key.modifiers.is_empty() => Key::Enter,
        KeyCode::Tab if key.modifiers.is_empty() => Key::Tab,
        KeyCode::Backspace if key.modifiers.is_empty() => Key::Backspace,
        KeyCode::Esc if key.modifiers.is_empty() => Key::Escape,
        KeyCode::Up if key.modifiers.is_empty() => Key::Up,
        KeyCode::Down if key.modifiers.is_empty() => Key::Down,
        KeyCode::Left if key.modifiers.is_empty() => Key::Left,
        KeyCode::Right if key.modifiers.is_empty() => Key::Right,
        KeyCode::Home if key.modifiers.is_empty() => Key::Home,
        KeyCode::End if key.modifiers.is_empty() => Key::End,
        _ => return Ok(EventResult::Continue),
    };
    queue_key(session, key)?;
    Ok(EventResult::Continue)
}

fn printable_character(character: char) -> bool {
    !character.is_control()
}

fn note_pending_activity(decoder: &Decoder, last_input_at: &mut Option<Instant>, now: Instant) {
    if decoder.pending_bytes() == 0 {
        *last_input_at = None;
    } else {
        *last_input_at = Some(now);
    }
}

fn incomplete_input_expired(
    decoder: &Decoder,
    last_input_at: Option<Instant>,
    now: Instant,
) -> bool {
    decoder.pending_bytes() > 0
        && last_input_at
            .is_some_and(|last| now.saturating_duration_since(last) >= INCOMPLETE_INPUT_TIMEOUT)
}

fn queue_key(session: &mut TerminalSession, key: Key) -> Result<(), ProbeError> {
    match session.key(key) {
        Ok(()) | Err(TerminalError::InputDisabled) => Ok(()),
        Err(_) => Err(ProbeError::InputRejected),
    }
}

fn validate_launch_limits(
    launch: &ResolvedLaunch,
    arguments: &[OsString],
    environment: &Environment,
) -> Result<(), ProbeError> {
    if arguments.len() > MAX_ARGUMENTS || environment.len() > MAX_ENVIRONMENT_VARIABLES {
        return Err(ProbeError::InvalidLaunchProfile);
    }
    let mut argument_bytes = launch.requested_path.as_os_str().as_bytes().len();
    if argument_bytes > MAX_ARGUMENT_BYTES {
        return Err(ProbeError::InvalidLaunchProfile);
    }
    for argument in arguments {
        argument_bytes = argument_bytes
            .checked_add(argument.as_bytes().len())
            .ok_or(ProbeError::InvalidLaunchProfile)?;
        if argument_bytes > MAX_ARGUMENT_BYTES {
            return Err(ProbeError::InvalidLaunchProfile);
        }
    }
    let mut environment_bytes = 0_usize;
    for (name, value) in environment {
        environment_bytes = environment_bytes
            .checked_add(name.as_bytes().len())
            .and_then(|total| total.checked_add(value.as_bytes().len()))
            .ok_or(ProbeError::InvalidLaunchProfile)?;
        if environment_bytes > MAX_ENVIRONMENT_BYTES {
            return Err(ProbeError::InvalidLaunchProfile);
        }
    }
    let term = environment.get(OsStr::new("TERM"));
    if term.map(|value| value.as_os_str().as_bytes()) != Some(b"panelink-probe".as_slice()) {
        return Err(ProbeError::InvalidLaunchProfile);
    }
    Ok(())
}

fn write_control(tty: &mut impl Write, bytes: &[u8], budget: Duration) -> io::Result<()> {
    let deadline = Instant::now() + budget;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        match tty.write(&bytes[cursor..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
            Ok(count) => cursor += count,
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted
                    || error.kind() == io::ErrorKind::WouldBlock =>
            {
                thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn register_stop_flag() -> Result<Arc<AtomicBool>, ProbeError> {
    let stop = Arc::new(AtomicBool::new(false));
    flag::register(SIGTERM, Arc::clone(&stop)).map_err(|_| ProbeError::SignalSetup)?;
    flag::register(SIGHUP, Arc::clone(&stop)).map_err(|_| ProbeError::SignalSetup)?;
    flag::register(SIGINT, Arc::clone(&stop)).map_err(|_| ProbeError::SignalSetup)?;
    Ok(stop)
}

/// Run one child using the explicit launch profile and current host TTY.
pub(crate) fn run(
    launch: &ResolvedLaunch,
    arguments: &[OsString],
    environment: &Environment,
) -> Result<ProbeExit, ProbeError> {
    if RUN_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ProbeError::AlreadyRun);
    }
    validate_launch_limits(launch, arguments, environment)?;

    // All host-independent bounded buffers exist before a child can be spawned.
    let mut decoder = Decoder::new();
    let mut chunk = InputChunk::new();
    let mut frame = Frame::new()?;
    let (host, initial_host_size) = HostGuard::open()?;
    let stop = register_stop_flag()?;
    if stop.load(Ordering::Relaxed) {
        return Ok(ProbeExit::Interrupted);
    }

    let initial_size = initial_host_size.core_size()?;
    let session = TerminalSession::spawn(launch, arguments, environment, initial_size)
        .map_err(|_| ProbeError::ChildSpawn)?;
    let mut resources = Resources {
        host,
        session,
        cleanup: None,
    };

    let body = (|| {
        if stop.load(Ordering::Relaxed) {
            return Ok(ProbeExit::Interrupted);
        }
        resources.host.enter_raw()?;
        resources.host.enter_visual_modes()?;
        run_loop(
            &mut resources,
            &mut decoder,
            &mut chunk,
            &mut frame,
            initial_host_size,
            &stop,
        )
    })();

    match resources.cleanup() {
        Err(cleanup_error) => Err(cleanup_error),
        Ok(()) => body,
    }
}

fn run_loop(
    resources: &mut Resources,
    decoder: &mut Decoder,
    chunk: &mut InputChunk,
    frame: &mut Frame,
    mut host_size: HostSize,
    stop: &AtomicBool,
) -> Result<ProbeExit, ProbeError> {
    let mut dirty = true;
    let mut last_render = None;
    let mut last_resize_check = Instant::now();
    let mut last_input_at = None;
    let mut pending_host_resize: Option<(HostSize, TerminalSize)> = None;
    let mut final_deadline = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(ProbeExit::Interrupted);
        }

        if !chunk.has_suffix() {
            let tty = resources.host.tty.as_mut().ok_or(ProbeError::HostOpen)?;
            if let Some(read_at) = chunk.read_once(tty)? {
                last_input_at = Some(read_at);
            }
        }

        let work_started = Instant::now();
        let mut processed = 0_usize;
        while chunk.has_suffix() && processed < INPUT_CHUNK_BYTES {
            if stop.load(Ordering::Relaxed) {
                return Ok(ProbeExit::Interrupted);
            }
            let byte = chunk.bytes[chunk.start];
            chunk.start += 1;
            processed += 1;
            if let Some(event) = decoder
                .advance(byte, true)
                .map_err(|_| ProbeError::InputDecode)?
            {
                if matches!(
                    dispatch_event(&mut resources.session, event)?,
                    EventResult::Quit
                ) {
                    return Ok(ProbeExit::Quit);
                }
            }
            // A completed key earlier in this same chunk must not erase the
            // inactivity clock for a later incomplete sequence.
            note_pending_activity(decoder, &mut last_input_at, Instant::now());
            if work_started.elapsed() >= INPUT_WORK_BUDGET {
                break;
            }
        }
        if !chunk.has_suffix() {
            chunk.start = 0;
            chunk.end = 0;
        }

        if !chunk.has_suffix() {
            if let (Some(last), 1) = (last_input_at, decoder.pending_bytes()) {
                if last.elapsed() >= ESCAPE_TIMEOUT {
                    if let Some(event) = decoder
                        .finish_escape()
                        .map_err(|_| ProbeError::InputDecode)?
                    {
                        if matches!(
                            dispatch_event(&mut resources.session, event)?,
                            EventResult::Quit
                        ) {
                            return Ok(ProbeExit::Quit);
                        }
                    }
                    if decoder.pending_bytes() == 0 {
                        last_input_at = None;
                    }
                }
            }
            if incomplete_input_expired(decoder, last_input_at, Instant::now()) {
                return Err(ProbeError::InputTimeout);
            }
        }

        let now = Instant::now();
        if now.duration_since(last_resize_check) >= RESIZE_INTERVAL {
            last_resize_check = now;
            let tty = resources.host.tty.as_ref().ok_or(ProbeError::HostOpen)?;
            let observed = HostSize::query(tty)?;
            if observed.rows != host_size.rows || observed.columns != host_size.columns {
                let core_size = observed.core_size()?;
                pending_host_resize = Some((observed, core_size));
                // Only renderer bytes may be abandoned. PTY input remains owned
                // by the core and the next frame starts with an ESC reset.
                frame.discard();
                dirty = true;
                if resources.session.size() != core_size {
                    resources
                        .session
                        .request_resize(core_size)
                        .map_err(|_| ProbeError::Resize)?;
                }
            }
        }

        if stop.load(Ordering::Relaxed) {
            return Ok(ProbeExit::Interrupted);
        }
        let PumpOutcome {
            bytes_read,
            pty_eof,
            child_exited,
            ..
        } = resources.session.pump().map_err(|_| ProbeError::CorePump)?;
        if bytes_read > 0 {
            dirty = true;
        }
        if let Some((observed, accepted)) = pending_host_resize {
            if resources.session.size() != accepted {
                return Err(ProbeError::Resize);
            }
            host_size = observed;
            pending_host_resize = None;
            dirty = true;
        }
        if pty_eof && child_exited && final_deadline.is_none() {
            final_deadline = Some(Instant::now() + FINAL_FRAME_TIMEOUT);
        }

        let now = Instant::now();
        if dirty
            && !frame.pending()
            && last_render.is_none_or(|last| now.duration_since(last) >= RENDER_INTERVAL)
        {
            frame.render(&resources.session, &host_size)?;
            last_render = Some(now);
            dirty = false;
        }

        if frame.pending() {
            let tty = resources.host.tty.as_mut().ok_or(ProbeError::HostOpen)?;
            frame.write_once(tty)?;
        }

        if let Some(deadline) = final_deadline {
            if !dirty && !frame.pending() {
                if let Some(status) = resources.session.exit_status() {
                    return Ok(ProbeExit::Child(status));
                }
            }
            if Instant::now() >= deadline {
                return Err(ProbeError::FinalFrameTimeout);
            }
        }

        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::os::fd::AsFd;
    use std::os::unix::ffi::OsStringExt;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
    use rustix::process::{kill_process_group, test_kill_process, Pid, Signal};
    use rustix::termios::{tcgetattr, LocalModes, Winsize};
    use rustix_openpty::openpty;

    const READY_CHILD_PREFIX: &str = "PANELINK-RESTORE-ORDER-READY:";
    const CHILD_HUP_GRACE: Duration = Duration::from_secs(1);

    fn launch(path: &str) -> ResolvedLaunch {
        ResolvedLaunch {
            requested_path: path.into(),
            executable_path: path.into(),
            working_directory: "/".into(),
        }
    }

    #[test]
    fn profile_caps_reject_before_any_host_resource_is_opened() {
        let mut environment = Environment::new();
        environment.insert("TERM".into(), "panelink-probe".into());
        let arguments = [OsString::from_vec(vec![b'x'; MAX_ARGUMENT_BYTES])];
        assert_eq!(
            validate_launch_limits(&launch("/bin/sh"), &arguments, &environment),
            Err(ProbeError::InvalidLaunchProfile)
        );

        let mut environment = BTreeMap::new();
        environment.insert("TERM".into(), "wrong-profile".into());
        assert_eq!(
            validate_launch_limits(&launch("/bin/sh"), &[], &environment),
            Err(ProbeError::InvalidLaunchProfile)
        );
    }

    #[test]
    fn pending_timeout_clock_survives_a_completed_key_in_the_same_read_chunk() {
        for bytes in [b"x\x1b".as_slice(), b"x\x1b[".as_slice()] {
            let mut decoder = Decoder::new();
            let mut last_input_at = None;
            let base = Instant::now();
            for (index, byte) in bytes.iter().copied().enumerate() {
                let _ = decoder.advance(byte, true).unwrap();
                let timestamp = base + Duration::from_millis(index as u64 + 1);
                note_pending_activity(&decoder, &mut last_input_at, timestamp);
            }
            assert!(decoder.pending_bytes() > 0);
            assert_eq!(
                last_input_at,
                Some(base + Duration::from_millis(bytes.len() as u64))
            );
        }
    }

    #[test]
    fn incomplete_single_byte_utf8_prefix_fails_closed_after_inactivity() {
        let mut decoder = Decoder::new();
        assert!(decoder.advance(0xc3, true).unwrap().is_none());
        assert_eq!(decoder.pending_bytes(), 1);
        let started = Instant::now();
        assert!(decoder.finish_escape().unwrap().is_none());
        assert_eq!(decoder.pending_bytes(), 1);
        assert!(!incomplete_input_expired(
            &decoder,
            Some(started),
            started + ESCAPE_TIMEOUT
        ));
        assert!(incomplete_input_expired(
            &decoder,
            Some(started),
            started + INCOMPLETE_INPUT_TIMEOUT
        ));
    }

    #[test]
    fn only_non_control_scalars_are_eligible_for_typed_key_forwarding() {
        assert!(!printable_character('\0'));
        assert!(!printable_character('\u{7f}'));
        assert!(!printable_character('\u{85}'));
        assert!(printable_character('é'));
        assert!(printable_character('界'));
    }

    #[test]
    fn ctrl_x_decodes_to_local_quit_without_queuing_child_input() {
        let environment =
            Environment::from([(OsString::from("TERM"), OsString::from("panelink-probe"))]);
        let mut session = TerminalSession::spawn(
            &launch("/bin/sleep"),
            &[OsString::from("30")],
            &environment,
            TerminalSize::new(24, 80).unwrap(),
        )
        .unwrap();
        let event = Decoder::new().advance(0x18, true).unwrap().unwrap();
        assert!(matches!(
            dispatch_event(&mut session, event).unwrap(),
            EventResult::Quit
        ));
        assert_eq!(session.pending_input_bytes(), 0);
        // Plain x remains ordinary child input.
        let event = Decoder::new().advance(b'x', true).unwrap().unwrap();
        assert!(matches!(
            dispatch_event(&mut session, event).unwrap(),
            EventResult::Continue
        ));
        assert_eq!(session.pending_input_bytes(), 1);
        session.shutdown().unwrap();
    }

    #[test]
    fn host_error_diagnostic_is_fixed_text_and_uses_a_bounded_write_budget() {
        let mut bytes = [0_u8; 128];
        let length = error_line(ProbeError::OutputStalled, &mut bytes);
        assert_eq!(
            &bytes[..length],
            b"error: host terminal output made no progress\n"
        );

        struct WouldBlock;
        impl Write for WouldBlock {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let started = Instant::now();
        assert_eq!(
            write_control(&mut WouldBlock, &bytes[..length], CONTROL_WRITE_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    struct PartialThenError {
        bytes: Vec<u8>,
    }

    impl Write for PartialThenError {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            if self.bytes.is_empty() {
                self.bytes.extend_from_slice(&input[..input.len().min(2)]);
                Ok(input.len().min(2))
            } else {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn setup_control_write_stops_after_a_partial_hard_error() {
        let mut writer = PartialThenError { bytes: Vec::new() };
        let error = write_control(&mut writer, b"trusted-control", Duration::from_millis(50))
            .expect_err("injected second write must fail");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(writer.bytes, b"tr");
    }

    #[test]
    fn frame_baseline_advances_only_after_all_output_and_resize_invalidates_it() {
        struct ShortWriter(usize);
        impl Write for ShortWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len().min(self.0))
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("host frames must never call blocking flush");
            }
        }

        let host = HostSize {
            rows: 24,
            columns: 80,
        };
        let first = ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap();
        let second = ScreenSnapshot::try_new(TerminalSize::new(6, 20).unwrap()).unwrap();
        let mut frame = Frame::new().unwrap();
        frame.render_snapshot(&first, &host).unwrap();
        frame.write_once(&mut ShortWriter(3)).unwrap();
        assert!(frame.pending());
        assert!(!frame.displayed_valid);
        while frame.pending() {
            frame
                .write_once(&mut ShortWriter(MAX_OUTPUT_WRITE))
                .unwrap();
        }
        assert!(frame.displayed_valid);
        assert_eq!(frame.displayed.rows(), 5);

        frame.render_snapshot(&second, &host).unwrap();
        frame.write_once(&mut ShortWriter(3)).unwrap();
        assert_eq!(
            frame.displayed.rows(),
            5,
            "partial target is not a baseline"
        );
        assert!(frame.pending());
        frame.discard();
        assert!(!frame.displayed_valid);
        frame.render_snapshot(&first, &host).unwrap();
        assert!(frame.bytes[..frame.len].windows(4).any(|b| b == b"\x1b[2J"));
        while frame.pending() {
            frame
                .write_once(&mut ShortWriter(MAX_OUTPUT_WRITE))
                .unwrap();
        }
        frame.render_snapshot(&first, &host).unwrap();
        assert!(!frame.pending(), "identical output must not repaint");
        assert_eq!(frame.len, 0);
        assert!(frame.displayed_valid);
        assert_eq!(frame.displayed.rows(), 5);
    }

    #[test]
    fn host_restore_flushes_already_queued_input_before_releasing_tty() {
        let pty = openpty(
            None,
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )
        .unwrap();
        let master = File::from(pty.controller);
        let observer = pty.user.as_fd().try_clone_to_owned().unwrap();
        let original = tcgetattr(&pty.user).unwrap();
        let flags = fcntl_getfl(&pty.user).unwrap();
        fcntl_setfl(&pty.user, flags | OFlags::NONBLOCK).unwrap();
        let mut host = HostGuard {
            tty: Some(File::from(pty.user)),
            original,
            restore_needed: false,
            restoration: None,
        };
        host.enter_raw().unwrap();
        host.enter_visual_modes().unwrap();
        (&master).write_all(b"already-queued\r").unwrap();
        host.restore().unwrap();
        host.close_after_child().unwrap();

        let mut readback = File::from(observer);
        let flags = fcntl_getfl(&readback).unwrap();
        fcntl_setfl(&readback, flags | OFlags::NONBLOCK).unwrap();
        let mut byte = [0_u8; 1];
        let error = readback
            .read(&mut byte)
            .expect_err("flushed input must not remain");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn unwinding_owner_drop_restores_saved_host_state() {
        let pty = openpty(
            None,
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )
        .unwrap();
        let observer = pty.user.as_fd().try_clone_to_owned().unwrap();
        let original = tcgetattr(&pty.user).unwrap();
        let flags = fcntl_getfl(&pty.user).unwrap();
        fcntl_setfl(&pty.user, flags | OFlags::NONBLOCK).unwrap();
        let host = HostGuard {
            tty: Some(File::from(pty.user)),
            original: original.clone(),
            restore_needed: false,
            restoration: None,
        };
        let environment =
            Environment::from([(OsString::from("TERM"), OsString::from("panelink-probe"))]);
        let launch = ResolvedLaunch {
            requested_path: "/bin/sh".into(),
            executable_path: "/bin/sh".into(),
            working_directory: "/".into(),
        };
        let arguments = [
            OsString::from("-c"),
            OsString::from("trap '' HUP; while :; do sleep 1; done"),
        ];
        let session = TerminalSession::spawn(
            &launch,
            &arguments,
            &environment,
            TerminalSize::new(24, 80).unwrap(),
        )
        .unwrap();
        let mut resources = Resources {
            host,
            session,
            cleanup: None,
        };
        resources.host.enter_raw().unwrap();
        resources.host.enter_visual_modes().unwrap();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _resources = resources;
            panic!("injected unwind after raw host entry and child spawn");
        }));
        assert!(panic.is_err());

        let restored = tcgetattr(&observer).unwrap();
        assert_eq!(restored.input_modes, original.input_modes);
        assert_eq!(restored.output_modes, original.output_modes);
        assert_eq!(restored.control_modes, original.control_modes);
        assert_eq!(restored.local_modes, original.local_modes);
        assert_eq!(restored.input_speed(), original.input_speed());
        assert_eq!(restored.output_speed(), original.output_speed());
        assert_eq!(
            format!("{:?}", restored.special_codes),
            format!("{:?}", original.special_codes)
        );
        assert!(restored.local_modes.contains(LocalModes::ICANON));
    }

    #[derive(Clone, Copy)]
    enum CleanupPath {
        Explicit,
        Unwind,
    }

    enum CleanupCompletion {
        Explicit {
            result: Result<(), ProbeError>,
            child_reaped: bool,
            elapsed: Duration,
        },
        Unwind {
            panic_caught: bool,
            elapsed: Duration,
        },
    }

    fn termios_matches(actual: &Termios, expected: &Termios) -> bool {
        actual.input_modes == expected.input_modes
            && actual.output_modes == expected.output_modes
            && actual.control_modes == expected.control_modes
            && actual.local_modes == expected.local_modes
            && actual.input_speed() == expected.input_speed()
            && actual.output_speed() == expected.output_speed()
            && format!("{:?}", actual.special_codes) == format!("{:?}", expected.special_codes)
    }

    fn child_pid_from_ready_snapshot(snapshot: &ScreenSnapshot) -> Option<Pid> {
        let mut text = String::new();
        for column in 0..80 {
            if let Some(cell) = snapshot.cell(0, column) {
                text.push_str(cell.text());
            }
        }
        let marker = text.split_once(READY_CHILD_PREFIX)?.1;
        // A PTY read may split even this short marker. Never signal a process
        // group derived from an incomplete numeric prefix.
        let digits = marker.split_once(":READY-END")?.0;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Pid::from_raw(digits.parse().ok()?)
    }

    fn read_exact_with_deadline(file: &mut File, expected: &[u8], budget: Duration) {
        let deadline = Instant::now() + budget;
        let mut offset = 0;
        let mut bytes = [0_u8; 64];
        while offset < expected.len() {
            let count = expected.len().saturating_sub(offset).min(bytes.len());
            match file.read(&mut bytes[..count]) {
                Ok(0) => panic!("private PTY closed before the trusted control sequence arrived"),
                Ok(read) => {
                    assert_eq!(
                        &bytes[..read],
                        &expected[offset..offset + read],
                        "private PTY control sequence differed"
                    );
                    offset += read;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => panic!("private PTY control sequence read failed"),
            }
            assert!(
                Instant::now() < deadline,
                "private PTY control sequence exceeded its read deadline"
            );
            if offset < expected.len() {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn assert_restore_before_child_wait(path: CleanupPath) {
        let pty = openpty(
            None,
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )
        .unwrap();
        let mut master = File::from(pty.controller);
        let master_flags = fcntl_getfl(&master).unwrap();
        fcntl_setfl(&master, master_flags | OFlags::NONBLOCK).unwrap();
        let observer = pty.user.as_fd().try_clone_to_owned().unwrap();
        let original = tcgetattr(&pty.user).unwrap();
        let tty_flags = fcntl_getfl(&pty.user).unwrap();
        fcntl_setfl(&pty.user, tty_flags | OFlags::NONBLOCK).unwrap();
        let host = HostGuard {
            tty: Some(File::from(pty.user)),
            original: original.clone(),
            restore_needed: false,
            restoration: None,
        };
        let environment = Environment::from([
            (OsString::from("TERM"), OsString::from("panelink-probe")),
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
        ]);
        let arguments = [
            OsString::from("-c"),
            OsString::from(format!(
                "trap '' HUP; printf '{READY_CHILD_PREFIX}%s:READY-END\\n' \"$$\"; exec /bin/sleep 30"
            )),
        ];
        let session = TerminalSession::spawn(
            &launch("/bin/sh"),
            &arguments,
            &environment,
            TerminalSize::new(24, 80).unwrap(),
        )
        .unwrap();
        let mut resources = Resources {
            host,
            session,
            cleanup: None,
        };

        let ready_deadline = Instant::now() + Duration::from_secs(2);
        let child_pid = loop {
            resources.session.pump().unwrap();
            if let Some(pid) = child_pid_from_ready_snapshot(resources.session.snapshot()) {
                break pid;
            }
            assert!(
                Instant::now() < ready_deadline,
                "HUP-ignoring child did not send its readiness marker"
            );
            thread::sleep(Duration::from_millis(1));
        };

        resources.host.enter_raw().unwrap();
        resources.host.enter_visual_modes().unwrap();
        read_exact_with_deadline(&mut master, ENTER_SEQUENCE, Duration::from_millis(250));

        // Exercise the installed HUP disposition with a signal scoped to this
        // test child. The child prints readiness only after installing it.
        kill_process_group(child_pid, Signal::HUP).unwrap();
        thread::sleep(Duration::from_millis(25));
        assert!(
            !resources.session.pump().unwrap().child_exited,
            "ready child exited after the test HUP"
        );
        assert!(
            test_kill_process(child_pid).is_ok(),
            "ready child must remain alive after the test HUP"
        );

        let cleanup_finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&cleanup_finished);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (completion_tx, completion_rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("panelink-restore-order-cleanup".into())
            .spawn(move || {
                let started = Instant::now();
                let _ = started_tx.send(started);
                let completion = match path {
                    CleanupPath::Explicit => {
                        let result = resources.cleanup();
                        let child_reaped = resources.session.exit_status().is_some();
                        drop(resources);
                        CleanupCompletion::Explicit {
                            result,
                            child_reaped,
                            elapsed: started.elapsed(),
                        }
                    }
                    CleanupPath::Unwind => {
                        let caught = catch_unwind(AssertUnwindSafe(move || {
                            let _resources = resources;
                            std::panic::resume_unwind(Box::new("injected cleanup unwind"));
                        }));
                        CleanupCompletion::Unwind {
                            panic_caught: caught.is_err(),
                            elapsed: started.elapsed(),
                        }
                    }
                };
                worker_finished.store(true, Ordering::Release);
                let _ = completion_tx.send(completion);
            })
            .unwrap();
        let worker_started = started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cleanup worker did not start within its bound");

        let observation_deadline = worker_started + Duration::from_millis(1_500);
        let mut restore_offset = 0;
        let mut restore_bytes = [0_u8; 64];
        let restore_observed_at = loop {
            if restore_offset < RESTORE_SEQUENCE.len() {
                let count = (RESTORE_SEQUENCE.len() - restore_offset).min(restore_bytes.len());
                match master.read(&mut restore_bytes[..count]) {
                    Ok(0) => panic!("private PTY closed before host restoration completed"),
                    Ok(read) => {
                        assert_eq!(
                            &restore_bytes[..read],
                            &RESTORE_SEQUENCE[restore_offset..restore_offset + read],
                            "observed host restoration bytes did not match the exact sequence"
                        );
                        restore_offset += read;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => panic!("private PTY restoration read failed"),
                }
            }

            let restored =
                tcgetattr(&observer).is_ok_and(|actual| termios_matches(&actual, &original));
            if restore_offset == RESTORE_SEQUENCE.len() && restored {
                let observed_at = Instant::now();
                let elapsed = observed_at.duration_since(worker_started);
                assert!(
                    test_kill_process(child_pid).is_ok(),
                    "child was no longer alive when host restoration was observed"
                );
                assert!(
                    elapsed < CHILD_HUP_GRACE,
                    "host restoration was observed after the one-second child HUP grace: {elapsed:?}"
                );
                assert!(
                    !cleanup_finished.load(Ordering::Acquire),
                    "cleanup finished before saved termios and the exact visual reset were observed"
                );
                break observed_at;
            }
            assert!(
                !cleanup_finished.load(Ordering::Acquire),
                "cleanup completed before saved termios and the exact visual reset were observed"
            );
            assert!(
                Instant::now() < observation_deadline,
                "saved termios and the exact visual reset were not both observed within the bound"
            );
            thread::sleep(Duration::from_millis(1));
        };

        let completion = completion_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("bounded child cleanup did not finish within its deadline");
        assert!(cleanup_finished.load(Ordering::Acquire));
        let cleanup_elapsed = match completion {
            CleanupCompletion::Explicit {
                result,
                child_reaped,
                elapsed,
            } => {
                assert_eq!(result, Ok(()));
                assert!(child_reaped, "explicit cleanup must reap the child");
                elapsed
            }
            CleanupCompletion::Unwind {
                panic_caught,
                elapsed,
            } => {
                assert!(
                    panic_caught,
                    "fixture panic must unwind through Resources::drop"
                );
                assert_eq!(
                    test_kill_process(child_pid),
                    Err(rustix::io::Errno::SRCH),
                    "unwind cleanup must stop and reap the child"
                );
                elapsed
            }
        };
        assert!(
            cleanup_elapsed >= CHILD_HUP_GRACE,
            "HUP-ignoring child cleanup ended before its grace window: {cleanup_elapsed:?}"
        );
        let observed_ms = restore_observed_at
            .duration_since(worker_started)
            .as_millis();
        eprintln!(
            "restore_order_path={}; restore_observed_ms={observed_ms}; child_cleanup_ms={}; exact_sequence_bytes={}; child_alive_at_restore=true",
            match path {
                CleanupPath::Explicit => "explicit",
                CleanupPath::Unwind => "unwind-drop",
            },
            cleanup_elapsed.as_millis(),
            RESTORE_SEQUENCE.len()
        );
        drop(worker);
    }

    #[test]
    fn explicit_cleanup_restores_host_before_child_shutdown_wait() {
        assert_restore_before_child_wait(CleanupPath::Explicit);
    }

    #[test]
    fn unwind_drop_restores_host_before_child_shutdown_wait() {
        assert_restore_before_child_wait(CleanupPath::Unwind);
    }

    #[test]
    fn unread_outer_pty_returns_output_stalled_before_bounded_cleanup() {
        let pty = openpty(
            None,
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )
        .unwrap();
        // Keep the master unread for the whole loop and cleanup. The retained
        // slave observer lets this test check restoration after the child is
        // reaped without relying on /dev/tty or a test-runner session leader.
        let _unread_master = File::from(pty.controller);
        let observer = pty.user.as_fd().try_clone_to_owned().unwrap();
        let original = tcgetattr(&pty.user).unwrap();
        let flags = fcntl_getfl(&pty.user).unwrap();
        fcntl_setfl(&pty.user, flags | OFlags::NONBLOCK).unwrap();
        let host = HostGuard {
            tty: Some(File::from(pty.user)),
            original: original.clone(),
            restore_needed: false,
            restoration: None,
        };

        let mut decoder = Decoder::new();
        let mut chunk = InputChunk::new();
        let mut frame = Frame::new().unwrap();
        let environment = Environment::from([
            (OsString::from("TERM"), OsString::from("panelink-probe")),
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
        ]);
        let arguments = [
            OsString::from("-c"),
            OsString::from(
                "trap '' HUP; printf 'HUP-READY'; /bin/sleep 0.25; while :; do printf '\\n'; /bin/sleep 0.01; done",
            ),
        ];
        let session = TerminalSession::spawn(
            &launch("/bin/sh"),
            &arguments,
            &environment,
            TerminalSize::new(24, 80).unwrap(),
        )
        .unwrap();
        let mut resources = Resources {
            host,
            session,
            cleanup: None,
        };

        let ready_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            resources.session.pump().unwrap();
            let ready = b"HUP-READY".iter().enumerate().all(|(column, byte)| {
                resources
                    .session
                    .snapshot()
                    .cell(0, column as u16)
                    .is_some_and(|cell| cell.text().as_bytes() == [*byte])
            });
            if ready {
                break;
            }
            assert!(
                Instant::now() < ready_deadline,
                "child did not report readiness after installing its HUP disposition"
            );
            thread::sleep(Duration::from_millis(1));
        }

        resources.host.enter_raw().unwrap();
        resources.host.enter_visual_modes().unwrap();
        assert!(
            fcntl_getfl(resources.host.tty.as_ref().unwrap())
                .unwrap()
                .contains(OFlags::NONBLOCK),
            "raw-mode setup must preserve the separately opened nonblocking descriptor"
        );

        let host_size = HostSize {
            rows: 24,
            columns: 80,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let watchdog_stop = Arc::clone(&stop);
        let (cancel_tx, cancel_rx) = std::sync::mpsc::sync_channel(1);
        let (watchdog_done_tx, watchdog_done_rx) = std::sync::mpsc::channel();
        let watchdog = thread::spawn(move || {
            if cancel_rx.recv_timeout(Duration::from_secs(3)).is_err() {
                watchdog_stop.store(true, Ordering::Relaxed);
            }
            let _ = watchdog_done_tx.send(());
        });

        let started = Instant::now();
        let body = catch_unwind(AssertUnwindSafe(|| {
            run_loop(
                &mut resources,
                &mut decoder,
                &mut chunk,
                &mut frame,
                host_size,
                &stop,
            )
        }));
        let elapsed = started.elapsed();
        let _ = cancel_tx.send(());
        watchdog_done_rx
            .recv_timeout(Duration::from_millis(250))
            .expect("watchdog did not stop within its bounded join interval");
        watchdog.join().expect("watchdog thread must exit cleanly");
        let body = match body {
            Ok(body) => body,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        let observed = body
            .as_ref()
            .err()
            .map_or("successful loop return", |error| error.message());
        eprintln!(
            "unread_outer_pty_loop_result={observed}; elapsed_ms={}",
            elapsed.as_millis()
        );
        assert!(
            matches!(body, Err(ProbeError::OutputStalled)),
            "expected OutputStalled, got {observed} after {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_millis(1_500),
            "output stall was not detected within the one-second deadline: {elapsed:?}"
        );

        assert_eq!(resources.cleanup(), Err(ProbeError::HostRestore));
        let child_cleanup = resources.session.shutdown();
        assert!(
            resources.session.exit_status().is_some(),
            "bounded HUP/KILL cleanup must collect the child status; cached child result: {child_cleanup:?}"
        );

        let restored = tcgetattr(&observer).unwrap();
        assert_eq!(restored.input_modes, original.input_modes);
        assert_eq!(restored.output_modes, original.output_modes);
        assert_eq!(restored.control_modes, original.control_modes);
        assert_eq!(restored.local_modes, original.local_modes);
        assert_eq!(restored.input_speed(), original.input_speed());
        assert_eq!(restored.output_speed(), original.output_speed());
        assert_eq!(
            format!("{:?}", restored.special_codes),
            format!("{:?}", original.special_codes)
        );
        assert!(restored.local_modes.contains(LocalModes::ICANON));
    }
}
