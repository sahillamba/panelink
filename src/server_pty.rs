//! Synchronous terminal effects backed by one server-owned private PTY.
//!
//! This adapter is an experimental library boundary. It does not own a host
//! terminal, socket, client attachment, scheduler, durable launch record, or
//! process-recovery mechanism.

use crate::close_protocol::StopOutcome;
use crate::control_protocol::TargetRef;
use crate::control_protocol::{
    Command, ReadSource, MAX_INPUT_TEXT_BYTES, MAX_READ_BYTES, MAX_READ_LINES,
};
use crate::history_rows;
use crate::history_store::HistoryStore;
use crate::protocol::Outcome;
use crate::response_protocol::RejectionCode;
use crate::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use crate::server::{EffectSink, ReadResult};
use crate::terminal::{Key, PumpOutcome, ScreenSnapshot, TerminalSession, TerminalSize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const CAPTURE_INTERVAL: Duration = Duration::from_secs(5);

struct Capture {
    store: Arc<HistoryStore>,
    failure: Arc<AtomicBool>,
    target: TargetRef,
    last_revision: Option<u64>,
    last_buffer: Option<ScreenBuffer>,
    last_attempt: Option<Instant>,
    failed: bool,
    history_cursor: vt100::HistoryCursor,
    end_pending: bool,
}

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
    output_sequence: u64,
    last_pty_eof: bool,
    last_child_exited: bool,
    failed: bool,
    stopped: bool,
    exit_recorded: bool,
    capture: Option<Capture>,
}

impl std::fmt::Debug for PtyEffects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyEffects")
            .field("revision", &self.revision)
            .field("pty_eof", &self.last_pty_eof)
            .field("child_exited", &self.last_child_exited)
            .field("failed", &self.failed)
            .field("stopped", &self.stopped)
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
            output_sequence: 0,
            last_pty_eof,
            last_child_exited,
            failed: false,
            stopped: false,
            exit_recorded: false,
            capture: None,
        }
    }

    /// Return the current nonzero screen/lifecycle revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Install a server-owned checkpoint writer for this exact session.
    /// Failure to save a screen never interrupts the PTY or input.
    pub fn enable_capture(
        &mut self,
        store: Arc<HistoryStore>,
        failure: Arc<AtomicBool>,
        target: TargetRef,
    ) {
        let history_cursor =
            self.session
                .history_screen()
                .history_cursor()
                .unwrap_or(vt100::HistoryCursor {
                    epoch: 1,
                    ordinal: 0,
                });
        self.capture = Some(Capture {
            store,
            failure,
            target,
            last_revision: None,
            last_buffer: None,
            last_attempt: None,
            failed: false,
            history_cursor,
            end_pending: false,
        });
        // The parser may already have scrolled during launch. Never silently
        // claim pre-attachment output was persisted as a continuous segment.
        if let Some(capture) = &mut self.capture {
            if capture
                .store
                .begin_history(target, history_cursor.epoch, history_cursor.ordinal != 0)
                .is_err()
            {
                capture.failed = true;
                capture.failure.store(true, Ordering::Relaxed);
            }
        }
        if history_cursor.ordinal != 0 {
            if let Some(capture) = &mut self.capture {
                capture.history_cursor.ordinal = 0;
            }
        }
        self.capture_screen(true);
        self.capture_history();
    }

    /// Whether a checkpoint write has failed since capture was installed.
    pub fn capture_failed(&self) -> bool {
        self.capture.as_ref().is_some_and(|capture| capture.failed)
    }

    /// Save the latest visible screen on detach or orderly server shutdown.
    pub fn capture_now(&mut self) {
        self.capture_screen(true);
        self.capture_history();
    }

    /// Record a confirmed child exit as the start of history's age window.
    pub fn capture_exit(&mut self) -> bool {
        self.capture_now();
        let Some(capture) = &mut self.capture else {
            return true;
        };
        capture.end_pending = true;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok());
        if now.is_none_or(|now| capture.store.mark_ended(capture.target, now).is_err()) {
            capture.failed = true;
            capture.failure.store(true, Ordering::Relaxed);
            return false;
        }
        capture.end_pending = false;
        true
    }

    fn mark_final_output_gap(&mut self) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        if capture
            .store
            .mark_history(capture.target, capture.history_cursor.epoch, false)
            .is_err()
        {
            capture.failed = true;
            capture.failure.store(true, Ordering::Relaxed);
        }
    }

    fn capture_screen(&mut self, force: bool) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        let buffer = if self.session.snapshot().alternate_screen() {
            ScreenBuffer::Alternate
        } else {
            ScreenBuffer::Normal
        };
        if capture.last_revision == Some(self.revision)
            && capture.last_buffer == Some(buffer)
            && !capture.failed
        {
            return;
        }
        // Throttle failed writes too: otherwise a damaged destination or
        // removed directory would trigger an encode/fsync on every owner tick.
        if !force
            && capture.last_buffer == Some(buffer)
            && capture
                .last_attempt
                .is_some_and(|at| at.elapsed() < CAPTURE_INTERVAL)
        {
            return;
        }
        capture.last_attempt = Some(Instant::now());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok());
        let result = (|| {
            let metadata = CheckpointMetadata::new(
                capture.target,
                self.output_sequence,
                now.ok_or(())?,
                self.revision,
            )
            .map_err(|_| ())?;
            let screen = self.session.snapshot();
            let bytes = screen_checkpoint::encode(screen, &metadata).map_err(|_| ())?;
            capture
                .store
                .save(capture.target, buffer, &bytes)
                .map_err(|_| ())
        })();
        if result.is_ok() {
            capture.last_revision = Some(self.revision);
            capture.last_buffer = Some(buffer);
            capture.failed = false;
        } else {
            capture.failed = true;
            capture.failure.store(true, Ordering::Relaxed);
        }
    }

    fn capture_history(&mut self) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        let screen = self.session.history_screen();
        let latest = match screen.history_cursor() {
            Ok(cursor) => cursor,
            Err(_) => {
                capture.failed = true;
                capture.failure.store(true, Ordering::Relaxed);
                let _ =
                    capture
                        .store
                        .mark_history(capture.target, capture.history_cursor.epoch, true);
                return;
            }
        };
        let mut gap = false;
        let rows = match screen.history_after(capture.history_cursor) {
            Ok(rows) => rows.collect::<Vec<_>>(),
            Err(vt100::HistoryError::Reset | vt100::HistoryError::Gap) => {
                gap = true;
                let start = vt100::HistoryCursor {
                    epoch: latest.epoch,
                    // At most 1,024 rows survive the bounded engine ring.
                    // Starting here reads the retained suffix after a burst
                    // instead of silently discarding it on `Gap`.
                    ordinal: latest.ordinal.saturating_sub(1024),
                };
                match screen.history_after(start) {
                    Ok(rows) => rows.collect::<Vec<_>>(),
                    Err(_) => {
                        capture.failed = true;
                        capture.failure.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
            Err(_) => return,
        };
        if rows.is_empty() {
            if gap {
                if capture
                    .store
                    .mark_history(capture.target, latest.epoch, false)
                    .is_err()
                {
                    capture.failed = true;
                    capture.failure.store(true, Ordering::Relaxed);
                } else {
                    capture.history_cursor = latest;
                }
            }
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok())
            .unwrap_or(0);
        let records = rows
            .iter()
            .map(|row| history_rows::encode(capture.target, latest.epoch, *row, now))
            .collect::<std::io::Result<Vec<_>>>();
        let result =
            records.and_then(|records| capture.store.append_rows(capture.target, &records, gap));
        if result.is_err() {
            capture.failed = true;
            capture.failure.store(true, Ordering::Relaxed);
            return;
        }
        capture.history_cursor = latest;
        capture.failed = false;
    }

    fn is_live(&self) -> bool {
        !self.failed && !self.stopped && !self.last_pty_eof && !self.last_child_exited
    }

    fn observe_progress(&mut self, progress: PumpOutcome) -> bool {
        if progress.bytes_read != 0 {
            match self.output_sequence.checked_add(progress.bytes_read as u64) {
                Some(next) => self.output_sequence = next,
                None => {
                    // Terminal IO continues even when capture ordering is exhausted.
                    if let Some(capture) = &self.capture {
                        capture.failure.store(true, Ordering::Relaxed);
                    }
                    self.capture = None;
                }
            }
        }
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
        if self.stopped {
            // The PTY was closed by an explicit stop; nothing remains to pump.
            return Ok(());
        }
        match self.session.pump() {
            Ok(progress) => {
                if self.observe_progress(progress) {
                    self.capture_history();
                    self.capture_screen(progress.child_exited || progress.pty_eof);
                    if progress.child_exited && progress.pty_eof && !self.exit_recorded {
                        self.exit_recorded = self.capture_exit();
                    }
                    if self
                        .capture
                        .as_ref()
                        .is_some_and(|capture| capture.end_pending)
                        && !self.exit_recorded
                    {
                        self.exit_recorded = self.capture_exit();
                    }
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

    fn apply_size(&mut self, rows: u16, columns: u16) -> Result<(), Outcome> {
        self.resize(rows, columns)
    }

    /// Explicitly stop this child: HUP, bounded wait, KILL, bounded wait.
    ///
    /// Uses the core's cached cleanup, so it is idempotent and keeps the
    /// restore-first ordering of [`TerminalSession::shutdown`]. Pending input
    /// is discarded first and is never replayed. After this call the PTY is
    /// closed; later reads report `Unknown` and input is rejected.
    fn stop(&mut self) -> StopOutcome {
        self.capture_screen(true);
        self.capture_history();
        self.session.discard_pending_input();
        let already_exited = self.session.exit_status().is_some();
        let result = self.session.shutdown_with_final_output();
        // The PTY is closed: no further pump, input or resize. The retained
        // snapshot stays readable.
        self.stopped = true;
        self.last_child_exited = self.session.exit_status().is_some();
        self.capture_screen(true);
        self.capture_history();
        if !matches!(result, Ok(true)) || self.session.final_output_bytes() != 0 {
            // Rows may have entered and left the bounded engine ring during
            // teardown before the owner could durably accept them. The
            // terminal snapshot is saved, but completeness is unproved.
            self.mark_final_output_gap();
        }
        if self.last_child_exited && !self.exit_recorded {
            self.exit_recorded = self.capture_exit();
        }
        if result.is_err() || self.session.exit_status().is_none() {
            return StopOutcome::Unknown;
        }
        if !already_exited && self.session.kill_escalated() {
            StopOutcome::Escalated
        } else {
            StopOutcome::Exited
        }
    }
}

impl Drop for PtyEffects {
    fn drop(&mut self) {
        if self.capture.is_some() && !self.exit_recorded {
            // The owner may retire a pane as soon as it reaps the child,
            // before the PTY signals EOF. Drain only immediately available
            // output within a fixed work budget; never wait for a live child.
            if self.session.exit_status().is_some() && !self.session.pty_eof() && !self.stopped {
                for _ in 0..64 {
                    let Ok(progress) = self.session.pump() else {
                        break;
                    };
                    if !self.observe_progress(progress) {
                        break;
                    }
                    self.capture_history();
                    self.capture_screen(progress.pty_eof);
                    if progress.pty_eof || progress.bytes_read == 0 {
                        break;
                    }
                }
                if !self.session.pty_eof() {
                    self.mark_final_output_gap();
                }
            }
            self.capture_now();
            // Orderly owner teardown stops this child even if it was still
            // running when the client/server chose shutdown. Record the end
            // time only after the core confirms that cleanup reaped it.
            if self.session.exit_status().is_none() {
                let _ = self.session.shutdown();
            }
            if self.session.exit_status().is_some() {
                self.exit_recorded = self.capture_exit();
            }
        }
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
        if self.stopped {
            return Err(Outcome::Failed);
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
                self.capture_screen(false);
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
