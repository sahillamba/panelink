//! Synchronous terminal effects backed by one server-owned private PTY.
//!
//! This adapter is an experimental library boundary. It does not own a host
//! terminal, socket, client attachment, scheduler, durable launch record, or
//! process-recovery mechanism.

use crate::control_protocol::{
    Command, ReadSource, MAX_INPUT_TEXT_BYTES, MAX_READ_BYTES, MAX_READ_LINES,
};
use crate::protocol::Outcome;
use crate::response_protocol::RejectionCode;
use crate::server::{EffectSink, ReadResult};
use crate::terminal::{Key, PumpOutcome, ScreenSnapshot, TerminalSession, TerminalSize};
use std::thread;
use std::time::{Duration, Instant};

/// Maximum display text returned by a schema-1 read after its 90-byte response
/// metadata prefix is accounted for.
pub const MAX_READ_TEXT_BYTES: usize = MAX_READ_BYTES as usize - 90;

const INPUT_DRAIN_WINDOW: Duration = Duration::from_millis(100);
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// One real terminal session owned for the lifetime of a server-side target.
///
/// The child and PTY remain here when a logical client detaches. Input methods
/// never expose raw terminal output and never retain a caller's text outside
/// the core's fixed pending-input buffer.
pub struct PtyEffects {
    session: TerminalSession,
    revision: u64,
    last_pty_eof: bool,
    last_child_exited: bool,
    failed: bool,
}

impl std::fmt::Debug for PtyEffects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyEffects")
            .field("revision", &self.revision)
            .field("pty_eof", &self.last_pty_eof)
            .field("child_exited", &self.last_child_exited)
            .field("failed", &self.failed)
            .finish()
    }
}

impl PtyEffects {
    /// Take ownership of an already-created core session at initial revision 1.
    pub fn new(session: TerminalSession) -> Self {
        let last_pty_eof = session.pty_eof();
        let last_child_exited = session.exit_status().is_some();
        Self {
            session,
            revision: 1,
            last_pty_eof,
            last_child_exited,
            failed: false,
        }
    }

    /// Return the current nonzero screen/lifecycle revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    fn is_live(&self) -> bool {
        !self.failed && !self.last_pty_eof && !self.last_child_exited
    }

    fn observe_progress(&mut self, progress: PumpOutcome) -> bool {
        let lifecycle_changed = progress.pty_eof != self.last_pty_eof
            || progress.child_exited != self.last_child_exited;
        self.last_pty_eof = progress.pty_eof;
        self.last_child_exited = progress.child_exited;
        if progress.bytes_read == 0 && !lifecycle_changed {
            return true;
        }
        match self.revision.checked_add(1) {
            Some(next) => {
                self.revision = next;
                true
            }
            None => {
                self.fail_closed();
                false
            }
        }
    }

    fn fail_closed(&mut self) {
        self.failed = true;
        self.session.discard_pending_input();
    }

    fn read_screen(&self, max_text_bytes: usize, max_lines: u16) -> Result<ReadResult, Outcome> {
        if self.failed {
            return Err(Outcome::Unknown);
        }
        if max_text_bytes > MAX_READ_TEXT_BYTES || !(1..=MAX_READ_LINES).contains(&max_lines) {
            return Err(Outcome::Failed);
        }

        let snapshot = self.session.snapshot();
        let (text, truncated) = screen_text(snapshot, max_text_bytes, max_lines);
        Ok(ReadResult {
            text,
            revision: self.revision,
            truncated,
        })
    }

    fn apply_input(&mut self, command: Command<'_>) -> Outcome {
        if self.validate(command).is_err() {
            return Outcome::Failed;
        }

        match command {
            Command::Interrupt { .. } => match self.session.interrupt() {
                Ok(()) => Outcome::Applied,
                Err(_) => Outcome::Failed,
            },
            Command::Text { text, .. } => self.enqueue_and_drain(text, false, false),
            Command::Submit { text, .. } => self.enqueue_and_drain(text, true, false),
            Command::Paste { text, .. } => self.enqueue_and_drain(text, false, true),
            Command::Read { .. } => Outcome::Failed,
        }
    }

    fn enqueue_and_drain(&mut self, text: &str, submit: bool, paste: bool) -> Outcome {
        let pending_before = self.session.pending_input_bytes();
        if pending_before != 0 {
            return Outcome::Failed;
        }

        let enqueued = if paste {
            self.session.paste(text)
        } else {
            self.session.text(text, submit)
        };
        if enqueued.is_err() {
            self.session.discard_pending_input();
            return Outcome::Failed;
        }
        let request_bytes = self
            .session
            .pending_input_bytes()
            .saturating_sub(pending_before);
        self.drain_request(request_bytes)
    }

    /// Write exactly one already-queued request within the bounded window.
    /// Any unwritten suffix is discarded and never replayed.
    fn drain_request(&mut self, request_bytes: usize) -> Outcome {
        if request_bytes == 0 {
            self.session.discard_pending_input();
            return Outcome::Applied;
        }

        let deadline = Instant::now() + INPUT_DRAIN_WINDOW;
        let mut delivered = 0_usize;
        while delivered < request_bytes {
            let now = Instant::now();
            if now >= deadline {
                break;
            }

            match self.session.pump() {
                Ok(progress) => {
                    if !self.observe_progress(progress) {
                        self.session.discard_pending_input();
                        return Outcome::Unknown;
                    }
                    delivered = delivered.saturating_add(progress.bytes_written);
                    delivered = delivered.min(request_bytes);
                    if progress.child_exited || progress.pty_eof {
                        break;
                    }
                    if progress.bytes_written == 0 {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if !remaining.is_zero() {
                            thread::sleep(INPUT_POLL_INTERVAL.min(remaining));
                        }
                    }
                }
                Err(_) => {
                    self.fail_closed();
                    break;
                }
            }
        }

        // No bytes or callback replies may remain queued across an owner change.
        self.session.discard_pending_input();
        if delivered == request_bytes {
            Outcome::Applied
        } else if delivered != 0 {
            Outcome::Partial
        } else {
            // The core reports successful writes exactly. A zero-byte timeout
            // therefore establishes that no request byte reached the child.
            Outcome::Failed
        }
    }
}

impl EffectSink for PtyEffects {
    fn screen_revision(&self) -> Option<u64> {
        if self.failed {
            None
        } else {
            Some(self.revision)
        }
    }

    fn validate(&self, command: Command<'_>) -> Result<(), RejectionCode> {
        match command {
            Command::Read {
                source,
                max_bytes,
                max_lines,
            } => {
                if source == ReadSource::LiveScrollback {
                    return Err(RejectionCode::Unsupported);
                }
                if max_bytes > MAX_READ_BYTES || !(1..=MAX_READ_LINES).contains(&max_lines) {
                    return Err(RejectionCode::InvalidRequest);
                }
                if self.failed {
                    return Err(RejectionCode::Unsupported);
                }
                Ok(())
            }
            Command::Text { text, .. }
            | Command::Submit { text, .. }
            | Command::Paste { text, .. } => {
                if text.len() > MAX_INPUT_TEXT_BYTES
                    || (matches!(command, Command::Text { .. } | Command::Submit { .. })
                        && text.chars().any(char::is_control))
                {
                    return Err(RejectionCode::InvalidRequest);
                }
                self.validate_input_capacity()
            }
            Command::Interrupt { .. } => self.validate_input_capacity(),
        }
    }

    fn read(
        &mut self,
        source: ReadSource,
        max_text_bytes: usize,
        max_lines: u16,
    ) -> Result<ReadResult, Outcome> {
        match source {
            ReadSource::Screen => self.read_screen(max_text_bytes, max_lines),
            ReadSource::LiveScrollback => Err(Outcome::Failed),
        }
    }

    fn input(&mut self, command: Command<'_>) -> Outcome {
        self.apply_input(command)
    }

    fn poll(&mut self) -> Result<(), Outcome> {
        if self.failed {
            return Err(Outcome::Unknown);
        }
        match self.session.pump() {
            Ok(progress) => {
                if self.observe_progress(progress) {
                    Ok(())
                } else {
                    Err(Outcome::Unknown)
                }
            }
            Err(_) => {
                self.fail_closed();
                Err(Outcome::Unknown)
            }
        }
    }

    fn child_exited(&self) -> bool {
        // A terminal-adapter failure does not prove that its child has exited.
        self.session.exit_status().is_some()
    }
}

impl PtyEffects {
    /// Apply one absolute PTY and screen-engine size with one bounded pump.
    ///
    /// Sizes outside the terminal-core bounds fail before any effect. The
    /// applying pump makes ordinary output progress and never enqueues caller
    /// input. `Err(Outcome::Unknown)` means the PTY may or may not have
    /// accepted the size; a pump error fails this adapter closed, as `poll`
    /// does. A successful size change advances the screen revision.
    pub fn resize(&mut self, rows: u16, columns: u16) -> Result<(), Outcome> {
        if self.failed {
            return Err(Outcome::Unknown);
        }
        let size = TerminalSize::new(rows, columns).map_err(|_| Outcome::Failed)?;
        if self.session.size() == size {
            return Ok(());
        }
        self.session
            .request_resize(size)
            .map_err(|_| Outcome::Failed)?;
        match self.session.pump() {
            Ok(progress) => {
                if !self.observe_progress(progress) {
                    return Err(Outcome::Unknown);
                }
            }
            Err(_) => {
                self.fail_closed();
                return Err(Outcome::Unknown);
            }
        }
        if self.session.size() != size {
            return Err(Outcome::Unknown);
        }
        match self.revision.checked_add(1) {
            Some(next) => {
                self.revision = next;
                Ok(())
            }
            None => {
                self.fail_closed();
                Err(Outcome::Unknown)
            }
        }
    }

    /// Borrow the current sanitized, styled visible screen.
    ///
    /// The snapshot corresponds to [`Self::revision`] at the time of the call.
    pub fn screen(&self) -> &ScreenSnapshot {
        self.session.snapshot()
    }

    /// Check that typed keys could be queued now, without any effect.
    pub fn validate_keys(&self) -> Result<(), RejectionCode> {
        self.validate_input_capacity()
    }

    /// Queue and synchronously deliver one bounded sequence of typed keys.
    ///
    /// Keys go through the core's typed encoder only, never as raw host
    /// escape bytes. Like text input, the queue must be empty first; any
    /// suffix not written within the bounded drain window is discarded and
    /// reported as `Partial`/`Failed`, never retried.
    pub fn keys(&mut self, keys: impl IntoIterator<Item = Key>) -> Outcome {
        if self.validate_input_capacity().is_err() {
            return Outcome::Failed;
        }
        for key in keys {
            if self.session.key(key).is_err() {
                self.session.discard_pending_input();
                return Outcome::Failed;
            }
        }
        let request_bytes = self.session.pending_input_bytes();
        self.drain_request(request_bytes)
    }

    fn validate_input_capacity(&self) -> Result<(), RejectionCode> {
        if !self.is_live() {
            return Err(RejectionCode::Unsupported);
        }
        if self.session.pending_input_bytes() != 0 {
            return Err(RejectionCode::Capacity);
        }
        Ok(())
    }
}

fn screen_text(snapshot: &ScreenSnapshot, max_text_bytes: usize, max_lines: u16) -> (String, bool) {
    let mut text = String::with_capacity(max_text_bytes);
    let rows = snapshot.rows();
    let line_limit = rows.min(max_lines);
    let truncated = rows > line_limit;

    for row in 0..line_limit {
        if row != 0 && !push_char(&mut text, '\n', max_text_bytes) {
            return (text, true);
        }
        for column in 0..snapshot.columns() {
            let Some(cell) = snapshot.cell(row, column) else {
                if !push_char(&mut text, ' ', max_text_bytes) {
                    return (text, true);
                }
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let cell_text = cell.text();
            if cell_text.is_empty() {
                if !push_char(&mut text, ' ', max_text_bytes) {
                    return (text, true);
                }
                continue;
            }
            for scalar in cell_text.chars() {
                let safe = if scalar.is_control() {
                    '\u{fffd}'
                } else {
                    scalar
                };
                if !push_char(&mut text, safe, max_text_bytes) {
                    return (text, true);
                }
            }
        }
    }

    (text, truncated)
}

fn push_char(text: &mut String, scalar: char, budget: usize) -> bool {
    if scalar.len_utf8() > budget.saturating_sub(text.len()) {
        return false;
    }
    text.push(scalar);
    true
}
