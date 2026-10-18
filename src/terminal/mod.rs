//! Experimental same-process PTY and terminal-engine core.
//!
//! This module owns one child, one PTY, and one bounded VT100 screen. It does
//! not provide host input IO, persistence, or server lifetime.

pub mod render;

/// Bounded parser-only host decoder; callers own raw input IO and restoration.
pub use crossterm_input as input;

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::Errno;
use rustix::process::{ioctl_tiocsctty, kill_process_group, setsid, Pid, Signal};
use rustix::termios::{tcsetwinsize, Winsize};
use rustix_openpty::openpty;
use vt100::{Callbacks, Color, Parser, Screen};

use crate::launch::{Environment, ResolvedLaunch};

const HISTORY_ROWS: usize = 1_024;
const MAX_READ_BYTES: usize = 16 * 1024;
const MAX_INPUT_BYTES: usize = 65_536;
const MAX_QUERY_REPLIES: usize = 64;
const MAX_QUERY_RESPONSE_BYTES: usize = 256;
const HUP_WINDOW: Duration = Duration::from_secs(1);
const KILL_WINDOW: Duration = Duration::from_secs(1);
const CLEANUP_POLL: Duration = Duration::from_millis(10);
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// A checked PTY size. Rows and columns cannot be set directly by callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    rows: u16,
    columns: u16,
}

impl TerminalSize {
    /// Construct a size within Panelink's prototype dimensions.
    pub fn new(rows: u16, columns: u16) -> Result<Self, TerminalError> {
        if !(5..=80).contains(&rows) || !(20..=240).contains(&columns) {
            return Err(TerminalError::InvalidSize);
        }
        Ok(Self { rows, columns })
    }

    /// Number of visible rows.
    pub const fn rows(self) -> u16 {
        self.rows
    }

    /// Number of visible columns.
    pub const fn columns(self) -> u16 {
        self.columns
    }

    fn winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

/// Typed input accepted by the core. Host escape streams are never accepted.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Key {
    /// A non-control Unicode scalar.
    Char(char),
    /// Enter/return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Interrupt byte (normally interpreted by the child PTY line discipline).
    Interrupt,
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(_) => f.write_str("Char(<redacted>)"),
            Self::Enter => f.write_str("Enter"),
            Self::Tab => f.write_str("Tab"),
            Self::Backspace => f.write_str("Backspace"),
            Self::Escape => f.write_str("Escape"),
            Self::Interrupt => f.write_str("Interrupt"),
            Self::Up => f.write_str("Up"),
            Self::Down => f.write_str("Down"),
            Self::Left => f.write_str("Left"),
            Self::Right => f.write_str("Right"),
            Self::Home => f.write_str("Home"),
            Self::End => f.write_str("End"),
        }
    }
}

/// Stable, redacted errors from the terminal core.
#[derive(Clone, Eq, PartialEq)]
pub enum TerminalError {
    /// The requested rows or columns are outside the accepted range.
    InvalidSize,
    /// A resolved launch value failed local validation.
    InvalidLaunch,
    /// An argument contains a NUL byte.
    InvalidArgument,
    /// The explicit environment is invalid or does not contain the owned TERM.
    InvalidEnvironment,
    /// The PTY spawn lock was poisoned.
    SpawnLockPoisoned,
    /// A printable-character key contained a control scalar.
    InvalidKey,
    /// The encoded input would exceed the fixed pending-input queue.
    InputQueueFull,
    /// Input is disabled because the child exited or input previously failed.
    InputDisabled,
    /// The session was explicitly shut down and its PTY is closed.
    SessionClosed,
    /// A system operation failed; the label and kind contain no caller input.
    Io {
        /// Stable operation label.
        operation: &'static str,
        /// Operating-system error category.
        kind: io::ErrorKind,
    },
    /// The child was not reaped within both bounded cleanup windows.
    ShutdownTimedOut,
}

impl fmt::Debug for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => f.write_str("terminal dimensions are outside the accepted range"),
            Self::InvalidLaunch => f.write_str("resolved launch failed validation"),
            Self::InvalidArgument => f.write_str("launch argument contains an invalid byte"),
            Self::InvalidEnvironment => f.write_str("explicit child environment failed validation"),
            Self::SpawnLockPoisoned => f.write_str("PTY spawn lock is poisoned"),
            Self::InvalidKey => f.write_str("printable key contains a control character"),
            Self::InputQueueFull => f.write_str("pending input queue is full"),
            Self::InputDisabled => f.write_str("child input is disabled"),
            Self::SessionClosed => f.write_str("terminal session is closed"),
            Self::Io { operation, kind } => write!(f, "{operation} failed ({kind:?})"),
            Self::ShutdownTimedOut => f.write_str("child cleanup exceeded its bounded windows"),
        }
    }
}

impl std::error::Error for TerminalError {}

fn io_error(operation: &'static str, error: impl Into<io::Error>) -> TerminalError {
    TerminalError::Io {
        operation,
        kind: error.into().kind(),
    }
}

/// One sanitized visible cell retained by a [`ScreenSnapshot`].
///
/// Text uses fixed storage. Upstream cells contain at most 22 UTF-8 bytes and
/// at most nine scalars; replacing every scalar by U+FFFD needs at most 27.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SnapshotCell {
    text: [u8; 32],
    text_len: u8,
    wide: bool,
    wide_continuation: bool,
    foreground: Color,
    background: Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl fmt::Debug for SnapshotCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotCell")
            .field("text", &"<redacted>")
            .field("wide", &self.wide)
            .field("wide_continuation", &self.wide_continuation)
            .field("foreground", &self.foreground)
            .field("background", &self.background)
            .finish_non_exhaustive()
    }
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self {
            text: [0; 32],
            text_len: 0,
            wide: false,
            wide_continuation: false,
            foreground: Color::Default,
            background: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

impl SnapshotCell {
    /// Sanitized UTF-8 text for this cell, possibly empty for a blank cell.
    pub fn text(&self) -> &str {
        // Text is written only from valid scalar encodings in `set_from`.
        std::str::from_utf8(&self.text[..usize::from(self.text_len)])
            .expect("snapshot text is valid UTF-8")
    }

    /// Whether the cell is the first half of a wide character.
    pub const fn is_wide(&self) -> bool {
        self.wide
    }

    /// Whether the cell continues the preceding wide character.
    pub const fn is_wide_continuation(&self) -> bool {
        self.wide_continuation
    }

    /// Foreground color.
    pub const fn foreground(&self) -> Color {
        self.foreground
    }

    /// Background color.
    pub const fn background(&self) -> Color {
        self.background
    }

    /// Bold attribute.
    pub const fn bold(&self) -> bool {
        self.bold
    }

    /// Dim attribute.
    pub const fn dim(&self) -> bool {
        self.dim
    }

    /// Italic attribute.
    pub const fn italic(&self) -> bool {
        self.italic
    }

    /// Underline attribute.
    pub const fn underline(&self) -> bool {
        self.underline
    }

    /// Inverse attribute.
    pub const fn inverse(&self) -> bool {
        self.inverse
    }

    fn set_from(&mut self, cell: &vt100::Cell) {
        self.text.fill(0);
        self.text_len = 0;
        for scalar in cell.contents().chars() {
            let safe = if unsafe_for_host(scalar) {
                '\u{fffd}'
            } else {
                scalar
            };
            let mut encoded = [0_u8; 4];
            let bytes = safe.encode_utf8(&mut encoded).as_bytes();
            let start = usize::from(self.text_len);
            let end = start + bytes.len();
            if end <= self.text.len() {
                self.text[start..end].copy_from_slice(bytes);
                self.text_len = u8::try_from(end).unwrap_or(0);
            }
        }
        self.wide = cell.is_wide();
        self.wide_continuation = cell.is_wide_continuation();
        self.foreground = cell.fgcolor();
        self.background = cell.bgcolor();
        self.bold = cell.bold();
        self.dim = cell.dim();
        self.italic = cell.italic();
        self.underline = cell.underline();
        self.inverse = cell.inverse();
    }
}

fn unsafe_for_host(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{061c}' | '\u{200e}' | '\u{200f}'
        )
        || matches!(c as u32, 0x202a..=0x202e | 0x2066..=0x206f)
}

fn encode_key(key: Key, application_cursor: bool) -> Result<Vec<u8>, TerminalError> {
    let encoded = match key {
        Key::Char(c) if !c.is_control() => {
            let mut bytes = [0; 4];
            c.encode_utf8(&mut bytes).as_bytes().to_vec()
        }
        Key::Char(_) => return Err(TerminalError::InvalidKey),
        Key::Enter => vec![b'\r'],
        Key::Tab => vec![b'\t'],
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::Interrupt => vec![0x03],
        Key::Up => {
            if application_cursor {
                b"\x1bOA".to_vec()
            } else {
                b"\x1b[A".to_vec()
            }
        }
        Key::Down => {
            if application_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        Key::Right => {
            if application_cursor {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }
        }
        Key::Left => {
            if application_cursor {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }
        }
        Key::Home => {
            if application_cursor {
                b"\x1bOH".to_vec()
            } else {
                b"\x1b[H".to_vec()
            }
        }
        Key::End => {
            if application_cursor {
                b"\x1bOF".to_vec()
            } else {
                b"\x1b[F".to_vec()
            }
        }
    };
    Ok(encoded)
}

/// A bounded, sanitized view of the visible terminal screen.
pub struct ScreenSnapshot {
    rows: u16,
    columns: u16,
    cursor_row: u16,
    cursor_column: u16,
    alternate_screen: bool,
    cursor_hidden: bool,
    application_cursor: bool,
    bracketed_paste: bool,
    cells: Vec<SnapshotCell>,
}

impl fmt::Debug for ScreenSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenSnapshot")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("cursor_row", &self.cursor_row)
            .field("cursor_column", &self.cursor_column)
            .field("alternate_screen", &self.alternate_screen)
            .field("application_cursor", &self.application_cursor)
            .field("bracketed_paste", &self.bracketed_paste)
            .field("cell_count", &self.cells.len())
            .field("text", &"<redacted>")
            .finish()
    }
}

impl ScreenSnapshot {
    fn new(size: TerminalSize) -> Self {
        Self::try_new(size).expect("bounded snapshot allocation")
    }

    /// Allocate a blank snapshot with room for every supported viewport.
    ///
    /// Callers can reserve render caches before starting a child, then update
    /// them with [`Self::copy_from`] without allocating during host IO.
    pub fn try_new(size: TerminalSize) -> Result<Self, TerminalError> {
        let mut cells = Vec::new();
        cells.try_reserve_exact(80 * 240).map_err(|_| {
            io_error(
                "snapshot allocation",
                io::Error::from(io::ErrorKind::OutOfMemory),
            )
        })?;
        cells.resize_with(
            usize::from(size.rows) * usize::from(size.columns),
            SnapshotCell::default,
        );
        Ok(Self {
            rows: size.rows,
            columns: size.columns,
            cursor_row: 0,
            cursor_column: 0,
            alternate_screen: false,
            cursor_hidden: false,
            application_cursor: false,
            bracketed_paste: false,
            cells,
        })
    }

    /// Replace this snapshot using its preallocated, bounded cell storage.
    /// Snapshot text remains volatile and is never included in diagnostics.
    pub fn copy_from(&mut self, source: &Self) {
        self.rows = source.rows;
        self.columns = source.columns;
        self.cursor_row = source.cursor_row;
        self.cursor_column = source.cursor_column;
        self.alternate_screen = source.alternate_screen;
        self.cursor_hidden = source.cursor_hidden;
        self.application_cursor = source.application_cursor;
        self.bracketed_paste = source.bracketed_paste;
        self.cells
            .resize_with(source.cells.len(), SnapshotCell::default);
        self.cells.copy_from_slice(&source.cells);
    }

    fn refresh(&mut self, screen: &Screen, size: TerminalSize) {
        self.rows = size.rows;
        self.columns = size.columns;
        let count = usize::from(size.rows) * usize::from(size.columns);
        self.cells.resize_with(count, SnapshotCell::default);
        let (row, col) = screen.cursor_position();
        self.cursor_row = row.min(size.rows - 1);
        self.cursor_column = col.min(size.columns - 1);
        self.alternate_screen = screen.alternate_screen();
        self.cursor_hidden = screen.hide_cursor();
        self.application_cursor = screen.application_cursor();
        self.bracketed_paste = screen.bracketed_paste();
        for row in 0..size.rows {
            for col in 0..size.columns {
                if let Some(cell) = screen.cell(row, col) {
                    self.cells[usize::from(row) * usize::from(size.columns) + usize::from(col)]
                        .set_from(cell);
                }
            }
        }
    }

    /// Number of visible rows.
    pub const fn rows(&self) -> u16 {
        self.rows
    }
    /// Number of visible columns.
    pub const fn columns(&self) -> u16 {
        self.columns
    }
    /// Zero-based visible cursor row.
    pub const fn cursor_row(&self) -> u16 {
        self.cursor_row
    }
    /// Zero-based visible cursor column.
    pub const fn cursor_column(&self) -> u16 {
        self.cursor_column
    }
    /// Whether the engine is displaying its alternate screen.
    pub const fn alternate_screen(&self) -> bool {
        self.alternate_screen
    }
    /// Whether the child requested a hidden cursor.
    pub const fn cursor_hidden(&self) -> bool {
        self.cursor_hidden
    }
    /// Whether cursor keys use application-mode encoding.
    pub const fn application_cursor(&self) -> bool {
        self.application_cursor
    }
    /// Whether paste framing is currently enabled by the child.
    pub const fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }
    /// Visible cells in row-major order.
    pub fn cells(&self) -> &[SnapshotCell] {
        &self.cells
    }
    /// Cell at a zero-based visible position.
    pub fn cell(&self, row: u16, column: u16) -> Option<&SnapshotCell> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        self.cells
            .get(usize::from(row) * usize::from(self.columns) + usize::from(column))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadResult {
    Bytes(usize),
    WouldBlock,
    Eof,
    Deferred,
}

struct InputQueue {
    bytes: Vec<u8>,
    start: usize,
}

impl InputQueue {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAX_INPUT_BYTES),
            start: 0,
        }
    }
    fn len(&self) -> usize {
        self.bytes.len() - self.start
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn append_parts(&mut self, parts: &[&[u8]]) -> Result<(), TerminalError> {
        let added = parts
            .iter()
            .try_fold(0_usize, |sum, part| sum.checked_add(part.len()))
            .ok_or(TerminalError::InputQueueFull)?;
        if added > MAX_INPUT_BYTES - self.len() {
            return Err(TerminalError::InputQueueFull);
        }
        if self.start > 0 {
            self.bytes.copy_within(self.start.., 0);
            self.bytes.truncate(self.len());
            self.start = 0;
        }
        for part in parts {
            self.bytes.extend_from_slice(part);
        }
        Ok(())
    }

    fn front(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    fn consume(&mut self, count: usize) {
        self.start += count;
        if self.start == self.bytes.len() {
            self.bytes.clear();
            self.start = 0;
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.start = 0;
    }
}

fn write_queue_once<W>(queue: &mut InputQueue, mut writer: W) -> io::Result<usize>
where
    W: FnMut(&[u8]) -> io::Result<usize>,
{
    let pending = queue.front();
    if pending.is_empty() {
        return Ok(0);
    }
    let accepted = writer(&pending[..pending.len().min(MAX_READ_BYTES)])?;
    if accepted > 0 {
        queue.consume(accepted.min(pending.len()).min(MAX_READ_BYTES));
    }
    Ok(accepted)
}

fn write_attempt<W>(queue: &mut InputQueue, enabled: &mut bool, writer: W) -> io::Result<usize>
where
    W: FnMut(&[u8]) -> io::Result<usize>,
{
    if !*enabled || queue.is_empty() {
        return Ok(0);
    }
    match write_queue_once(queue, writer) {
        Ok(0) => {
            queue.clear();
            *enabled = false;
            Err(io::Error::from(io::ErrorKind::WriteZero))
        }
        Ok(count) => Ok(count),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || error.kind() == io::ErrorKind::Interrupted =>
        {
            Ok(0)
        }
        Err(error) => {
            queue.clear();
            *enabled = false;
            Err(error)
        }
    }
}

fn commit_resize<F>(
    screen: &mut Screen,
    accepted: &mut TerminalSize,
    requested: TerminalSize,
    operation: F,
) -> Result<(), TerminalError>
where
    F: FnOnce(Winsize) -> io::Result<()>,
{
    operation(requested.winsize()).map_err(|error| io_error("resize PTY", error))?;
    screen.set_size(requested.rows, requested.columns);
    *accepted = requested;
    Ok(())
}

fn enqueue_paste(queue: &mut InputQueue, text: &str, bracketed: bool) -> Result<(), TerminalError> {
    let open: &[u8] = if bracketed { b"\x1b[200~" } else { b"" };
    let close: &[u8] = if bracketed { b"\x1b[201~" } else { b"" };
    let total = text
        .len()
        .checked_add(open.len())
        .and_then(|n| n.checked_add(close.len()))
        .ok_or(TerminalError::InputQueueFull)?;
    if total > MAX_INPUT_BYTES - queue.len() {
        return Err(TerminalError::InputQueueFull);
    }
    queue.append_parts(&[open, text.as_bytes(), close])
}

struct CoreCallbacks {
    window_started: Instant,
    window_count: u8,
    reply_count: usize,
    replies: [[u8; MAX_QUERY_RESPONSE_BYTES]; MAX_QUERY_REPLIES],
    reply_lengths: [u16; MAX_QUERY_REPLIES],
}

impl CoreCallbacks {
    fn new() -> Self {
        Self {
            window_started: Instant::now(),
            window_count: 0,
            reply_count: 0,
            replies: [[0; MAX_QUERY_RESPONSE_BYTES]; MAX_QUERY_REPLIES],
            reply_lengths: [0; MAX_QUERY_REPLIES],
        }
    }

    fn begin_chunk(&mut self) {
        self.reply_count = 0;
    }

    fn queue(&mut self, response: &[u8]) {
        if response.len() > MAX_QUERY_RESPONSE_BYTES || self.reply_count == MAX_QUERY_REPLIES {
            return;
        }
        if self.window_started.elapsed() >= Duration::from_secs(1) {
            self.window_started = Instant::now();
            self.window_count = 0;
        }
        if self.window_count >= MAX_QUERY_REPLIES as u8 {
            return;
        }
        let slot = self.reply_count;
        self.replies[slot][..response.len()].copy_from_slice(response);
        self.reply_lengths[slot] = u16::try_from(response.len()).unwrap_or(0);
        self.reply_count += 1;
        self.window_count += 1;
    }

    fn queued(&self) -> impl Iterator<Item = &[u8]> {
        self.replies
            .iter()
            .zip(self.reply_lengths.iter())
            .take(self.reply_count)
            .map(|(bytes, len)| &bytes[..usize::from(*len)])
    }
}

impl Callbacks for CoreCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if i1.is_some() || i2.is_some() {
            return;
        }
        let query = match (c, params) {
            ('c', [param]) if *param == [0] => Some(0),
            ('n', [param]) if *param == [5] => Some(5),
            ('n', [param]) if *param == [6] => Some(6),
            ('t', [param]) if *param == [18] => Some(18),
            _ => None,
        };
        let Some(query) = query else {
            return;
        };
        let (rows, cols) = screen.size();
        let (row, col) = screen.cursor_position();
        let response = match query {
            0 => b"\x1b[?1c".to_vec(),
            5 => b"\x1b[0n".to_vec(),
            6 => format!("\x1b[{};{}R", row.min(rows - 1) + 1, col.min(cols - 1) + 1).into_bytes(),
            18 => format!("\x1b[8;{rows};{cols}t").into_bytes(),
            _ => return,
        };
        self.queue(&response);
    }

    fn paste_from_clipboard(&mut self, _: &mut Screen, _: &[u8]) {
        // Deny without a response. Even an empty OSC reply enters the child's
        // input stream and can become shell input when no application is
        // awaiting a clipboard response. Never access or forward host data.
    }
}

/// Progress returned from one nonblocking pump call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PumpOutcome {
    /// Bytes read from the child during this call, at most 16 KiB.
    pub bytes_read: usize,
    /// Bytes written to the child during this call, at most 16 KiB.
    pub bytes_written: usize,
    /// Whether the PTY has reached EOF.
    pub pty_eof: bool,
    /// Whether the child's exit status has been collected.
    pub child_exited: bool,
}

/// One explicitly launched child with one PTY and one bounded screen engine.
pub struct TerminalSession {
    master: Option<File>,
    child: Option<Child>,
    process_group: Pid,
    parser: Parser<CoreCallbacks>,
    size: TerminalSize,
    pending_resize: Option<TerminalSize>,
    input: InputQueue,
    input_enabled: bool,
    pty_eof: bool,
    exit_status: Option<ExitStatus>,
    snapshot: ScreenSnapshot,
    cleanup: Option<Result<(), TerminalError>>,
}

impl fmt::Debug for TerminalSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalSession")
            .field("size", &self.size)
            .field("pending_resize", &self.pending_resize)
            .field("pending_input_bytes", &self.input.len())
            .field("pty_eof", &self.pty_eof)
            .field("child_exited", &self.exit_status.is_some())
            .finish_non_exhaustive()
    }
}

impl TerminalSession {
    /// Spawn an already-resolved executable with exactly the supplied environment.
    pub fn spawn(
        launch: &ResolvedLaunch,
        arguments: &[OsString],
        environment: &Environment,
        size: TerminalSize,
    ) -> Result<Self, TerminalError> {
        validate_launch(launch, arguments, environment)?;
        // Allocate all retained engine state before a child exists, so a
        // constructor failure cannot leave a spawned child without an owner.
        let parser =
            Parser::new_with_callbacks(size.rows, size.columns, HISTORY_ROWS, CoreCallbacks::new());
        let mut snapshot = ScreenSnapshot::new(size);
        snapshot.refresh(parser.screen(), size);
        let input = InputQueue::new();
        let _spawn_guard = SPAWN_LOCK
            .lock()
            .map_err(|_| TerminalError::SpawnLockPoisoned)?;
        let pty = openpty(None, Some(&size.winsize())).map_err(|e| io_error("open PTY", e))?;
        let flags = fcntl_getfl(&pty.controller).map_err(|e| io_error("read PTY flags", e))?;
        fcntl_setfl(&pty.controller, flags | OFlags::NONBLOCK)
            .map_err(|e| io_error("set PTY nonblocking", e))?;

        let controlling_slave = pty
            .user
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| io_error("clone controlling PTY", e))?;
        let mut command = Command::new(&launch.executable_path);
        command
            .args(arguments)
            .env_clear()
            .current_dir(&launch.working_directory)
            .stdin(Stdio::from(clone_fd(&pty.user)?))
            .stdout(Stdio::from(clone_fd(&pty.user)?))
            .stderr(Stdio::from(clone_fd(&pty.user)?));
        for (name, value) in environment {
            command.env(name, value);
        }

        // Safety: this child hook calls only the reviewed session and controlling-tty
        // wrappers. It does no allocation, locking, logging, or environment access.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling_slave).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn().map_err(|e| io_error("spawn child", e))?;
        drop(command);
        drop(pty.user);
        let process_group = Pid::from_child(&child);
        Ok(Self {
            master: Some(File::from(pty.controller)),
            child: Some(child),
            process_group,
            parser,
            size,
            pending_resize: None,
            input,
            input_enabled: true,
            pty_eof: false,
            exit_status: None,
            snapshot,
            cleanup: None,
        })
    }

    /// Queue a typed key, without accepting raw host escape sequences.
    pub fn key(&mut self, key: Key) -> Result<(), TerminalError> {
        if !self.input_enabled {
            return Err(TerminalError::InputDisabled);
        }
        let encoded = encode_key(key, self.parser.screen().application_cursor())?;
        self.input.append_parts(&[&encoded])
    }

    /// Queue one caller-authorized paste atomically, optionally bracketed by engine mode.
    pub fn paste(&mut self, text: &str) -> Result<(), TerminalError> {
        if !self.input_enabled {
            return Err(TerminalError::InputDisabled);
        }
        let bracketed = self.parser.screen().bracketed_paste();
        enqueue_paste(&mut self.input, text, bracketed)
    }

    /// Queue literal printable text and an optional Enter as one bounded unit.
    pub fn text(&mut self, text: &str, submit: bool) -> Result<(), TerminalError> {
        if !self.input_enabled {
            return Err(TerminalError::InputDisabled);
        }
        if text.chars().any(char::is_control) {
            return Err(TerminalError::InvalidKey);
        }
        let suffix: &[u8] = if submit { b"\r" } else { b"" };
        self.input.append_parts(&[text.as_bytes(), suffix])
    }

    /// Signal this still-owned child group; never queue or replay the signal.
    pub fn interrupt(&mut self) -> Result<(), TerminalError> {
        if self.cleanup.is_some() || self.poll_exit()? {
            return Err(TerminalError::InputDisabled);
        }
        send_group_signal(self.process_group, Signal::INT)
    }

    /// Discard a not-yet-written suffix before releasing its input owner.
    ///
    /// Already-written bytes cannot be recalled. The server must record a
    /// Partial/Unknown outcome and must not replay this discarded suffix.
    pub fn discard_pending_input(&mut self) {
        self.input.clear();
    }

    /// Coalesce this request with the newest checked absolute size.
    pub fn request_resize(&mut self, size: TerminalSize) -> Result<(), TerminalError> {
        if self.cleanup.is_some() {
            return Err(TerminalError::SessionClosed);
        }
        self.pending_resize = Some(size);
        Ok(())
    }

    /// Make bounded nonblocking progress for this PTY and child.
    pub fn pump(&mut self) -> Result<PumpOutcome, TerminalError> {
        if self.cleanup.is_some() {
            return Err(TerminalError::SessionClosed);
        }
        let mut buffer = [0_u8; MAX_READ_BYTES];
        let read = if self.pty_eof {
            ReadResult::Eof
        } else {
            self.read_once(&mut buffer)?
        };
        let bytes_read = match read {
            ReadResult::Bytes(count) => {
                self.parser.callbacks_mut().begin_chunk();
                self.parser.process(&buffer[..count]);
                if self.input_enabled {
                    for reply in self.parser.callbacks().queued() {
                        let _ = self.input.append_parts(&[reply]);
                    }
                } else {
                    self.parser.callbacks_mut().begin_chunk();
                }
                count
            }
            ReadResult::WouldBlock | ReadResult::Deferred | ReadResult::Eof => 0,
        };
        if read == ReadResult::Eof {
            self.pty_eof = true;
            self.input.clear();
            self.input_enabled = false;
            self.parser.callbacks_mut().begin_chunk();
        }

        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_status = Some(status);
                    self.child = None;
                    self.input.clear();
                    self.input_enabled = false;
                    self.parser.callbacks_mut().begin_chunk();
                }
                Ok(None) => {}
                Err(error) => {
                    let reported = io_error("poll child", error);
                    let _ = self.shutdown();
                    return Err(reported);
                }
            }
        }
        let bytes_written = if self.input_enabled {
            self.write_once()?
        } else {
            0
        };
        if let Some(requested) = self.pending_resize.take() {
            let master = self.master.as_ref().ok_or(TerminalError::SessionClosed)?;
            commit_resize(
                self.parser.screen_mut(),
                &mut self.size,
                requested,
                |winsize| tcsetwinsize(master, winsize).map_err(io::Error::from),
            )?;
        }
        self.snapshot.refresh(self.parser.screen(), self.size);
        Ok(PumpOutcome {
            bytes_read,
            bytes_written,
            pty_eof: self.pty_eof,
            child_exited: self.exit_status.is_some(),
        })
    }

    fn read_once(
        &mut self,
        buffer: &mut [u8; MAX_READ_BYTES],
    ) -> Result<ReadResult, TerminalError> {
        let master = self.master.as_mut().ok_or(TerminalError::SessionClosed)?;
        match master.read(buffer) {
            Ok(0) => Ok(ReadResult::Eof),
            Ok(count) => Ok(ReadResult::Bytes(count)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(ReadResult::WouldBlock),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(ReadResult::Deferred),
            Err(error) if error.raw_os_error() == Some(Errno::IO.raw_os_error()) => {
                Ok(ReadResult::Eof)
            }
            Err(error) => {
                let reported = io_error("read PTY", error);
                let _ = self.shutdown();
                Err(reported)
            }
        }
    }

    fn write_once(&mut self) -> Result<usize, TerminalError> {
        let master = self.master.as_mut().ok_or(TerminalError::SessionClosed)?;
        match write_attempt(&mut self.input, &mut self.input_enabled, |bytes| {
            master.write(bytes)
        }) {
            Ok(count) => Ok(count),
            Err(error) => {
                let reported = io_error("write PTY", error);
                let _ = self.shutdown();
                Err(reported)
            }
        }
    }

    /// Current dimensions accepted by both PTY and screen engine.
    pub const fn size(&self) -> TerminalSize {
        self.size
    }
    /// Borrow the single retained sanitized snapshot.
    pub fn snapshot(&self) -> &ScreenSnapshot {
        &self.snapshot
    }
    /// Child exit status, once collected.
    pub const fn exit_status(&self) -> Option<ExitStatus> {
        self.exit_status
    }
    /// Whether PTY output reached EOF.
    pub const fn pty_eof(&self) -> bool {
        self.pty_eof
    }
    /// Pending encoded input byte count.
    pub fn pending_input_bytes(&self) -> usize {
        self.input.len()
    }
    /// Stop and reap the managed child within the two cleanup windows.
    pub fn shutdown(&mut self) -> Result<(), TerminalError> {
        if let Some(result) = &self.cleanup {
            return result.clone();
        }
        let result = self.cleanup_child();
        self.parser.callbacks_mut().begin_chunk();
        self.master.take();
        self.cleanup = Some(result.clone());
        result
    }

    fn cleanup_child(&mut self) -> Result<(), TerminalError> {
        self.input.clear();
        self.input_enabled = false;
        self.pending_resize = None;
        if self.child.is_none() {
            return Ok(());
        }
        if self.poll_exit()? {
            return Ok(());
        }
        let hup_error = send_group_signal(self.process_group, Signal::HUP).err();
        if self.wait_bounded(HUP_WINDOW)? {
            return Ok(());
        }
        if self.poll_exit()? {
            return Ok(());
        }
        let kill_error = send_group_signal(self.process_group, Signal::KILL).err();
        if self.wait_bounded(KILL_WINDOW)? {
            return Ok(());
        }
        if let Some(error) = kill_error {
            return Err(error);
        }
        if let Some(error) = hup_error {
            return Err(error);
        }
        Err(TerminalError::ShutdownTimedOut)
    }

    fn poll_exit(&mut self) -> Result<bool, TerminalError> {
        let Some(child) = &mut self.child else {
            return Ok(true);
        };
        match child.try_wait().map_err(|e| io_error("reap child", e))? {
            Some(status) => {
                self.exit_status = Some(status);
                self.child = None;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn wait_bounded(&mut self, duration: Duration) -> Result<bool, TerminalError> {
        let deadline = Instant::now() + duration;
        loop {
            if self.poll_exit()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(CLEANUP_POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn send_group_signal(group: Pid, signal: Signal) -> Result<(), TerminalError> {
    match kill_process_group(group, signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(error) => Err(io_error("signal child process group", error)),
    }
}

fn validate_launch(
    launch: &ResolvedLaunch,
    arguments: &[OsString],
    environment: &Environment,
) -> Result<(), TerminalError> {
    if !launch.requested_path.is_absolute()
        || !launch.executable_path.is_absolute()
        || !launch.working_directory.is_absolute()
        || launch.requested_path.as_os_str().as_bytes().contains(&0)
        || launch.executable_path.as_os_str().as_bytes().contains(&0)
        || launch.working_directory.as_os_str().as_bytes().contains(&0)
    {
        return Err(TerminalError::InvalidLaunch);
    }
    let executable =
        fs::metadata(&launch.executable_path).map_err(|e| io_error("validate executable", e))?;
    if !executable.is_file() {
        return Err(TerminalError::InvalidLaunch);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if executable.permissions().mode() & 0o111 == 0 {
            return Err(TerminalError::InvalidLaunch);
        }
    }
    if !fs::metadata(&launch.working_directory)
        .map_err(|e| io_error("validate working directory", e))?
        .is_dir()
    {
        return Err(TerminalError::InvalidLaunch);
    }
    if arguments.iter().any(|arg| arg.as_bytes().contains(&0)) {
        return Err(TerminalError::InvalidArgument);
    }
    let mut has_term = false;
    for (name, value) in environment {
        let key = name.as_bytes();
        if key.is_empty()
            || key.contains(&0)
            || key.contains(&b'=')
            || value.as_bytes().contains(&0)
        {
            return Err(TerminalError::InvalidEnvironment);
        }
        if key == b"TERM" {
            has_term = value.as_bytes() == b"panelink-probe";
            if !has_term {
                return Err(TerminalError::InvalidEnvironment);
            }
        }
    }
    if !has_term {
        return Err(TerminalError::InvalidEnvironment);
    }
    Ok(())
}

fn clone_fd(fd: &OwnedFd) -> Result<OwnedFd, TerminalError> {
    fd.as_fd()
        .try_clone_to_owned()
        .map_err(|e| io_error("clone child PTY", e))
}

#[cfg(test)]
mod unit_tests;
