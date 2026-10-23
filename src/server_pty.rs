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
use crate::history_capture::{
    lock_capture, CaptureObservation, CheckpointRecord, SharedHistoryCapture,
};
use crate::history_diagnostics::{HistoryDiagnostics, HistoryReason};
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
    store: Option<Arc<HistoryStore>>,
    coordinator: Option<SharedHistoryCapture>,
    failure: Arc<AtomicBool>,
    diagnostics: HistoryDiagnostics,
    target: TargetRef,
    // In async mode these are the latest encoded/observed candidate, not the
    // durable checkpoint; coordinator receipt state owns durability.
    last_revision: Option<u64>,
    last_buffer: Option<ScreenBuffer>,
    checkpoint_metadata: Option<(ScreenBuffer, CheckpointMetadata)>,
    #[cfg(test)]
    checkpoint_time_override: Option<u64>,
    last_attempt: Option<Instant>,
    failed: bool,
    history_cursor: vt100::HistoryCursor,
    end_pending: bool,
    end_reserved: bool,
    ordering_exhausted: bool,
    async_closed: bool,
}

impl Capture {
    fn record_failure(&self, reason: HistoryReason) {
        self.diagnostics.record(reason);
        self.failure.store(true, Ordering::Release);
    }

    fn fail(&mut self, reason: HistoryReason) {
        self.failed = true;
        self.record_failure(reason);
    }
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
    closing: bool,
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
            .field("closing", &self.closing)
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
            closing: false,
            exit_recorded: false,
            capture: None,
        }
    }

    /// Return the current nonzero screen/lifecycle revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Return a confirmed process exit code or signal, if the child was reaped.
    pub(crate) fn exit_status(&self) -> Option<crate::multi_runtime::PaneExit> {
        use std::os::unix::process::ExitStatusExt;

        self.session.exit_status().and_then(|status| {
            status
                .code()
                .map(crate::multi_runtime::PaneExit::Code)
                .or_else(|| status.signal().map(crate::multi_runtime::PaneExit::Signal))
        })
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
            store: Some(store),
            coordinator: None,
            failure,
            diagnostics: HistoryDiagnostics::new(),
            target,
            last_revision: None,
            last_buffer: None,
            checkpoint_metadata: None,
            #[cfg(test)]
            checkpoint_time_override: None,
            last_attempt: None,
            failed: false,
            history_cursor,
            end_pending: false,
            end_reserved: false,
            ordering_exhausted: false,
            async_closed: false,
        });
        // The parser may already have scrolled during launch. Never silently
        // claim pre-attachment output was persisted as a continuous segment.
        if let Some(capture) = &mut self.capture {
            if capture
                .store
                .as_ref()
                .expect("synchronous capture store")
                .begin_history(target, history_cursor.epoch, history_cursor.ordinal != 0)
                .is_err()
            {
                capture.fail(HistoryReason::CaptureSetupFailed);
            }
        }
        if history_cursor.ordinal != 0 {
            if let Some(capture) = &mut self.capture {
                capture.history_cursor.ordinal = 0;
            }
        }
        self.capture_observation(true, false);
    }

    /// Install the asynchronous coordinator used by every live runtime path.
    /// Registration failure disables only capture; the PTY remains usable.
    pub(crate) fn enable_async_capture(
        &mut self,
        coordinator: SharedHistoryCapture,
        failure: Arc<AtomicBool>,
        target: TargetRef,
    ) {
        // Live runtime capture must never fall back to a retained synchronous
        // store if registration or cursor sampling fails.
        self.capture = None;
        let diagnostics = lock_capture(&coordinator).diagnostics();
        let initial = self.session.history_screen().history_cursor();
        let Some(initial) = initial.ok() else {
            diagnostics.record(HistoryReason::CaptureInvalid);
            failure.store(true, Ordering::Release);
            return;
        };
        let registered = lock_capture(&coordinator).register_target(target, initial);
        if !registered {
            failure.store(true, Ordering::Release);
            return;
        }
        self.capture = Some(Capture {
            store: None,
            coordinator: Some(coordinator),
            failure,
            diagnostics,
            target,
            last_revision: None,
            last_buffer: None,
            checkpoint_metadata: None,
            #[cfg(test)]
            checkpoint_time_override: None,
            last_attempt: None,
            failed: false,
            history_cursor: vt100::HistoryCursor {
                epoch: initial.epoch,
                ordinal: 0,
            },
            end_pending: false,
            end_reserved: false,
            ordering_exhausted: false,
            async_closed: false,
        });
        self.capture_observation(true, false);
    }

    /// Whether this target or the shared asynchronous writer has failed.
    pub fn capture_failed(&self) -> bool {
        self.capture.as_ref().is_some_and(|capture| {
            capture.failed
                || (capture.coordinator.is_some() && capture.failure.load(Ordering::Acquire))
        })
    }

    /// Save the latest visible screen on detach or orderly server shutdown.
    pub fn capture_now(&mut self) {
        self.capture_observation(true, false);
    }

    /// Reserve terminal history work. Async success means the final batch was
    /// reserved/submitted; only its matching writer receipt proves durability.
    pub fn capture_exit(&mut self) -> bool {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.end_reserved || capture.async_closed)
        {
            return true;
        }
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.coordinator.is_some())
        {
            let Some(observation) = self.capture_observation_value(true, true) else {
                if let Some(capture) = &self.capture {
                    capture.record_failure(HistoryReason::CaptureInvalid);
                }
                return false;
            };
            let now = self.session.exit_status().and_then(|_| unix_time_ms());
            let Some(capture) = &self.capture else {
                return true;
            };
            if capture.async_closed {
                return true;
            }
            let target = capture.target;
            let coordinator =
                Arc::clone(capture.coordinator.as_ref().expect("async capture present"));
            let accepted = lock_capture(&coordinator).finish_target(target, observation, now);
            if !accepted {
                if let Some(capture) = &mut self.capture {
                    capture.fail(HistoryReason::CaptureInvalid);
                }
                return false;
            }
            if let Some(capture) = &mut self.capture {
                capture.end_reserved = true;
                capture.end_pending = false;
                if now.is_none() {
                    capture.failed = true;
                }
            }
            return true;
        }
        self.capture_now();
        let Some(capture) = &mut self.capture else {
            return true;
        };
        capture.end_pending = true;
        let Some(now) = unix_time_ms() else {
            capture.fail(HistoryReason::UnknownEndTime);
            return false;
        };
        if capture
            .store
            .as_ref()
            .is_none_or(|store| store.mark_ended(capture.target, now).is_err())
        {
            capture.fail(HistoryReason::StorageError);
            return false;
        }
        capture.end_pending = false;
        true
    }

    fn mark_final_output_gap(&mut self) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        if capture.async_closed {
            return;
        }
        if let Some(coordinator) = &capture.coordinator {
            let epoch = self
                .session
                .history_screen()
                .history_cursor()
                .map_or(capture.history_cursor.epoch, |cursor| cursor.epoch);
            let through = self
                .session
                .history_screen()
                .history_cursor()
                .map_or(capture.history_cursor.ordinal, |cursor| cursor.ordinal);
            lock_capture(coordinator).mark_gap(capture.target, epoch, through);
            return;
        }
        if capture.store.as_ref().is_none_or(|store| {
            store
                .mark_history(capture.target, capture.history_cursor.epoch, false)
                .is_err()
        }) {
            capture.fail(HistoryReason::StorageError);
        }
    }

    fn capture_observation(&mut self, force: bool, final_capture: bool) {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.end_reserved || capture.async_closed)
        {
            return;
        }
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.coordinator.is_some())
        {
            let Some(observation) = self.capture_observation_value(force, final_capture) else {
                return;
            };
            if let Some(capture) = &self.capture {
                if !capture.async_closed {
                    if let Some(coordinator) = &capture.coordinator {
                        lock_capture(coordinator).observe(capture.target, observation);
                    }
                }
            }
        } else {
            self.capture_screen_sync(force);
            self.capture_history_sync();
        }
    }

    fn capture_observation_value(
        &mut self,
        force: bool,
        final_capture: bool,
    ) -> Option<CaptureObservation> {
        let (target, submitted_cursor, regular_pending, can_capture) = {
            let capture = self.capture.as_ref()?;
            let coordinator = capture.coordinator.as_ref()?.clone();
            if capture.async_closed {
                return None;
            }
            let guard = lock_capture(&coordinator);
            let submitted = guard.submitted_cursor(capture.target)?;
            let pending = guard.has_regular_pending(capture.target);
            let can_capture = guard.can_capture(capture.target);
            drop(guard);
            (capture.target, submitted, pending, can_capture)
        };

        if !can_capture {
            if !final_capture {
                return None;
            }
            let cursor = self
                .session
                .history_screen()
                .history_cursor()
                .unwrap_or(submitted_cursor);
            return Some(CaptureObservation {
                cursor,
                gap: true,
                rows: Vec::new(),
                screens: Vec::new(),
            });
        }

        let screen = self.session.history_screen();
        let latest = match screen.history_cursor() {
            Ok(cursor) => cursor,
            Err(_) => {
                if let Some(capture) = &self.capture {
                    capture.record_failure(HistoryReason::CaptureInvalid);
                }
                return Some(CaptureObservation {
                    cursor: submitted_cursor,
                    gap: true,
                    rows: Vec::new(),
                    screens: Vec::new(),
                });
            }
        };
        let mut gap = false;
        let mut rows = Vec::new();
        if final_capture || !regular_pending {
            let after = match screen.history_after(submitted_cursor) {
                Ok(rows) => rows.collect::<Vec<_>>(),
                Err(vt100::HistoryError::Reset | vt100::HistoryError::Gap) => {
                    gap = true;
                    let start = vt100::HistoryCursor {
                        epoch: latest.epoch,
                        ordinal: latest.ordinal.saturating_sub(1024),
                    };
                    match screen.history_after(start) {
                        Ok(rows) => rows.collect::<Vec<_>>(),
                        Err(_) => {
                            if let Some(capture) = &self.capture {
                                capture.record_failure(HistoryReason::CaptureInvalid);
                            }
                            Vec::new()
                        }
                    }
                }
                Err(_) => {
                    if let Some(capture) = &self.capture {
                        capture.record_failure(HistoryReason::CaptureInvalid);
                    }
                    gap = true;
                    Vec::new()
                }
            };
            let timestamp = unix_time_ms();
            if let Some(timestamp) = timestamp {
                rows = after
                    .iter()
                    .map(|row| history_rows::encode(target, latest.epoch, *row, timestamp))
                    .collect::<std::io::Result<Vec<_>>>()
                    .unwrap_or_else(|_| {
                        gap = true;
                        if let Some(capture) = &self.capture {
                            capture.record_failure(HistoryReason::CaptureInvalid);
                        }
                        Vec::new()
                    });
            } else if !after.is_empty() {
                gap = true;
                if let Some(capture) = &self.capture {
                    capture.record_failure(HistoryReason::CaptureInvalid);
                }
            }
        }

        let buffer = if self.session.snapshot().alternate_screen() {
            ScreenBuffer::Alternate
        } else {
            ScreenBuffer::Normal
        };
        let should_capture_screen = {
            let capture = self.capture.as_ref()?;
            let changed = !capture.ordering_exhausted
                && (capture.last_revision != Some(self.revision)
                    || capture.last_buffer != Some(buffer));
            let elapsed = capture
                .last_attempt
                .is_none_or(|at| at.elapsed() >= CAPTURE_INTERVAL);
            changed && (force || buffer != capture.last_buffer.unwrap_or(buffer) || elapsed)
        };
        let mut screens = Vec::new();
        if should_capture_screen {
            let capture = self.capture.as_mut()?;
            capture.last_attempt = Some(Instant::now());
            let metadata = capture
                .checkpoint_metadata
                .filter(|(saved_buffer, metadata)| {
                    *saved_buffer == buffer
                        && metadata.target() == target
                        && metadata.output_sequence() == self.output_sequence
                        && metadata.screen_revision() == self.revision
                })
                .map(|(_, metadata)| metadata)
                .or_else(|| {
                    let now = unix_time_ms()?;
                    CheckpointMetadata::new(target, self.output_sequence, now, self.revision).ok()
                });
            if let Some(metadata) = metadata {
                capture.checkpoint_metadata = Some((buffer, metadata));
                let encoded = screen_checkpoint::encode(self.session.snapshot(), &metadata);
                let record = encoded
                    .ok()
                    .and_then(|bytes| CheckpointRecord::new(target, buffer, metadata, bytes).ok());
                if let Some(record) = record {
                    screens.push(record);
                    capture.last_revision = Some(self.revision);
                    capture.last_buffer = Some(buffer);
                } else {
                    capture.fail(HistoryReason::CaptureInvalid);
                }
            } else {
                capture.fail(HistoryReason::CaptureInvalid);
            }
        }
        Some(CaptureObservation {
            cursor: latest,
            gap,
            rows,
            screens,
        })
    }

    fn capture_screen(&mut self, force: bool) {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.coordinator.is_some())
        {
            self.capture_observation(force, false);
        } else {
            self.capture_screen_sync(force);
        }
    }

    fn capture_screen_sync(&mut self, force: bool) {
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
        // Keep retries byte-identical while this observation is unchanged;
        // a new buffer, output sequence, or screen revision gets fresh metadata.
        #[cfg(test)]
        let time_override = capture.checkpoint_time_override;
        #[cfg(not(test))]
        let time_override = None;
        let metadata = capture
            .checkpoint_metadata
            .filter(|(saved_buffer, metadata)| {
                *saved_buffer == buffer
                    && metadata.target() == capture.target
                    && metadata.output_sequence() == self.output_sequence
                    && metadata.screen_revision() == self.revision
            })
            .map(|(_, metadata)| metadata)
            .or_else(|| {
                let now = time_override.or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|time| u64::try_from(time.as_millis()).ok())
                })?;
                CheckpointMetadata::new(capture.target, self.output_sequence, now, self.revision)
                    .ok()
            });
        let Some(metadata) = metadata else {
            capture.fail(HistoryReason::CaptureInvalid);
            return;
        };
        capture.checkpoint_metadata = Some((buffer, metadata));
        let screen = self.session.snapshot();
        let bytes = match screen_checkpoint::encode(screen, &metadata) {
            Ok(bytes) => bytes,
            Err(_) => {
                capture.fail(HistoryReason::CaptureInvalid);
                return;
            }
        };
        let Some(store) = capture.store.as_ref() else {
            capture.fail(HistoryReason::CaptureInvalid);
            return;
        };
        if store.save(capture.target, buffer, &bytes).is_err() {
            capture.fail(HistoryReason::StorageError);
        } else {
            capture.last_revision = Some(self.revision);
            capture.last_buffer = Some(buffer);
            capture.failed = false;
        }
    }

    #[cfg(test)]
    fn capture_history(&mut self) {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.coordinator.is_some())
        {
            self.capture_observation(false, false);
        } else {
            self.capture_history_sync();
        }
    }

    fn capture_history_sync(&mut self) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        let screen = self.session.history_screen();
        let latest = match screen.history_cursor() {
            Ok(cursor) => cursor,
            Err(_) => {
                capture.fail(HistoryReason::CaptureInvalid);
                if let Some(store) = &capture.store {
                    let _ = store.mark_history(capture.target, capture.history_cursor.epoch, true);
                }
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
                        capture.fail(HistoryReason::CaptureInvalid);
                        return;
                    }
                }
            }
            Err(_) => return,
        };
        if rows.is_empty() {
            if gap {
                if capture.store.as_ref().is_none_or(|store| {
                    store
                        .mark_history(capture.target, latest.epoch, false)
                        .is_err()
                }) {
                    capture.fail(HistoryReason::StorageError);
                } else {
                    capture.history_cursor = latest;
                }
            }
            return;
        }
        let Some(now) = unix_time_ms() else {
            capture.fail(HistoryReason::CaptureInvalid);
            let _ = capture
                .store
                .as_ref()
                .map(|store| store.mark_history(capture.target, latest.epoch, true));
            return;
        };
        let records = rows
            .iter()
            .map(|row| history_rows::encode(capture.target, latest.epoch, *row, now))
            .collect::<std::io::Result<Vec<_>>>();
        let records = match records {
            Ok(records) => records,
            Err(_) => {
                capture.fail(HistoryReason::CaptureInvalid);
                return;
            }
        };
        let Some(store) = capture.store.as_ref() else {
            capture.fail(HistoryReason::CaptureInvalid);
            return;
        };
        if store.append_rows(capture.target, &records, gap).is_err() {
            capture.fail(HistoryReason::StorageError);
            return;
        }
        capture.history_cursor = latest;
        capture.failed = false;
    }

    fn is_live(&self) -> bool {
        !self.failed && !self.stopped && !self.last_pty_eof && !self.last_child_exited
    }

    fn observe_progress(&mut self, progress: PumpOutcome) -> bool {
        let mut ordering_exhausted = false;
        if progress.bytes_read != 0 {
            match self.output_sequence.checked_add(progress.bytes_read as u64) {
                Some(next) => self.output_sequence = next,
                None => {
                    // Terminal IO continues even when capture ordering is exhausted.
                    if let Some(capture) = &mut self.capture {
                        capture.fail(HistoryReason::CaptureInvalid);
                        if capture.coordinator.is_some() {
                            capture.ordering_exhausted = true;
                            ordering_exhausted = true;
                        } else {
                            self.capture = None;
                        }
                    }
                }
            }
        }
        if ordering_exhausted {
            self.mark_final_output_gap();
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
                if let Some(capture) = &mut self.capture {
                    capture.fail(HistoryReason::CaptureInvalid);
                    capture.ordering_exhausted = true;
                }
                self.mark_final_output_gap();
                self.fail_closed();
                false
            }
        }
    }

    fn fail_closed(&mut self) {
        self.failed = true;
        self.session.discard_pending_input();
    }

    /// Start server teardown without consuming a per-pane wait window.
    pub(crate) fn prepare_driver_shutdown(&mut self) {
        let _ = <Self as EffectSink>::begin_stop(self);
    }

    /// Advance one shared-deadline server shutdown observation.
    pub(crate) fn advance_driver_shutdown(&mut self, kill: bool) -> bool {
        matches!(<Self as EffectSink>::advance_stop(self, kill), Ok(Some(_)))
    }

    pub(crate) fn driver_shutdown_complete(&self) -> bool {
        self.session.exit_status().is_some()
    }

    fn observe_shutdown_output(
        &mut self,
        prior_bytes: usize,
        prior_eof: bool,
        prior_child_exited: bool,
    ) -> usize {
        let total_bytes = self.session.final_output_bytes();
        let newly_parsed = total_bytes.saturating_sub(prior_bytes);
        let child_exited = self.session.exit_status().is_some();
        let pty_eof = self.session.pty_eof();
        let lifecycle_changed = child_exited != prior_child_exited || pty_eof != prior_eof;
        let mut ordering_exhausted = false;
        if newly_parsed != 0 {
            match u64::try_from(newly_parsed)
                .ok()
                .and_then(|count| self.output_sequence.checked_add(count))
            {
                Some(next) => self.output_sequence = next,
                None => {
                    if let Some(capture) = &mut self.capture {
                        capture.fail(HistoryReason::CaptureInvalid);
                        capture.ordering_exhausted = true;
                        ordering_exhausted = true;
                    }
                }
            }
        }
        if newly_parsed != 0 || lifecycle_changed {
            match self.revision.checked_add(1) {
                Some(next) => self.revision = next,
                None => {
                    if let Some(capture) = &mut self.capture {
                        capture.fail(HistoryReason::CaptureInvalid);
                        capture.ordering_exhausted = true;
                        ordering_exhausted = true;
                    }
                }
            }
        }
        if ordering_exhausted {
            self.mark_final_output_gap();
        }
        self.last_child_exited = child_exited;
        self.last_pty_eof = pty_eof;
        newly_parsed
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
    fn process_identity(&self) -> Option<crate::process_metadata::ProcessIdentity> {
        self.session.process_identity()
    }

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
                if self.closing || self.stopped {
                    return Err(RejectionCode::Unsupported);
                }
                self.validate_input_capacity()
            }
            Command::Interrupt { .. } => {
                if self.closing || self.stopped {
                    Err(RejectionCode::Unsupported)
                } else {
                    self.validate_input_capacity()
                }
            }
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
        if self.closing || self.stopped {
            Outcome::Failed
        } else {
            self.apply_input(command)
        }
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
                    self.capture_observation(progress.child_exited || progress.pty_eof, false);
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
        if self.closing || self.stopped {
            Err(Outcome::Failed)
        } else {
            self.resize(rows, columns)
        }
    }

    fn begin_stop(&mut self) -> Result<Option<StopOutcome>, StopOutcome> {
        if self.closing {
            return Err(StopOutcome::Unknown);
        }
        self.capture_observation(true, false);
        let prior_final_bytes = self.session.final_output_bytes();
        let prior_eof = self.session.pty_eof();
        let prior_child_exited = self.session.exit_status().is_some();
        self.closing = true;
        match self.session.begin_nonblocking_shutdown() {
            Ok(true) => self
                .finish_reaped_close(prior_final_bytes, prior_eof, prior_child_exited)
                .map(Some),
            Ok(false) => Ok(None),
            Err(_) => Err(StopOutcome::Unknown),
        }
    }

    fn advance_stop(&mut self, kill: bool) -> Result<Option<StopOutcome>, StopOutcome> {
        let prior_final_bytes = self.session.final_output_bytes();
        let prior_eof = self.session.pty_eof();
        let prior_child_exited = self.session.exit_status().is_some();
        if self
            .session
            .poll_shutdown_exit()
            .map_err(|_| StopOutcome::Unknown)?
        {
            return self
                .finish_reaped_close(prior_final_bytes, prior_eof, prior_child_exited)
                .map(Some);
        }
        if kill {
            self.session
                .escalate_nonblocking_shutdown()
                .map_err(|_| StopOutcome::Unknown)?;
        }
        Ok(None)
    }

    /// Explicitly stop this child: HUP, bounded wait, KILL, bounded wait.
    ///
    /// Uses the core's cached cleanup, so it is idempotent and keeps the
    /// restore-first ordering of [`TerminalSession::shutdown`]. Pending input
    /// is discarded first and is never replayed. After this call the PTY is
    /// closed; later reads report `Unknown` and input is rejected.
    fn stop(&mut self) -> StopOutcome {
        self.capture_observation(true, false);
        self.session.discard_pending_input();
        self.closing = true;
        let already_exited = self.session.exit_status().is_some();
        let prior_final_bytes = self.session.final_output_bytes();
        let prior_eof = self.session.pty_eof();
        let prior_child_exited = already_exited;
        let result = self.session.shutdown_with_final_output();
        // The PTY is closed: no further pump, input or resize. The retained
        // snapshot stays readable.
        self.stopped = true;
        let newly_parsed =
            self.observe_shutdown_output(prior_final_bytes, prior_eof, prior_child_exited);
        if !matches!(result, Ok(true)) || newly_parsed != 0 {
            // Rows may have entered and left the bounded engine ring during
            // teardown before the owner could durably accept them. The
            // terminal snapshot is saved, but completeness is unproved.
            self.mark_final_output_gap();
        }
        if self.last_child_exited && !self.exit_recorded {
            self.exit_recorded = self.capture_exit();
        } else {
            self.capture_observation(true, false);
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

impl PtyEffects {
    fn finish_reaped_close(
        &mut self,
        prior_final_bytes: usize,
        prior_eof: bool,
        prior_child_exited: bool,
    ) -> Result<StopOutcome, StopOutcome> {
        let result = self
            .session
            .finish_reaped_shutdown()
            .map_err(|_| StopOutcome::Unknown)?;
        self.stopped = true;
        let newly_parsed =
            self.observe_shutdown_output(prior_final_bytes, prior_eof, prior_child_exited);
        if !result || newly_parsed != 0 {
            self.mark_final_output_gap();
        }
        if self.last_child_exited && !self.exit_recorded {
            self.exit_recorded = self.capture_exit();
        } else {
            self.capture_observation(true, false);
        }
        Ok(if self.session.kill_escalated() {
            StopOutcome::Escalated
        } else {
            StopOutcome::Exited
        })
    }
}

fn unix_time_ms() -> Option<u64> {
    let milliseconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let timestamp = u64::try_from(milliseconds).ok()?;
    (timestamp != 0).then_some(timestamp)
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
                    self.capture_observation(progress.pty_eof, false);
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
            if self.session.exit_status().is_none() && !self.closing {
                let _ = <Self as EffectSink>::stop(self);
            }
            if self.session.exit_status().is_some() {
                self.exit_recorded = self.capture_exit();
            } else if self.closing {
                self.mark_final_output_gap();
                if let Some(capture) = &mut self.capture {
                    capture.async_closed = true;
                }
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
        if self.failed || self.closing {
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
        if self.closing || !self.is_live() {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::history_store::TestFaultPoint;
    use crate::launch::{Environment, ResolvedLaunch};
    use crate::terminal::{TerminalError, TerminalSession};
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after Unix epoch")
                .as_nanos();
            for suffix in 0..32 {
                let requested = std::env::temp_dir().join(format!(
                    "panelink-pty-history-unit-{}-{}-{suffix}",
                    std::process::id(),
                    stamp
                ));
                match fs::DirBuilder::new().mode(0o700).create(&requested) {
                    Ok(()) => return Self(fs::canonicalize(requested).unwrap()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create private PTY fixture: {error}"),
                }
            }
            panic!("could not allocate a unique private PTY fixture")
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn target(session: u64) -> TargetRef {
        TargetRef {
            generation: 73,
            workspace: 11,
            pane: 13,
            session,
            occupant_revision: 19,
        }
    }

    fn shell(script: &str, cwd: &Path) -> TerminalSession {
        let launch = ResolvedLaunch {
            requested_path: PathBuf::from("/bin/sh"),
            executable_path: PathBuf::from("/bin/sh"),
            working_directory: cwd.to_path_buf(),
        };
        let environment: Environment = [(OsString::from("TERM"), OsString::from("panelink-probe"))]
            .into_iter()
            .collect();
        TerminalSession::spawn(
            &launch,
            &[OsString::from("-c"), OsString::from(script)],
            &environment,
            TerminalSize::new(8, 80).unwrap(),
        )
        .expect("spawn a real fixture PTY")
    }

    fn pump_until_exit(effects: &mut PtyEffects) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let progress = effects.session.pump().expect("pump fixture PTY");
            assert!(effects.observe_progress(progress));
            effects.capture_history();
            effects.capture_screen(progress.child_exited || progress.pty_eof);
            if progress.child_exited && progress.pty_eof {
                return;
            }
            assert!(Instant::now() < deadline, "fixture child did not exit");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn visible_text(effects: &PtyEffects) -> String {
        let snapshot = effects.session.snapshot();
        let mut text = String::new();
        for row in 0..snapshot.rows() {
            for column in 0..snapshot.columns() {
                if let Some(cell) = snapshot.cell(row, column) {
                    if !cell.is_wide_continuation() {
                        text.push_str(cell.text());
                    }
                }
            }
            text.push('\n');
        }
        text
    }

    fn pump_until_screen_text(effects: &mut PtyEffects, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let progress = effects.session.pump().expect("pump fixture PTY");
            assert!(effects.observe_progress(progress));
            if visible_text(effects).contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "fixture output did not arrive");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn direct_resize_rejects_a_pane_after_close_begins() {
        let fixture = Fixture::new();
        let mut effects = PtyEffects::new(shell(
            "trap '' HUP; while :; do /bin/sleep 1; done",
            fixture.path(),
        ));
        assert!(matches!(EffectSink::begin_stop(&mut effects), Ok(None)));
        assert_eq!(effects.validate_keys(), Err(RejectionCode::Unsupported));
        assert_eq!(effects.keys([Key::Char('x')]), Outcome::Failed);
        assert_eq!(effects.session.pending_input_bytes(), 0);
        assert_eq!(effects.resize(9, 80), Err(Outcome::Unknown));
        assert_eq!(
            effects
                .session
                .request_resize(TerminalSize::new(9, 80).unwrap()),
            Err(TerminalError::SessionClosed)
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if matches!(EffectSink::advance_stop(&mut effects, true), Ok(Some(_))) {
                break;
            }
            assert!(Instant::now() < deadline, "fixture child should exit");
            thread::yield_now();
        }
    }

    #[test]
    fn capture_exit_retries_end_sync_and_prunes_from_first_end_time() {
        let fixture = Fixture::new();
        let history_path = fixture.path().join("history");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&history_path)
            .unwrap();
        let store = Arc::new(HistoryStore::open(&history_path).unwrap());
        let ended = target(17);
        let live = target(18);
        let recently_ended = target(19);
        store.begin_history(live, 1, false).unwrap();
        store.begin_history(recently_ended, 1, false).unwrap();

        let mut effects = PtyEffects::new(shell("printf 'done\\n'; exit 0", fixture.path()));
        let capture_failure = Arc::new(AtomicBool::new(false));
        effects.enable_capture(Arc::clone(&store), Arc::clone(&capture_failure), ended);
        pump_until_exit(&mut effects);
        effects.capture_now();
        assert!(!effects.capture_failed());

        store.inject_test_fault(TestFaultPoint::DirectorySync, 1);
        assert!(!effects.capture_exit());
        assert!(effects.capture.as_ref().unwrap().end_pending);
        assert!(effects.capture_failed());
        assert!(capture_failure.load(Ordering::Relaxed));
        let first_end = store.load_rows(ended).unwrap().unwrap().ended_at_unix_ms;
        assert_ne!(first_end, 0);

        store.inject_test_fault(TestFaultPoint::DirectorySync, 1);
        assert!(store.mark_ended(ended, first_end + 1).is_err());
        assert!(effects.capture.as_ref().unwrap().end_pending);
        assert_eq!(
            store.load_rows(ended).unwrap().unwrap().ended_at_unix_ms,
            first_end
        );
        store.inject_test_fault(TestFaultPoint::DirectorySync, 0);
        assert!(effects.capture_exit());
        assert!(!effects.capture.as_ref().unwrap().end_pending);
        assert!(!effects.capture_failed());
        assert_eq!(
            store.load_rows(ended).unwrap().unwrap().ended_at_unix_ms,
            first_end
        );
        effects.exit_recorded = true;
        drop(effects);

        store.mark_ended(recently_ended, first_end + 1).unwrap();
        assert_eq!(
            store
                .prune_expired(first_end + 30 * 24 * 60 * 60 * 1000)
                .unwrap(),
            1
        );
        assert!(store.load_rows(ended).unwrap().is_none());
        assert!(store.load_rows(live).unwrap().is_some());
        assert!(store.load_rows(recently_ended).unwrap().is_some());
    }

    #[test]
    fn checkpoint_retry_reuses_metadata_until_directory_sync_succeeds() {
        let fixture = Fixture::new();
        let history_path = fixture.path().join("history");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&history_path)
            .unwrap();
        let store = Arc::new(HistoryStore::open(&history_path).unwrap());
        let target = target(21);
        let mut effects = PtyEffects::new(shell(
            "printf 'CHECKPOINT-READY\\n'; exec /bin/sleep 30",
            fixture.path(),
        ));
        let capture_failure = Arc::new(AtomicBool::new(false));
        effects.enable_capture(Arc::clone(&store), Arc::clone(&capture_failure), target);
        pump_until_screen_text(&mut effects, "CHECKPOINT-READY");
        effects.capture.as_mut().unwrap().checkpoint_time_override = Some(111);

        let before = store
            .load(target, ScreenBuffer::Normal)
            .unwrap()
            .expect("initial checkpoint was saved");
        store.inject_test_fault(TestFaultPoint::DirectorySync, 2);
        effects.capture_screen(true);
        assert!(effects.capture_failed());
        assert!(capture_failure.load(Ordering::Relaxed));
        let post_rename = store
            .load(target, ScreenBuffer::Normal)
            .unwrap()
            .expect("renamed checkpoint remains visible after sync failure");
        assert_ne!(post_rename, before);
        let checkpoint_text = screen_checkpoint::decode(&post_rename)
            .unwrap()
            .cells()
            .map(|cell| cell.text().to_owned())
            .collect::<String>();
        assert!(checkpoint_text.contains("CHECKPOINT-READY"));
        assert_eq!(
            screen_checkpoint::decode(&post_rename)
                .unwrap()
                .metadata()
                .captured_at_unix_ms(),
            111
        );

        effects.capture.as_mut().unwrap().checkpoint_time_override = Some(222);
        effects.capture_screen(true);
        assert!(effects.capture_failed());
        assert_eq!(
            store.load(target, ScreenBuffer::Normal).unwrap(),
            Some(post_rename.clone())
        );
        effects.capture_screen(true);
        assert!(!effects.capture_failed());
        assert_eq!(
            store.load(target, ScreenBuffer::Normal).unwrap(),
            Some(post_rename)
        );
    }
}
