//! One bounded FIFO writer for durable saved-history batches.
//!
//! This component owns its input after admission, serializes storage calls on
//! one worker, and reports durable completion separately from admission.
//! Timed-out shutdown requests cancellation between storage calls; an
//! in-progress call cannot be stopped, and an already executing final required
//! call may still commit afterward. A timed-out shutdown may also perform a
//! bounded metadata-only end finalization for accepted canceled batches.

use crate::control_protocol::TargetRef;
use crate::history_diagnostics::{HistoryDiagnostics, HistoryDrainOutcome, HistoryReason};
use crate::history_rows::{self, MAX_ROW_BYTES};
use crate::history_store::{HistoryStore, SavedHistory};
use crate::screen_checkpoint::{self, ScreenBuffer, MAX_SCREEN_CHECKPOINT_BYTES};
use crate::server_storage::{HistoryWriterGuard, ServerStorage};
use std::fmt;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Maximum number of accepted batches, including the worker's current batch.
pub const MAX_QUEUED_BATCHES: usize = 16;
/// Maximum encoded payload retained by accepted batches, including in-flight.
pub const MAX_QUEUED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
/// Maximum rows accepted in one batch, matching the persisted segment limit.
pub const MAX_BATCH_ROWS: usize = 1024;
/// Maximum payload that one batch can retain under its row and checkpoint caps.
pub const MAX_BATCH_PAYLOAD_BYTES: usize =
    MAX_BATCH_ROWS * MAX_ROW_BYTES + 2 * MAX_SCREEN_CHECKPOINT_BYTES;
/// Maximum time an explicit shutdown waits for the worker.
pub const MAX_SHUTDOWN_WAIT: Duration = Duration::from_millis(250);
/// Metadata-only slots reserved for registered capture targets.
pub(crate) const MAX_END_RESERVATIONS: usize = 8;

const RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(10);
const STATUS_PENDING: u8 = 0;
const STATUS_COMMITTED: u8 = 1;
const STATUS_FAILED: u8 = 2;
const STATUS_CANCELED: u8 = 3;

fn invalid_batch() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid history batch")
}

/// An owned, validated set of history operations for one target and epoch.
///
/// Encoded rows and checkpoints are boxed before the batch is retained, so a
/// caller's spare `Vec` capacity is not hidden from admission accounting.
pub struct HistoryBatch {
    target: TargetRef,
    epoch: u64,
    token: u64,
    gap: bool,
    rows: Option<Box<[Box<[u8]>]>>,
    screens: Box<[(ScreenBuffer, Box<[u8]>)]>,
    ended_at_unix_ms: Option<u64>,
    payload_bytes: usize,
}

impl fmt::Debug for HistoryBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HistoryBatch { <redacted> }")
    }
}

impl HistoryBatch {
    /// Validate and take ownership of one batch of sanitized rows/checkpoints.
    ///
    /// `token` is nonzero and must increase for each admitted batch on a
    /// writer. Rejected submissions do not consume their token.
    pub fn new(
        target: TargetRef,
        epoch: u64,
        token: u64,
        gap: bool,
        rows: Vec<Vec<u8>>,
        screens: Vec<Vec<u8>>,
        ended_at_unix_ms: Option<u64>,
    ) -> io::Result<Self> {
        if !valid_target(target)
            || epoch == 0
            || token == 0
            || rows.len() > MAX_BATCH_ROWS
            || screens.len() > 2
            || ended_at_unix_ms == Some(0)
        {
            return Err(invalid_batch());
        }

        let mut payload_bytes = 0_usize;
        let mut previous_ordinal = 0_u64;
        for record in &rows {
            let row = history_rows::decode(record).map_err(|_| invalid_batch())?;
            if row.target != target || row.epoch != epoch || row.ordinal <= previous_ordinal {
                return Err(invalid_batch());
            }
            previous_ordinal = row.ordinal;
            payload_bytes = payload_bytes
                .checked_add(record.len())
                .ok_or_else(invalid_batch)?;
        }

        let mut seen_normal = false;
        let mut seen_alternate = false;
        let mut screen_buffers = Vec::with_capacity(screens.len());
        for record in &screens {
            let checkpoint = screen_checkpoint::decode(record).map_err(|_| invalid_batch())?;
            if checkpoint.metadata().target() != target {
                return Err(invalid_batch());
            }
            let buffer = checkpoint.buffer();
            match buffer {
                ScreenBuffer::Normal if !seen_normal => seen_normal = true,
                ScreenBuffer::Alternate if !seen_alternate => seen_alternate = true,
                _ => return Err(invalid_batch()),
            }
            screen_buffers.push(buffer);
            payload_bytes = payload_bytes
                .checked_add(record.len())
                .ok_or_else(invalid_batch)?;
        }

        if payload_bytes > MAX_BATCH_PAYLOAD_BYTES {
            return Err(invalid_batch());
        }

        let boxed_rows = rows
            .into_iter()
            .map(Vec::into_boxed_slice)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let boxed_screens = screens
            .into_iter()
            .zip(screen_buffers)
            .map(|(screen, buffer)| (buffer, screen.into_boxed_slice()))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(Self {
            target,
            epoch,
            token,
            gap,
            rows: Some(boxed_rows),
            screens: boxed_screens,
            ended_at_unix_ms,
            payload_bytes,
        })
    }

    /// Keep the newest complete-row suffix that fits alongside every screen.
    ///
    /// The original batch must already have passed [`HistoryBatch::new`], so
    /// every row is validated before a prefix can be discarded. The encoded
    /// row and screen boxes are moved without cloning their bodies. Returns
    /// the retained batch, the number of removed rows, and whether any rows
    /// remain. `None` means the screens alone do not fit or no row can be
    /// retained with them.
    pub(crate) fn retain_newest_suffix_within(
        mut self,
        max_payload_bytes: usize,
    ) -> Option<(Self, usize, bool)> {
        let rows = self.rows.take()?;
        let row_bytes = rows
            .iter()
            .try_fold(0_usize, |total, row| total.checked_add(row.len()))?;
        let screen_bytes = self
            .screens
            .iter()
            .try_fold(0_usize, |total, (_, screen)| {
                total.checked_add(screen.len())
            })?;
        let full_bytes = row_bytes.checked_add(screen_bytes)?;
        debug_assert_eq!(self.payload_bytes, full_bytes);

        if full_bytes <= max_payload_bytes {
            let has_rows = !rows.is_empty();
            self.rows = Some(rows);
            return Some((self, 0, has_rows));
        }
        if screen_bytes > max_payload_bytes {
            return None;
        }

        let row_budget = max_payload_bytes - screen_bytes;
        let mut first_retained = rows.len();
        let mut retained_bytes = 0_usize;
        while first_retained > 0 {
            let next_bytes = rows[first_retained - 1].len();
            let Some(total) = retained_bytes.checked_add(next_bytes) else {
                break;
            };
            if total > row_budget {
                break;
            }
            retained_bytes = total;
            first_retained -= 1;
        }
        if !rows.is_empty() && first_retained == rows.len() {
            return None;
        }

        let removed_rows = first_retained;
        let mut rows = rows.into_vec();
        drop(rows.drain(..removed_rows));
        let has_rows = !rows.is_empty();
        self.rows = Some(rows.into_boxed_slice());
        self.payload_bytes = screen_bytes + retained_bytes;
        self.gap |= removed_rows > 0;
        Some((self, removed_rows, has_rows))
    }

    /// If checkpoints alone cannot fit, release both and salvage complete
    /// newest rows. The caller must record the discarded checkpoint loss.
    pub(crate) fn retain_rows_without_screens_within(
        mut self,
        max_payload_bytes: usize,
    ) -> Option<(Self, usize)> {
        if self.screens.is_empty() || self.rows.as_ref().is_none_or(|rows| rows.is_empty()) {
            return None;
        }
        self.screens = Vec::new().into_boxed_slice();
        self.payload_bytes = self.rows.as_ref()?.iter().map(|row| row.len()).sum();
        let (batch, removed, retained) = self.retain_newest_suffix_within(max_payload_bytes)?;
        retained.then_some((batch, removed))
    }

    pub(crate) fn screen_bytes(&self) -> usize {
        self.screens.iter().map(|(_, screen)| screen.len()).sum()
    }

    #[cfg(test)]
    pub(crate) fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
}

/// Exact capacity boundary that rejected a history batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityReason {
    /// The bounded accounting lock was already held by another operation.
    Contended,
    /// The accepted-batch count reached its fixed limit.
    Count,
    /// The encoded payload budget would be exceeded.
    Bytes,
}

/// Why a batch was rejected before ownership transferred to the worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitReason {
    /// A bounded admission check could not reserve capacity.
    Full,
    /// The writer is shutting down or its worker has stopped.
    Stopped,
    /// Earlier storage work failed and the writer is closed fail-safe.
    Failed,
    /// The batch token is not greater than the last admitted token.
    StaleToken,
}

impl SubmitReason {
    const fn message(self) -> &'static str {
        match self {
            Self::Full => "history writer capacity is full",
            Self::Stopped => "history writer is stopped",
            Self::Failed => "history writer has failed",
            Self::StaleToken => "history batch token is stale",
        }
    }
}

/// A pre-admission rejection that returns the exact caller-owned batch.
pub struct SubmitError {
    reason: SubmitReason,
    capacity_reason: Option<CapacityReason>,
    batch: Box<HistoryBatch>,
}

impl SubmitError {
    /// Fixed rejection reason; no batch contents are included.
    pub const fn reason(&self) -> SubmitReason {
        self.reason
    }

    /// Exact rejected capacity check, present only for [`SubmitReason::Full`].
    pub const fn capacity_reason(&self) -> Option<CapacityReason> {
        self.capacity_reason
    }

    /// Return the unchanged batch for retry or explicit disposal.
    pub fn into_batch(self) -> HistoryBatch {
        *self.batch
    }
}

impl fmt::Debug for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubmitError")
            .field("reason", &self.reason)
            .field("batch", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason.message())
    }
}

impl std::error::Error for SubmitError {}

/// Durable state of one admitted batch receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStatus {
    /// At least one storage operation remains incomplete.
    Pending,
    /// All required storage stages returned their durability acknowledgment.
    Committed,
    /// A storage stage exhausted retries or the worker failed.
    ///
    /// Earlier stages may already have durable effects; this is not rollback.
    Failed,
    /// Cancellation stopped this batch between storage operations.
    ///
    /// Earlier stages may already have durable effects. After an explicit
    /// timed-out shutdown, end metadata may be finalized separately; this
    /// receipt never advances a capture cursor and an in-progress filesystem
    /// call cannot be canceled by this status.
    Canceled,
}

/// Receipt for one admitted batch, stable even after its writer is dropped.
pub struct Receipt {
    target: TargetRef,
    epoch: u64,
    token: u64,
    status: Arc<AtomicU8>,
}

impl Receipt {
    /// Immutable identity of the target written by this batch.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Engine epoch identified by this batch.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Monotonic writer-local batch token.
    pub const fn token(&self) -> u64 {
        self.token
    }

    /// Read the completion state without waiting for storage work.
    pub fn poll(&mut self) -> CommitStatus {
        match self.status.load(Ordering::Acquire) {
            STATUS_COMMITTED => CommitStatus::Committed,
            STATUS_FAILED => CommitStatus::Failed,
            STATUS_CANCELED => CommitStatus::Canceled,
            _ => CommitStatus::Pending,
        }
    }
}

impl fmt::Debug for Receipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Receipt { <redacted> }")
    }
}

/// Result of a bounded explicit writer shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownOutcome {
    /// Every admitted batch reached its terminal status before the deadline.
    Drained,
    /// At least one admitted batch failed or the worker panicked.
    Failed,
    /// The wait budget expired; cancellation was requested between calls.
    ///
    /// This says work was incomplete at the deadline. An in-progress call is
    /// still allowed to finish and may durably commit the final required stage.
    TimedOut,
}

/// Bounded shutdown result.
pub struct ShutdownReport {
    /// Whether the queue drained, failed closed, or exceeded its wait budget.
    pub outcome: ShutdownOutcome,
}

impl fmt::Debug for ShutdownReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownReport")
            .field("outcome", &self.outcome)
            .finish()
    }
}

struct SharedState {
    accepting: bool,
    cancel: bool,
    finalize_canceled_ends: bool,
    finalize_reserved_ends: bool,
    failed_closed: bool,
    batches: usize,
    payload_bytes: usize,
    ends: Vec<EndReservation>,
}

#[derive(Clone, Copy)]
struct EndReservation {
    target: TargetRef,
    epoch: u64,
    at: Option<u64>,
    gap: bool,
    resolved: bool,
}

struct Shared {
    state: Mutex<SharedState>,
    had_storage_error: AtomicBool,
    diagnostics: HistoryDiagnostics,
}

impl Shared {
    fn reserve_end(&self, target: TargetRef, epoch: u64) -> bool {
        let mut state = self.lock();
        if !state.accepting {
            return false;
        }
        // Reuse slots whose end metadata already committed. Without this a
        // long-lived generation with repeated split/close would exhaust the
        // fixed table and refuse capture for later panes.
        if state.ends.len() >= MAX_END_RESERVATIONS {
            state.ends.retain(|end| !end.resolved);
        }
        if state.ends.len() >= MAX_END_RESERVATIONS {
            return false;
        }
        state.ends.push(EndReservation {
            target,
            epoch,
            at: None,
            gap: false,
            resolved: false,
        });
        true
    }

    fn record_end(&self, target: TargetRef, epoch: u64, at: Option<u64>, gap: bool) {
        let mut state = self.lock();
        if let Some(end) = state.ends.iter_mut().find(|end| end.target == target) {
            if epoch >= end.epoch {
                end.epoch = epoch;
                end.at = end.at.or(at);
                end.gap |= gap;
                end.resolved = false;
            }
        }
    }

    fn resolve_end(&self, target: TargetRef) {
        if let Some(end) = self.lock().ends.iter_mut().find(|end| end.target == target) {
            end.resolved = true;
        }
    }

    fn pending_ends(&self) -> Vec<EndReservation> {
        let state = self.lock();
        if !state.finalize_reserved_ends {
            return Vec::new();
        }
        state
            .ends
            .iter()
            .copied()
            .filter(|end| !end.resolved && (end.at.is_some() || end.gap))
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, SharedState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn try_lock(&self) -> Result<MutexGuard<'_, SharedState>, CapacityReason> {
        match self.state.try_lock() {
            Ok(state) => Ok(state),
            Err(TryLockError::WouldBlock) => Err(CapacityReason::Contended),
            Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        }
    }

    fn available_payload_bytes(&self) -> Option<usize> {
        let state = self.try_lock().ok()?;
        if !state.accepting || state.failed_closed || state.batches >= MAX_QUEUED_BATCHES {
            return None;
        }
        Some(MAX_QUEUED_PAYLOAD_BYTES.saturating_sub(state.payload_bytes))
    }

    fn reserve(&self, bytes: usize) -> Result<(), AdmissionError> {
        let mut state = self.try_lock().map_err(|capacity_reason| AdmissionError {
            reason: SubmitReason::Full,
            capacity_reason: Some(capacity_reason),
        })?;
        if state.failed_closed {
            return Err(AdmissionError {
                reason: SubmitReason::Failed,
                capacity_reason: None,
            });
        }
        if !state.accepting {
            return Err(AdmissionError {
                reason: SubmitReason::Stopped,
                capacity_reason: None,
            });
        }
        if state.batches >= MAX_QUEUED_BATCHES {
            return Err(AdmissionError {
                reason: SubmitReason::Full,
                capacity_reason: Some(CapacityReason::Count),
            });
        }
        if state
            .payload_bytes
            .checked_add(bytes)
            .is_none_or(|total| total > MAX_QUEUED_PAYLOAD_BYTES)
        {
            return Err(AdmissionError {
                reason: SubmitReason::Full,
                capacity_reason: Some(CapacityReason::Bytes),
            });
        }
        state.batches += 1;
        state.payload_bytes += bytes;
        Ok(())
    }

    fn release(&self, bytes: usize) {
        let mut state = self.lock();
        state.batches = state.batches.saturating_sub(1);
        state.payload_bytes = state.payload_bytes.saturating_sub(bytes);
    }

    fn stop_accepting(&self) {
        let mut state = self.lock();
        state.accepting = false;
        state.finalize_reserved_ends = true;
    }

    fn request_cancel(&self) {
        let mut state = self.lock();
        state.accepting = false;
        state.cancel = true;
        state.finalize_canceled_ends = false;
        state.finalize_reserved_ends = false;
    }

    fn request_timeout_cancel(&self) {
        let mut state = self.lock();
        state.accepting = false;
        state.cancel = true;
        state.finalize_canceled_ends = true;
    }

    fn canceled(&self) -> bool {
        self.lock().cancel
    }

    fn may_finalize_canceled_ends(&self) -> bool {
        let state = self.lock();
        state.cancel && state.finalize_canceled_ends && !state.failed_closed
    }

    fn failed_closed(&self) -> bool {
        self.lock().failed_closed
    }

    fn fail_closed(&self) {
        let mut state = self.lock();
        state.accepting = false;
        state.failed_closed = true;
    }

    fn storage_error(&self) {
        self.diagnostics.record(HistoryReason::StorageError);
        self.had_storage_error.store(true, Ordering::Release);
    }

    fn worker_failure(&self) {
        self.diagnostics.record(HistoryReason::WorkerFailure);
        // Preserve the existing broad failure signal consumed by the
        // coordinator, while keeping the reason bit distinct from a failed
        // storage stage.
        self.had_storage_error.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
struct AdmissionError {
    reason: SubmitReason,
    capacity_reason: Option<CapacityReason>,
}

struct Work {
    batch: Option<HistoryBatch>,
    receipt: Arc<AtomicU8>,
    shared: Arc<Shared>,
    finalized: bool,
}

impl Work {
    fn finish(mut self, status: CommitStatus) {
        let batch = self.batch.take().expect("work batch is present");
        let bytes = batch.payload_bytes;
        drop(batch);
        self.finalized = true;
        if status == CommitStatus::Canceled {
            self.shared.diagnostics.record(HistoryReason::WorkCanceled);
        }
        let encoded_status = match status {
            CommitStatus::Pending => STATUS_PENDING,
            CommitStatus::Committed => STATUS_COMMITTED,
            CommitStatus::Failed => STATUS_FAILED,
            CommitStatus::Canceled => STATUS_CANCELED,
        };
        self.shared.release(bytes);
        self.receipt.store(encoded_status, Ordering::Release);
    }

    fn reject(mut self) -> HistoryBatch {
        let batch = self.batch.take().expect("work batch is present");
        let bytes = batch.payload_bytes;
        self.finalized = true;
        self.shared.release(bytes);
        batch
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let bytes = self.batch.take().map_or(0, |batch| {
            let bytes = batch.payload_bytes;
            drop(batch);
            bytes
        });
        self.shared.fail_closed();
        self.shared.worker_failure();
        self.shared.release(bytes);
        self.receipt.store(STATUS_FAILED, Ordering::Release);
    }
}

/// One-producer, one-worker FIFO history writer.
pub struct HistoryWriter {
    sender: Option<SyncSender<Work>>,
    done: Receiver<ShutdownOutcome>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    last_token: u64,
    finalized: bool,
}

impl fmt::Debug for HistoryWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HistoryWriter { <redacted> }")
    }
}

impl HistoryWriter {
    /// Start one worker after verifying the store and claiming its server.
    ///
    /// The caller must supply the store belonging to this storage domain and
    /// stop any other direct mutation of that store while the writer lives.
    pub fn start(store: HistoryStore, storage: &ServerStorage) -> io::Result<Self> {
        Self::start_with_diagnostics(store, storage, HistoryDiagnostics::new())
    }

    /// Start the writer using a handle that may be retained before capture is
    /// enabled and observed after the writer or driver has been dropped.
    pub fn start_with_diagnostics(
        store: HistoryStore,
        storage: &ServerStorage,
        diagnostics: HistoryDiagnostics,
    ) -> io::Result<Self> {
        let result = storage
            .verify_history_store(&store)
            .and_then(|()| storage.claim_history_writer())
            .and_then(|writer_guard| {
                Self::spawn(
                    Box::new(StoreBackend { store }),
                    writer_guard,
                    diagnostics.clone(),
                )
            });
        if result.is_err() {
            diagnostics.record(HistoryReason::CaptureSetupFailed);
        }
        result
    }

    /// Return a shared clone of this writer's bounded diagnostic handle.
    pub fn diagnostics(&self) -> HistoryDiagnostics {
        self.shared.diagnostics.clone()
    }

    /// Observe remaining encoded-byte capacity without waiting or recording an
    /// admission diagnostic. The owner must not admit another batch between
    /// this snapshot and its single retry; worker activity can only free bytes.
    pub(crate) fn available_payload_bytes(&self) -> Option<usize> {
        self.shared.available_payload_bytes()
    }

    pub(crate) fn reserve_end(&self, target: TargetRef, epoch: u64) -> bool {
        self.shared.reserve_end(target, epoch)
    }

    pub(crate) fn record_end(&self, target: TargetRef, epoch: u64, at: Option<u64>, gap: bool) {
        self.shared.record_end(target, epoch, at, gap);
    }

    fn spawn(
        backend: Box<dyn HistoryBackend>,
        writer_guard: HistoryWriterGuard,
        diagnostics: HistoryDiagnostics,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_BATCHES);
        let (done_sender, done) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(SharedState {
                accepting: true,
                cancel: false,
                finalize_canceled_ends: false,
                finalize_reserved_ends: false,
                failed_closed: false,
                batches: 0,
                payload_bytes: 0,
                ends: Vec::with_capacity(MAX_END_RESERVATIONS),
            }),
            had_storage_error: AtomicBool::new(false),
            diagnostics,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("panelink-history-writer".to_owned())
            .spawn(move || {
                worker_entry(backend, writer_guard, receiver, worker_shared, done_sender);
            })
            .map_err(|_| io::Error::other("unable to start history writer"))?;

        Ok(Self {
            sender: Some(sender),
            done,
            worker: Some(worker),
            shared,
            last_token: 0,
            finalized: false,
        })
    }

    /// Admit without waiting for storage work or queue capacity.
    ///
    /// A rejection returns ownership of the complete batch. An accepted
    /// receipt means queued only; it does not mean data has been committed.
    pub fn try_submit(&mut self, batch: HistoryBatch) -> Result<Receipt, SubmitError> {
        if batch.token <= self.last_token {
            return Err(SubmitError {
                reason: SubmitReason::StaleToken,
                capacity_reason: None,
                batch: Box::new(batch),
            });
        }
        if let Err(error) = self.shared.reserve(batch.payload_bytes) {
            if let Some(capacity_reason) = error.capacity_reason {
                self.shared.diagnostics.record(match capacity_reason {
                    CapacityReason::Contended => HistoryReason::AdmissionContended,
                    CapacityReason::Count => HistoryReason::AdmissionCountFull,
                    CapacityReason::Bytes => HistoryReason::AdmissionBytesFull,
                });
            }
            return Err(SubmitError {
                reason: error.reason,
                capacity_reason: error.capacity_reason,
                batch: Box::new(batch),
            });
        }

        let status = Arc::new(AtomicU8::new(STATUS_PENDING));
        let receipt = Receipt {
            target: batch.target,
            epoch: batch.epoch,
            token: batch.token,
            status: Arc::clone(&status),
        };
        let work = Work {
            batch: Some(batch),
            receipt: status,
            shared: Arc::clone(&self.shared),
            finalized: false,
        };
        let Some(sender) = self.sender.as_ref() else {
            return Err(SubmitError {
                reason: SubmitReason::Stopped,
                capacity_reason: None,
                batch: Box::new(work.reject()),
            });
        };

        match sender.try_send(work) {
            Ok(()) => {
                self.last_token = receipt.token;
                Ok(receipt)
            }
            Err(TrySendError::Full(work)) => {
                self.shared
                    .diagnostics
                    .record(HistoryReason::AdmissionCountFull);
                Err(SubmitError {
                    reason: SubmitReason::Full,
                    capacity_reason: Some(CapacityReason::Count),
                    batch: Box::new(work.reject()),
                })
            }
            Err(TrySendError::Disconnected(work)) => {
                self.shared.fail_closed();
                self.shared.worker_failure();
                Err(SubmitError {
                    reason: SubmitReason::Failed,
                    capacity_reason: None,
                    batch: Box::new(work.reject()),
                })
            }
        }
    }

    /// True after any storage error, including an error recovered by retry.
    pub fn had_storage_error(&self) -> bool {
        self.shared.had_storage_error.load(Ordering::Acquire)
    }

    /// Stop admission and drain for at most `min(wait, 250 ms)`.
    ///
    /// A timeout requests cancellation between storage calls, then detaches.
    /// The worker may subsequently finalize known end metadata for accepted
    /// canceled batches. An active filesystem call cannot be canceled and the
    /// worker retains the duplicate singleton lock until it settles.
    pub fn shutdown(mut self, wait: Duration) -> ShutdownReport {
        self.shared.stop_accepting();
        self.sender.take();
        let budget = wait.min(MAX_SHUTDOWN_WAIT);
        let outcome = match self.done.recv_timeout(budget) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.shared.request_timeout_cancel();
                self.shared
                    .diagnostics
                    .record(HistoryReason::ShutdownTimedOut);
                ShutdownOutcome::TimedOut
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.shared.fail_closed();
                self.shared.request_cancel();
                self.shared.worker_failure();
                ShutdownOutcome::Failed
            }
        };
        self.shared
            .diagnostics
            .set_writer_drain_outcome(match outcome {
                ShutdownOutcome::Drained => HistoryDrainOutcome::Drained,
                ShutdownOutcome::Failed => HistoryDrainOutcome::Failed,
                ShutdownOutcome::TimedOut => HistoryDrainOutcome::TimedOut,
            });
        self.finalized = true;
        self.worker.take(); // Dropping the handle detaches any still-running worker.
        ShutdownReport { outcome }
    }

    #[cfg(test)]
    pub(crate) fn start_with_backend(
        backend: Box<dyn HistoryBackend>,
        storage: &ServerStorage,
    ) -> io::Result<Self> {
        Self::start_with_backend_and_diagnostics(backend, storage, HistoryDiagnostics::new())
    }

    #[cfg(test)]
    pub(crate) fn start_with_backend_and_diagnostics(
        backend: Box<dyn HistoryBackend>,
        storage: &ServerStorage,
        diagnostics: HistoryDiagnostics,
    ) -> io::Result<Self> {
        let result = storage
            .claim_history_writer()
            .and_then(|writer_guard| Self::spawn(backend, writer_guard, diagnostics.clone()));
        if result.is_err() {
            diagnostics.record(HistoryReason::CaptureSetupFailed);
        }
        result
    }
}

impl Drop for HistoryWriter {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.shared.request_cancel();
        self.sender.take();
        self.worker.take(); // Plain Drop never joins or waits for disk work.
    }
}

pub(crate) trait HistoryBackend: Send + 'static {
    fn load_rows(&mut self, target: TargetRef) -> io::Result<Option<SavedHistory>>;
    fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()>;
    fn mark_history(&mut self, target: TargetRef, epoch: u64, disabled: bool) -> io::Result<()>;
    fn append_rows(&mut self, target: TargetRef, rows: &[Vec<u8>], gap: bool) -> io::Result<()>;
    fn save_screen(
        &mut self,
        target: TargetRef,
        buffer: ScreenBuffer,
        screen: &[u8],
    ) -> io::Result<()>;
    fn mark_ended(&mut self, target: TargetRef, at_unix_ms: u64) -> io::Result<()>;
    fn finalize_incomplete_end(
        &mut self,
        _target: TargetRef,
        _epoch: u64,
        _at_unix_ms: u64,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "history backend does not support canceled end finalization",
        ))
    }
}

struct StoreBackend {
    store: HistoryStore,
}

impl HistoryBackend for StoreBackend {
    fn load_rows(&mut self, target: TargetRef) -> io::Result<Option<SavedHistory>> {
        self.store.load_rows(target)
    }

    fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
        self.store.begin_history(target, epoch, gap)
    }

    fn mark_history(&mut self, target: TargetRef, epoch: u64, disabled: bool) -> io::Result<()> {
        self.store.mark_history(target, epoch, disabled)
    }

    fn append_rows(&mut self, target: TargetRef, rows: &[Vec<u8>], gap: bool) -> io::Result<()> {
        self.store.append_rows(target, rows, gap)
    }

    fn save_screen(
        &mut self,
        target: TargetRef,
        buffer: ScreenBuffer,
        screen: &[u8],
    ) -> io::Result<()> {
        self.store.save(target, buffer, screen)
    }

    fn mark_ended(&mut self, target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
        self.store.mark_ended(target, at_unix_ms)
    }

    fn finalize_incomplete_end(
        &mut self,
        target: TargetRef,
        epoch: u64,
        at_unix_ms: u64,
    ) -> io::Result<()> {
        self.store
            .finalize_incomplete_end(target, epoch, at_unix_ms)
    }
}

fn worker_entry(
    mut backend: Box<dyn HistoryBackend>,
    writer_guard: HistoryWriterGuard,
    receiver: Receiver<Work>,
    shared: Arc<Shared>,
    done: SyncSender<ShutdownOutcome>,
) {
    let mut current = None;
    let result = catch_unwind(AssertUnwindSafe(|| {
        worker_loop(&mut *backend, &receiver, &shared, &mut current)
    }));
    let mut outcome = match result {
        Ok(outcome) => outcome,
        Err(_) => {
            shared.worker_failure();
            shared.fail_closed();
            if let Some(work) = current.take() {
                finish_work(work, CommitStatus::Failed);
            }
            fail_queued(&receiver);
            ShutdownOutcome::Failed
        }
    };
    if catch_unwind(AssertUnwindSafe(|| drop(backend))).is_err() {
        shared.worker_failure();
        shared.fail_closed();
        outcome = ShutdownOutcome::Failed;
    }
    drop(writer_guard);
    let _ = done.send(outcome);
}

fn worker_loop(
    backend: &mut dyn HistoryBackend,
    receiver: &Receiver<Work>,
    shared: &Shared,
    current: &mut Option<Work>,
) -> ShutdownOutcome {
    let mut any_failed = false;
    loop {
        if shared.canceled() {
            any_failed |= cancel_queued(backend, receiver, shared);
            any_failed |= finalize_reserved_ends(backend, shared);
            return if any_failed || shared.failed_closed() {
                ShutdownOutcome::Failed
            } else {
                ShutdownOutcome::Drained
            };
        }
        match receiver.recv() {
            Ok(work) => {
                if shared.canceled() {
                    *current = Some(work);
                    if let Some(work) = current.as_ref() {
                        any_failed |= finalize_canceled_work(work, backend, shared);
                    }
                    finish_work(
                        current.take().expect("current batch exists"),
                        CommitStatus::Canceled,
                    );
                    any_failed |= cancel_queued(backend, receiver, shared);
                    any_failed |= finalize_reserved_ends(backend, shared);
                    return if any_failed || shared.failed_closed() {
                        ShutdownOutcome::Failed
                    } else {
                        ShutdownOutcome::Drained
                    };
                }
                *current = Some(work);
                let status = if shared.failed_closed() {
                    CommitStatus::Failed
                } else {
                    process_batch(
                        current
                            .as_mut()
                            .expect("current batch exists")
                            .batch
                            .as_mut()
                            .expect("current batch is present"),
                        backend,
                        shared,
                    )
                };
                if status == CommitStatus::Canceled {
                    if let Some(work) = current.as_ref() {
                        any_failed |= finalize_canceled_work(work, backend, shared);
                    }
                }
                let work = current.take().expect("current batch exists");
                let completed_end = if status == CommitStatus::Committed {
                    work.batch
                        .as_ref()
                        .and_then(|batch| batch.ended_at_unix_ms.map(|_| batch.target))
                } else {
                    None
                };
                if status == CommitStatus::Failed {
                    any_failed = true;
                    shared.fail_closed();
                }
                finish_work(work, status);
                if let Some(target) = completed_end {
                    shared.resolve_end(target);
                }
            }
            Err(_) => {
                any_failed |= finalize_reserved_ends(backend, shared);
                return if any_failed || shared.failed_closed() {
                    ShutdownOutcome::Failed
                } else {
                    ShutdownOutcome::Drained
                };
            }
        }
    }
}

fn finish_work(work: Work, status: CommitStatus) {
    work.finish(status);
}

fn finalize_canceled_work(work: &Work, backend: &mut dyn HistoryBackend, shared: &Shared) -> bool {
    if !shared.may_finalize_canceled_ends() {
        return false;
    }
    let Some(batch) = work.batch.as_ref() else {
        return false;
    };
    let Some(at_unix_ms) = batch.ended_at_unix_ms.filter(|time| *time != 0) else {
        return false;
    };

    for attempt in 0..RETRIES {
        let result = catch_unwind(AssertUnwindSafe(|| {
            backend.finalize_incomplete_end(batch.target, batch.epoch, at_unix_ms)
        }));
        match result {
            Ok(Ok(())) => {
                shared.resolve_end(batch.target);
                shared
                    .diagnostics
                    .record(HistoryReason::CanceledEndMetadataFinalized);
                return false;
            }
            Ok(Err(error)) if error.kind() == io::ErrorKind::Unsupported => {
                shared
                    .diagnostics
                    .record(HistoryReason::CanceledEndMetadataFinalizeFailed);
                shared.fail_closed();
                return true;
            }
            Ok(Err(_)) => {
                shared.storage_error();
                if attempt + 1 == RETRIES {
                    shared.diagnostics.record(HistoryReason::RetryExhausted);
                    shared
                        .diagnostics
                        .record(HistoryReason::CanceledEndMetadataFinalizeFailed);
                    shared.fail_closed();
                    return true;
                }
                thread::sleep(RETRY_DELAY);
            }
            Err(_) => {
                shared.worker_failure();
                shared
                    .diagnostics
                    .record(HistoryReason::CanceledEndMetadataFinalizeFailed);
                shared.fail_closed();
                return true;
            }
        }
    }
    true
}

fn cancel_queued(
    backend: &mut dyn HistoryBackend,
    receiver: &Receiver<Work>,
    shared: &Shared,
) -> bool {
    let mut failed = false;
    loop {
        match receiver.try_recv() {
            Ok(work) => {
                if !shared.failed_closed() {
                    failed |= finalize_canceled_work(&work, backend, shared);
                }
                finish_work(work, CommitStatus::Canceled);
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return failed,
        }
    }
}

fn finalize_reserved_ends(backend: &mut dyn HistoryBackend, shared: &Shared) -> bool {
    let mut failed = false;
    for end in shared.pending_ends() {
        if shared.failed_closed() {
            break;
        }
        let mut storage_error_recorded = false;
        let result = if let Some(at) = end.at {
            // The helper atomically persists the conservative gap and end;
            // it also preserves the first known timestamp on a prior end.
            let mut result = Err(io::Error::other("end not attempted"));
            for attempt in 0..RETRIES {
                if attempt > 0 {
                    thread::sleep(RETRY_DELAY);
                }
                result = match catch_unwind(AssertUnwindSafe(|| {
                    backend.finalize_incomplete_end(end.target, end.epoch, at)
                })) {
                    Ok(result) => result,
                    Err(_) => {
                        shared.worker_failure();
                        shared.fail_closed();
                        result = Err(io::Error::other("history end finalizer panicked"));
                        break;
                    }
                };
                if result.is_ok()
                    || result
                        .as_ref()
                        .is_err_and(|error| error.kind() == io::ErrorKind::Unsupported)
                {
                    break;
                }
                shared.storage_error();
                storage_error_recorded = true;
                if attempt + 1 == RETRIES {
                    shared.diagnostics.record(HistoryReason::RetryExhausted);
                }
            }
            result
        } else {
            // No on-disk unknown-ended representation exists. Mark only the
            // conservative continuity gap, without inventing an exit time.
            let saved = backend.load_rows(end.target);
            match saved {
                Ok(None) => backend.mark_history(end.target, end.epoch, false),
                Ok(Some(saved))
                    if saved.ended_at_unix_ms != 0
                        || saved.epoch > end.epoch
                        || saved.capture_disabled =>
                {
                    Err(invalid_batch())
                }
                Ok(Some(_)) => backend.mark_history(end.target, end.epoch, false),
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(()) => {
                shared.resolve_end(end.target);
                shared
                    .diagnostics
                    .record(HistoryReason::ReservedEndMetadataFinalized);
            }
            Err(error) => {
                if error.kind() != io::ErrorKind::Unsupported
                    && !storage_error_recorded
                    && !shared.failed_closed()
                {
                    shared.storage_error();
                }
                shared
                    .diagnostics
                    .record(HistoryReason::ReservedEndMetadataFailed);
                shared.fail_closed();
                failed = true;
            }
        }
    }
    failed
}

fn fail_queued(receiver: &Receiver<Work>) {
    loop {
        match receiver.try_recv() {
            Ok(work) => finish_work(work, CommitStatus::Failed),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

fn process_batch(
    batch: &mut HistoryBatch,
    backend: &mut dyn HistoryBackend,
    shared: &Shared,
) -> CommitStatus {
    let row_boxes = batch.rows.take().unwrap_or_default();
    let rows = row_boxes
        .into_vec()
        .into_iter()
        .map(|bytes| bytes.into_vec())
        .collect::<Vec<_>>();

    match retry_history_stage(backend, shared, batch.target, batch.epoch, batch.gap) {
        StageResult::Done => {}
        StageResult::Canceled => return CommitStatus::Canceled,
        StageResult::Failed => return CommitStatus::Failed,
    }

    if !rows.is_empty() {
        match retry_stage(shared, || {
            backend.append_rows(batch.target, &rows, batch.gap)
        }) {
            StageResult::Done => {}
            StageResult::Canceled => return CommitStatus::Canceled,
            StageResult::Failed => return CommitStatus::Failed,
        }
    }

    for (buffer, screen) in batch.screens.iter() {
        match retry_stage(shared, || {
            backend.save_screen(batch.target, *buffer, screen)
        }) {
            StageResult::Done => {}
            StageResult::Canceled => return CommitStatus::Canceled,
            StageResult::Failed => return CommitStatus::Failed,
        }
    }

    if let Some(ended_at_unix_ms) = batch.ended_at_unix_ms {
        match retry_stage(shared, || {
            backend.mark_ended(batch.target, ended_at_unix_ms)
        }) {
            StageResult::Done => return CommitStatus::Committed,
            StageResult::Canceled => return CommitStatus::Canceled,
            StageResult::Failed => return CommitStatus::Failed,
        }
    }

    CommitStatus::Committed
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StageResult {
    Done,
    Canceled,
    Failed,
}

fn retry_history_stage(
    backend: &mut dyn HistoryBackend,
    shared: &Shared,
    target: TargetRef,
    epoch: u64,
    gap: bool,
) -> StageResult {
    for attempt in 0..RETRIES {
        if shared.canceled() {
            return StageResult::Canceled;
        }
        let existing = match backend.load_rows(target) {
            Ok(existing) => existing,
            Err(_) => {
                shared.storage_error();
                if attempt + 1 == RETRIES {
                    shared.diagnostics.record(HistoryReason::RetryExhausted);
                    return StageResult::Failed;
                }
                retry_delay(shared);
                continue;
            }
        };
        if shared.canceled() {
            return StageResult::Canceled;
        }

        let result = match existing {
            None => backend.begin_history(target, epoch, gap),
            Some(saved) if saved.ended_at_unix_ms != 0 || saved.epoch > epoch => {
                Err(invalid_batch())
            }
            Some(saved) if saved.epoch < epoch || gap => backend.mark_history(target, epoch, false),
            Some(_) => backend.begin_history(target, epoch, false),
        };
        match result {
            Ok(()) => return StageResult::Done,
            Err(_) => {
                shared.storage_error();
                if attempt + 1 == RETRIES {
                    shared.diagnostics.record(HistoryReason::RetryExhausted);
                    return StageResult::Failed;
                }
                retry_delay(shared);
            }
        }
    }
    StageResult::Failed
}

fn retry_stage<F>(shared: &Shared, mut operation: F) -> StageResult
where
    F: FnMut() -> io::Result<()>,
{
    for attempt in 0..RETRIES {
        if shared.canceled() {
            return StageResult::Canceled;
        }
        match operation() {
            Ok(()) => return StageResult::Done,
            Err(_) => {
                shared.storage_error();
                if attempt + 1 == RETRIES {
                    shared.diagnostics.record(HistoryReason::RetryExhausted);
                    return StageResult::Failed;
                }
                retry_delay(shared);
            }
        }
    }
    StageResult::Failed
}

fn retry_delay(shared: &Shared) {
    if !shared.canceled() {
        thread::sleep(RETRY_DELAY);
    }
}

fn valid_target(target: TargetRef) -> bool {
    [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .iter()
    .all(|value| *value != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_store::TestFaultPoint;
    use crate::screen_checkpoint::CheckpointMetadata;
    use crate::terminal::{ScreenSnapshot, TerminalSize};
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        history: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = PathBuf::from(format!(
                "/private/tmp/panelink-history-writer-{}-{suffix}",
                std::process::id()
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("create private writer fixture");
            let history = root.join("history");
            Self { root, history }
        }

        fn prepare(&self) -> (ServerStorage, HistoryStore, TargetRef) {
            let storage = ServerStorage::initialize(&self.root).expect("initialize storage");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&self.history)
                .expect("create private history directory");
            let store = HistoryStore::open(&self.history).expect("open history store");
            let target = make_target(storage.generation());
            (storage, store, target)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn make_target(generation: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace: 1,
            pane: 2,
            session: 3,
            occupant_revision: 1,
        }
    }

    fn row_records(target: TargetRef, epoch: u64) -> Vec<Vec<u8>> {
        let mut parser = vt100::Parser::new(2, 20, 1024);
        let cursor = parser.screen().history_cursor().expect("history cursor");
        parser.process(b"writer-row-one\r\nwriter-row-two\r\nwriter-row-three\r\n");
        parser
            .screen()
            .history_after(cursor)
            .expect("rows after cursor")
            .map(|row| history_rows::encode(target, epoch, row, 456).expect("encode row"))
            .collect()
    }

    fn screen_record(target: TargetRef, buffer: ScreenBuffer, size: TerminalSize) -> Vec<u8> {
        let snapshot = ScreenSnapshot::try_new(size).expect("allocate snapshot");
        let metadata = CheckpointMetadata::new(target, 1, 456, 2).expect("checkpoint metadata");
        let mut encoded = screen_checkpoint::encode(&snapshot, &metadata).expect("encode screen");
        encoded[80] = match buffer {
            ScreenBuffer::Normal => 0,
            ScreenBuffer::Alternate => 1,
        };
        screen_checkpoint::decode(&encoded).expect("screen remains valid");
        encoded
    }

    fn empty_batch(target: TargetRef, epoch: u64, token: u64) -> HistoryBatch {
        HistoryBatch::new(target, epoch, token, false, Vec::new(), Vec::new(), None)
            .expect("valid empty batch")
    }

    #[test]
    fn resolved_end_reservations_are_reused_beyond_the_fixed_table() {
        // Parent review regression: repeated split/close in one generation
        // must not permanently exhaust the eight metadata-only slots.
        let fixture = Fixture::new();
        let (storage, store, base) = fixture.prepare();
        let writer = HistoryWriter::start(store, &storage).unwrap();
        for pane in 1..=MAX_END_RESERVATIONS as u64 {
            let target = TargetRef {
                pane,
                session: pane,
                ..base
            };
            assert!(writer.reserve_end(target, 1));
        }
        let extra = TargetRef {
            pane: 100,
            session: 100,
            ..base
        };
        assert!(!writer.reserve_end(extra, 1), "unresolved table is full");
        writer.shared.resolve_end(TargetRef {
            pane: 1,
            session: 1,
            ..base
        });
        assert!(writer.reserve_end(extra, 1), "resolved slot is reused");
        assert_eq!(
            writer.shutdown(MAX_SHUTDOWN_WAIT).outcome,
            ShutdownOutcome::Drained
        );
    }

    #[test]
    fn real_store_reserved_known_and_unknown_ends_are_conservative() {
        let fixture = Fixture::new();
        let (storage, store, known) = fixture.prepare();
        let unknown = TargetRef {
            pane: 4,
            session: 4,
            ..known
        };
        let writer = HistoryWriter::start(store, &storage).unwrap();
        assert!(writer.reserve_end(known, 1));
        assert!(writer.reserve_end(unknown, 1));
        writer.record_end(known, 1, Some(777), true);
        writer.record_end(unknown, 1, None, true);
        assert_eq!(
            writer.shutdown(MAX_SHUTDOWN_WAIT).outcome,
            ShutdownOutcome::Drained
        );
        // Release the independently-owned storage handle, then wait for the
        // worker's retained duplicate lock before opening the committed store.
        drop(storage);
        wait_for_lock(&fixture.root);
        let store = HistoryStore::open(&fixture.history).unwrap();
        let saved_known = store.load_rows(known).unwrap().unwrap();
        assert_eq!(saved_known.ended_at_unix_ms, 777);
        assert!(saved_known.gap);
        let saved_unknown = store.load_rows(unknown).unwrap().unwrap();
        assert_eq!(saved_unknown.ended_at_unix_ms, 0);
        assert!(saved_unknown.gap);
    }

    fn one_row_batch(target: TargetRef, epoch: u64, token: u64) -> HistoryBatch {
        let mut rows = row_records(target, epoch);
        rows.truncate(1);
        HistoryBatch::new(target, epoch, token, false, rows, Vec::new(), None)
            .expect("valid row batch")
    }

    fn ended_batch(target: TargetRef, epoch: u64, token: u64, at_unix_ms: u64) -> HistoryBatch {
        HistoryBatch::new(
            target,
            epoch,
            token,
            false,
            row_records(target, epoch),
            Vec::new(),
            Some(at_unix_ms),
        )
        .expect("valid ended batch")
    }

    fn wait_receipt(receipt: &mut Receipt) -> CommitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = receipt.poll();
            if status != CommitStatus::Pending {
                assert_eq!(receipt.poll(), status, "receipt must remain stable");
                return status;
            }
            assert!(Instant::now() < deadline, "receipt did not settle");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_lock(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match ServerStorage::lock_for_maintenance(path) {
                Ok(lock) => {
                    drop(lock);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("maintenance lock failed: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "retained server lock did not release"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Load,
        Begin,
        Mark,
        Append,
        Screen,
        End,
        Finalize,
    }

    enum Behavior {
        Fail,
        Unsupported,
        Block(Arc<Gate>),
        BlockThenFail(Arc<Gate>),
        BlockThenPanic(Arc<Gate>),
        Panic,
    }

    struct Gate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                state: Mutex::new((false, false)),
                changed: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }

        fn wait_until_entered(&self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            while !state.0 {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "backend did not reach blocking stage");
                let (next, timeout) = self
                    .changed
                    .wait_timeout(state, left)
                    .unwrap_or_else(|error| error.into_inner());
                state = next;
                assert!(!timeout.timed_out() || state.0, "backend stage timed out");
            }
        }

        fn release(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.1 = true;
            self.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct FakeHistory {
        epoch: u64,
        gap: bool,
        ended_at_unix_ms: u64,
        row_count: usize,
        screens: Vec<(ScreenBuffer, Vec<u8>)>,
    }

    #[derive(Default)]
    struct FakeState {
        calls: Vec<Call>,
        finalizations: Vec<(TargetRef, u64, u64)>,
        history: Option<FakeHistory>,
    }

    #[derive(Clone, Default)]
    struct FakeControl {
        state: Arc<Mutex<FakeState>>,
        behaviors: Arc<Mutex<VecDeque<(Call, Behavior)>>>,
    }

    impl FakeControl {
        fn schedule(&self, call: Call, behavior: Behavior) {
            self.behaviors
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_back((call, behavior));
        }

        fn calls(&self) -> Vec<Call> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .calls
                .clone()
        }

        fn finalizations(&self) -> Vec<(TargetRef, u64, u64)> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .finalizations
                .clone()
        }

        fn history(&self) -> Option<(u64, bool, u64, usize)> {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .history
                .as_ref()
                .map(|saved| {
                    (
                        saved.epoch,
                        saved.gap,
                        saved.ended_at_unix_ms,
                        saved.row_count,
                    )
                })
        }
    }

    struct FakeBackend(FakeControl);

    impl FakeBackend {
        fn before(&mut self, call: Call) -> io::Result<()> {
            self.0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .calls
                .push(call);
            let behavior = {
                let mut queue = self
                    .0
                    .behaviors
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if queue
                    .front()
                    .is_some_and(|(scheduled, _)| *scheduled == call)
                {
                    queue.pop_front().map(|(_, behavior)| behavior)
                } else {
                    None
                }
            };
            match behavior {
                Some(Behavior::Fail) => Err(io::Error::other("injected storage failure")),
                Some(Behavior::Unsupported) => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "injected unsupported operation",
                )),
                Some(Behavior::Block(gate)) => {
                    gate.wait();
                    Ok(())
                }
                Some(Behavior::BlockThenFail(gate)) => {
                    gate.wait();
                    Err(io::Error::other("injected storage failure"))
                }
                Some(Behavior::BlockThenPanic(gate)) => {
                    gate.wait();
                    std::panic::resume_unwind(Box::new("test panic after blocking"))
                }
                Some(Behavior::Panic) => std::panic::resume_unwind(Box::new("test panic")),
                None => Ok(()),
            }
        }
    }

    impl HistoryBackend for FakeBackend {
        fn load_rows(&mut self, _target: TargetRef) -> io::Result<Option<SavedHistory>> {
            self.before(Call::Load)?;
            let state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            Ok(state.history.as_ref().map(|saved| SavedHistory {
                gap: saved.gap,
                capture_disabled: false,
                epoch: saved.epoch,
                ended_at_unix_ms: saved.ended_at_unix_ms,
                records: Vec::new(),
            }))
        }

        fn begin_history(&mut self, _target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
            self.before(Call::Begin)?;
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match state.history.as_mut() {
                None => {
                    state.history = Some(FakeHistory {
                        epoch,
                        gap,
                        ..FakeHistory::default()
                    });
                    Ok(())
                }
                Some(saved) if saved.epoch == epoch && saved.ended_at_unix_ms == 0 => Ok(()),
                Some(_) => Err(invalid_batch()),
            }
        }

        fn mark_history(
            &mut self,
            _target: TargetRef,
            epoch: u64,
            _disabled: bool,
        ) -> io::Result<()> {
            self.before(Call::Mark)?;
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let saved = state.history.get_or_insert_with(FakeHistory::default);
            if saved.ended_at_unix_ms != 0 || epoch < saved.epoch {
                return Err(invalid_batch());
            }
            if epoch != saved.epoch {
                saved.epoch = epoch;
                saved.row_count = 0;
            }
            saved.gap = true;
            Ok(())
        }

        fn append_rows(
            &mut self,
            _target: TargetRef,
            rows: &[Vec<u8>],
            gap: bool,
        ) -> io::Result<()> {
            self.before(Call::Append)?;
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(saved) = state.history.as_mut() else {
                return Err(invalid_batch());
            };
            if saved.ended_at_unix_ms != 0 {
                return Err(invalid_batch());
            }
            saved.gap |= gap;
            saved.row_count += rows.len();
            Ok(())
        }

        fn save_screen(
            &mut self,
            _target: TargetRef,
            buffer: ScreenBuffer,
            screen: &[u8],
        ) -> io::Result<()> {
            self.before(Call::Screen)?;
            self.0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .history
                .as_mut()
                .ok_or_else(invalid_batch)?
                .screens
                .push((buffer, screen.to_vec()));
            Ok(())
        }

        fn mark_ended(&mut self, _target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
            self.before(Call::End)?;
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(saved) = state.history.as_mut() else {
                return Err(invalid_batch());
            };
            if saved.ended_at_unix_ms == 0 {
                saved.ended_at_unix_ms = at_unix_ms;
            }
            Ok(())
        }

        fn finalize_incomplete_end(
            &mut self,
            target: TargetRef,
            epoch: u64,
            at_unix_ms: u64,
        ) -> io::Result<()> {
            self.before(Call::Finalize)?;
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.finalizations.push((target, epoch, at_unix_ms));
            match state.history.as_mut() {
                None => {
                    state.history = Some(FakeHistory {
                        epoch,
                        gap: true,
                        ended_at_unix_ms: at_unix_ms,
                        ..FakeHistory::default()
                    });
                    Ok(())
                }
                Some(saved) if saved.epoch > epoch => Err(invalid_batch()),
                Some(saved) if saved.epoch < epoch && saved.ended_at_unix_ms != 0 => {
                    Err(invalid_batch())
                }
                Some(saved) if saved.epoch < epoch => {
                    *saved = FakeHistory {
                        epoch,
                        gap: true,
                        ended_at_unix_ms: at_unix_ms,
                        ..FakeHistory::default()
                    };
                    Ok(())
                }
                Some(saved) if saved.ended_at_unix_ms == 0 => {
                    saved.gap = true;
                    saved.ended_at_unix_ms = at_unix_ms;
                    Ok(())
                }
                Some(saved) if saved.epoch == epoch => Ok(()),
                Some(_) => Err(invalid_batch()),
            }
        }
    }

    fn fake_writer(
        fixture: &Fixture,
        backend: FakeControl,
    ) -> (ServerStorage, HistoryWriter, TargetRef) {
        let (storage, _store, target) = fixture.prepare();
        let writer = HistoryWriter::start_with_backend(Box::new(FakeBackend(backend)), &storage)
            .expect("start fake writer");
        (storage, writer, target)
    }

    #[test]
    fn validates_full_identity_order_unique_buffers_and_redacted_debug() {
        let fixture = Fixture::new();
        let (_storage, _store, target) = fixture.prepare();
        let rows = row_records(target, 4);
        assert!(!rows.is_empty());
        let duplicate_buffer = screen_record(
            target,
            ScreenBuffer::Normal,
            TerminalSize::new(8, 20).unwrap(),
        );
        assert!(HistoryBatch::new(
            target,
            4,
            1,
            false,
            rows.clone(),
            vec![duplicate_buffer.clone(), duplicate_buffer],
            None,
        )
        .is_err());
        assert!(HistoryBatch::new(target, 5, 1, false, rows.clone(), Vec::new(), None,).is_err());
        let other = make_target(target.generation.wrapping_add(1));
        assert!(
            HistoryBatch::new(target, 4, 1, false, row_records(other, 4), Vec::new(), None,)
                .is_err()
        );
        assert!(HistoryBatch::new(target, 4, 0, false, Vec::new(), Vec::new(), None).is_err());
        assert!(HistoryBatch::new(target, 4, 1, false, Vec::new(), Vec::new(), Some(0)).is_err());

        let text_canary = b"PRIVATE-ROW-CANARY";
        let mut parser = vt100::Parser::new(2, 20, 64);
        let cursor = parser.screen().history_cursor().unwrap();
        parser.process(text_canary);
        parser.process(b"\r\nother-line\r\nfinal-line\r\n");
        let canary_rows = parser
            .screen()
            .history_after(cursor)
            .unwrap()
            .map(|row| history_rows::encode(target, 4, row, 456).unwrap())
            .collect();
        let batch = HistoryBatch::new(target, 4, 1, false, canary_rows, Vec::new(), None).unwrap();
        assert!(!format!("{batch:?}").contains("PRIVATE-ROW-CANARY"));
    }

    #[test]
    fn newest_suffix_budget_preserves_screens_and_validates_before_trimming() {
        let fixture = Fixture::new();
        let (_storage, _store, target) = fixture.prepare();
        let mut rows = maximum_rows(target, 4);
        rows.truncate(3);
        let screens = vec![
            screen_record(
                target,
                ScreenBuffer::Normal,
                TerminalSize::new(8, 20).unwrap(),
            ),
            screen_record(
                target,
                ScreenBuffer::Alternate,
                TerminalSize::new(8, 20).unwrap(),
            ),
        ];
        let original_row_bytes = rows.iter().map(Vec::len).sum::<usize>();
        let screen_bytes = screens.iter().map(Vec::len).sum::<usize>();
        let expected_screen_bytes = screens.clone();
        let expected_suffix = rows[1..].to_vec();
        let original =
            HistoryBatch::new(target, 4, 17, false, rows.clone(), screens, Some(456)).unwrap();
        assert_eq!(original.payload_bytes, original_row_bytes + screen_bytes);

        let (unchanged, removed_rows, has_rows) = original
            .retain_newest_suffix_within(original_row_bytes + screen_bytes)
            .expect("the full validated payload fits");
        assert_eq!(removed_rows, 0);
        assert!(has_rows);
        assert!(!unchanged.gap);
        assert_eq!(unchanged.payload_bytes, original_row_bytes + screen_bytes);
        assert_eq!(unchanged.screens.len(), 2);
        assert_eq!(unchanged.screens[0].1.as_ref(), expected_screen_bytes[0]);
        assert_eq!(unchanged.screens[1].1.as_ref(), expected_screen_bytes[1]);
        assert_eq!(unchanged.ended_at_unix_ms, Some(456));
        assert_eq!((unchanged.token, unchanged.epoch), (17, 4));
        let original_rows = unchanged.rows.as_ref().unwrap();
        assert_eq!(original_rows.len(), 3);
        assert!(original_rows
            .iter()
            .zip(&rows)
            .all(|(actual, expected)| actual.as_ref() == expected));

        let budget = screen_bytes + rows[1].len() + rows[2].len();
        let trimmed = HistoryBatch::new(
            target,
            4,
            18,
            false,
            rows.clone(),
            expected_screen_bytes.clone(),
            Some(456),
        )
        .unwrap();
        let (suffix, removed_rows, has_rows) = trimmed
            .retain_newest_suffix_within(budget)
            .expect("the newest two rows fit with all screens");
        assert_eq!(removed_rows, 1);
        assert!(has_rows);
        assert!(suffix.gap);
        assert_eq!(suffix.payload_bytes, budget);
        assert_eq!(suffix.screens.len(), 2);
        assert_eq!(suffix.screens[0].1.as_ref(), expected_screen_bytes[0]);
        assert_eq!(suffix.screens[1].1.as_ref(), expected_screen_bytes[1]);
        assert_eq!(suffix.ended_at_unix_ms, Some(456));
        assert_eq!((suffix.token, suffix.epoch), (18, 4));
        assert_eq!(
            suffix
                .rows
                .as_ref()
                .unwrap()
                .iter()
                .map(|row| row.as_ref().to_vec())
                .collect::<Vec<_>>(),
            expected_suffix
        );

        let no_row_fits = HistoryBatch::new(
            target,
            4,
            19,
            false,
            rows.clone(),
            expected_screen_bytes.clone(),
            Some(456),
        )
        .unwrap();
        assert!(no_row_fits
            .retain_newest_suffix_within(screen_bytes + rows[2].len() - 1)
            .is_none());

        let screen_only = HistoryBatch::new(
            target,
            4,
            20,
            false,
            Vec::new(),
            expected_screen_bytes.clone(),
            Some(456),
        )
        .unwrap();
        let (screen_only, removed_rows, has_rows) = screen_only
            .retain_newest_suffix_within(screen_bytes)
            .expect("screen-only payload fits");
        assert_eq!(removed_rows, 0);
        assert!(!has_rows);
        assert_eq!(screen_only.payload_bytes, screen_bytes);
        assert_eq!(screen_only.screens.len(), 2);
        assert!(HistoryBatch::new(
            target,
            4,
            21,
            false,
            Vec::new(),
            expected_screen_bytes.clone(),
            Some(456),
        )
        .unwrap()
        .retain_newest_suffix_within(screen_bytes - 1)
        .is_none());

        let invalid_prefix = vec![vec![0], rows[1].clone(), rows[2].clone()];
        assert!(HistoryBatch::new(
            target,
            4,
            22,
            false,
            invalid_prefix,
            expected_screen_bytes,
            Some(456),
        )
        .is_err());
    }

    #[test]
    fn empty_batch_initializes_history_and_same_epoch_retry_only_syncs_begin() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let mut first = writer.try_submit(empty_batch(target, 1, 1)).unwrap();
        let mut second = writer.try_submit(empty_batch(target, 1, 2)).unwrap();
        assert_eq!(wait_receipt(&mut second), CommitStatus::Committed);
        assert_eq!(wait_receipt(&mut first), CommitStatus::Committed);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        assert_eq!(
            control.calls(),
            vec![Call::Load, Call::Begin, Call::Load, Call::Begin]
        );
        assert_eq!(control.history().unwrap(), (1, false, 0, 0));
    }

    #[test]
    fn fifo_epoch_reset_marks_gap_and_keeps_receipt_identity() {
        fn admit(writer: &mut HistoryWriter, mut batch: HistoryBatch) -> Receipt {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match writer.try_submit(batch) {
                    Ok(receipt) => return receipt,
                    Err(error) => {
                        // This scenario verifies FIFO/epoch receipts. Startup
                        // and quota-release contention is a valid rejection;
                        // keep the same owned batch and token until it clears.
                        assert_eq!(error.capacity_reason(), Some(CapacityReason::Contended));
                        assert!(
                            Instant::now() < deadline,
                            "accounting contention did not settle"
                        );
                        batch = error.into_batch();
                        thread::yield_now();
                    }
                }
            }
        }
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let mut old = admit(&mut writer, one_row_batch(target, 1, 1));
        let reset = HistoryBatch::new(target, 2, 2, true, row_records(target, 2), Vec::new(), None)
            .unwrap();
        // Leave the batch owned by the writer; no producer-side mutation is
        // available after this call.
        let mut current = admit(&mut writer, reset);
        assert_eq!(wait_receipt(&mut current), CommitStatus::Committed);
        assert_eq!(wait_receipt(&mut old), CommitStatus::Committed);
        assert_eq!((old.epoch(), old.token()), (1, 1));
        assert_eq!((current.epoch(), current.token()), (2, 2));
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        let history = control.history().unwrap();
        assert_eq!(history.0, 2);
        assert!(history.1);
        assert_eq!(history.3, row_records(target, 2).len());
        assert!(control.calls().contains(&Call::Mark));
    }

    #[test]
    fn count_quota_includes_inflight_and_submission_rejection_returns_ownership() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut receipts = Vec::new();
        receipts.push(writer.try_submit(empty_batch(target, 1, 1)).unwrap());
        gate.wait_until_entered();
        for token in 2..=MAX_QUEUED_BATCHES as u64 {
            receipts.push(writer.try_submit(empty_batch(target, 1, token)).unwrap());
        }

        let rejected = empty_batch(target, 1, 17);
        let started = Instant::now();
        let error = writer
            .try_submit(rejected)
            .expect_err("the count limit is full");
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(error.reason(), SubmitReason::Full);
        assert_eq!(error.capacity_reason(), Some(CapacityReason::Count));
        let returned = error.into_batch();
        assert_eq!(returned.token, 17);
        assert!(!format!("{returned:?}").contains("PRIVATE"));

        // Capacity rejection did not consume token 17; a stale-token
        // rejection did not consume any caller payload either.
        let stale = writer
            .try_submit(empty_batch(target, 1, 16))
            .expect_err("tokens must increase");
        assert_eq!(stale.reason(), SubmitReason::StaleToken);
        drop(stale.into_batch());

        gate.release();
        for receipt in &mut receipts {
            assert_eq!(wait_receipt(receipt), CommitStatus::Committed);
        }
        let mut retried = writer
            .try_submit(returned)
            .expect("rejected token remains available");
        assert_eq!(wait_receipt(&mut retried), CommitStatus::Committed);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionCountFull));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionBytesFull));
    }

    #[test]
    fn contended_capacity_check_is_reported_without_consuming_the_batch() {
        let fixture = Fixture::new();
        let (_storage, mut writer, target) = fake_writer(&fixture, FakeControl::default());
        let diagnostics = writer.diagnostics();
        let shared = Arc::clone(&writer.shared);
        let guard = shared.lock();
        let rejected = writer
            .try_submit(empty_batch(target, 1, 1))
            .expect_err("the accounting mutex is contended");
        assert_eq!(rejected.reason(), SubmitReason::Full);
        assert_eq!(rejected.capacity_reason(), Some(CapacityReason::Contended));
        assert_eq!(rejected.into_batch().token, 1);
        drop(guard);

        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionContended));
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
    }

    #[test]
    fn payload_snapshot_counts_inflight_and_contention_adds_no_diagnostic() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let batch = maximum_payload_batch(target, 1);
        let payload_bytes = batch.payload_bytes;
        let mut receipt = writer.try_submit(batch).expect("admit bounded payload");
        gate.wait_until_entered();

        assert_eq!(
            writer.available_payload_bytes(),
            Some(MAX_QUEUED_PAYLOAD_BYTES - payload_bytes),
            "the snapshot includes the in-flight batch"
        );
        let guard = writer.shared.lock();
        assert_eq!(writer.available_payload_bytes(), None);
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionContended));
        drop(guard);

        gate.release();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Committed);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
    }

    fn maximum_rows(target: TargetRef, epoch: u64) -> Vec<Vec<u8>> {
        let mut parser = vt100::Parser::new(1, 240, 2048);
        let cursor = parser.screen().history_cursor().unwrap();
        let mut output = Vec::with_capacity(1025 * 242);
        for _ in 0..1025 {
            output.extend(std::iter::repeat_n(b'x', 240));
            output.extend_from_slice(b"\r\n");
        }
        parser.process(&output);
        let mut records = parser
            .screen()
            .history_after(cursor)
            .unwrap()
            .map(|row| history_rows::encode(target, epoch, row, 456).unwrap())
            .take(MAX_BATCH_ROWS)
            .collect::<Vec<_>>();
        assert_eq!(records.len(), MAX_BATCH_ROWS, "fixture must fill row cap");
        assert!(records.iter().all(|record| record.len() == MAX_ROW_BYTES));
        records.shrink_to_fit();
        records
    }

    fn maximum_payload_batch(target: TargetRef, token: u64) -> HistoryBatch {
        let rows = maximum_rows(target, 1);
        let size = TerminalSize::new(80, 240).unwrap();
        let screens = vec![
            screen_record(target, ScreenBuffer::Normal, size),
            screen_record(target, ScreenBuffer::Alternate, size),
        ];
        HistoryBatch::new(target, 1, token, false, rows, screens, None).unwrap()
    }

    #[test]
    fn encoded_byte_quota_counts_boxed_inflight_and_queued_payload() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let large_size = MAX_BATCH_ROWS * MAX_ROW_BYTES + 2 * MAX_SCREEN_CHECKPOINT_BYTES;
        assert!(large_size < MAX_QUEUED_PAYLOAD_BYTES);
        let first = writer.try_submit(maximum_payload_batch(target, 1)).unwrap();
        gate.wait_until_entered();
        let mut second = writer.try_submit(maximum_payload_batch(target, 2)).unwrap();
        let third = maximum_payload_batch(target, 3);
        let rejected = writer
            .try_submit(third)
            .expect_err("the byte limit is full");
        assert_eq!(rejected.reason(), SubmitReason::Full);
        assert_eq!(rejected.capacity_reason(), Some(CapacityReason::Bytes));
        assert_eq!(rejected.into_batch().token, 3);
        gate.release();
        let mut first = first;
        assert_eq!(wait_receipt(&mut first), CommitStatus::Committed);
        assert_eq!(wait_receipt(&mut second), CommitStatus::Committed);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionBytesFull));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionCountFull));
    }

    #[test]
    fn byte_rejection_retries_unchanged_after_worker_frees_capacity() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();

        let mut first = writer.try_submit(maximum_payload_batch(target, 1)).unwrap();
        gate.wait_until_entered();
        let mut second = writer.try_submit(maximum_payload_batch(target, 2)).unwrap();
        let third_rows = maximum_rows(target, 1)
            .into_iter()
            .take(520)
            .collect::<Vec<_>>();
        let third = HistoryBatch::new(target, 1, 3, false, third_rows, Vec::new(), None).unwrap();
        let third_bytes = third.payload_bytes;
        let mut third = writer.try_submit(third).unwrap();

        let expected_rows = maximum_rows(target, 1)
            .into_iter()
            .take(500)
            .collect::<Vec<_>>();
        let expected_screens = vec![
            screen_record(
                target,
                ScreenBuffer::Normal,
                TerminalSize::new(80, 240).unwrap(),
            ),
            screen_record(
                target,
                ScreenBuffer::Alternate,
                TerminalSize::new(80, 240).unwrap(),
            ),
        ];
        let rejected = HistoryBatch::new(
            target,
            1,
            4,
            false,
            expected_rows.clone(),
            expected_screens.clone(),
            Some(777),
        )
        .unwrap();
        let rejected_bytes = rejected.payload_bytes;
        let occupied_bytes = 2 * maximum_payload_batch(target, 9).payload_bytes + third_bytes;
        assert!(occupied_bytes < MAX_QUEUED_PAYLOAD_BYTES);
        assert!(MAX_QUEUED_PAYLOAD_BYTES - occupied_bytes < rejected_bytes);

        let rejected = writer
            .try_submit(rejected)
            .expect_err("accepted work still occupies the byte quota");
        assert_eq!(rejected.reason(), SubmitReason::Full);
        assert_eq!(rejected.capacity_reason(), Some(CapacityReason::Bytes));
        let rejected = rejected.into_batch();
        assert_eq!(rejected.token, 4);
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionBytesFull));

        gate.release();
        assert_eq!(wait_receipt(&mut first), CommitStatus::Committed);
        assert_eq!(wait_receipt(&mut second), CommitStatus::Committed);
        assert_eq!(wait_receipt(&mut third), CommitStatus::Committed);
        let available = writer
            .available_payload_bytes()
            .expect("capacity snapshot succeeds after the worker drains");
        assert!(available >= rejected_bytes);
        let (retry, removed_rows, retained_rows) = rejected
            .retain_newest_suffix_within(available)
            .expect("the complete rejected payload now fits");
        assert_eq!(removed_rows, 0);
        assert!(retained_rows);
        assert!(!retry.gap);
        assert_eq!(retry.payload_bytes, rejected_bytes);
        assert_eq!((retry.token, retry.epoch), (4, 1));
        assert_eq!(retry.ended_at_unix_ms, Some(777));
        assert_eq!(
            retry
                .rows
                .as_ref()
                .unwrap()
                .iter()
                .map(|row| row.as_ref().to_vec())
                .collect::<Vec<_>>(),
            expected_rows
        );
        assert_eq!(retry.screens.len(), expected_screens.len());
        for ((_, actual), expected) in retry.screens.iter().zip(expected_screens) {
            assert_eq!(actual.as_ref(), expected.as_slice());
        }

        let mut retry = writer
            .try_submit(retry)
            .expect("initial rejection did not consume token 4");
        assert_eq!(wait_receipt(&mut retry), CommitStatus::Committed);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
    }

    #[test]
    fn retry_repeats_only_the_failed_stage_and_sets_sticky_error() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        control.schedule(Call::Begin, Behavior::Fail);
        control.schedule(Call::Append, Behavior::Fail);
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut receipt = writer
            .try_submit(one_row_batch(target, 1, 1))
            .expect("admit batch");
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Committed);
        assert!(writer.had_storage_error());
        assert!(diagnostics.snapshot().contains(HistoryReason::StorageError));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::RetryExhausted));
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        assert_eq!(
            control.calls(),
            vec![
                Call::Load,
                Call::Begin,
                Call::Load,
                Call::Begin,
                Call::Append,
                Call::Append,
            ]
        );
        assert_eq!(control.history().unwrap().3, 1);
    }

    #[test]
    fn exhausted_stage_fails_writer_closed_without_later_effects() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Append, Behavior::BlockThenFail(Arc::clone(&gate)));
        control.schedule(Call::Append, Behavior::Fail);
        control.schedule(Call::Append, Behavior::Fail);
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut first = writer
            .try_submit(one_row_batch(target, 1, 1))
            .expect("admit first");
        gate.wait_until_entered();
        let mut later = writer
            .try_submit(one_row_batch(target, 1, 2))
            .expect("queue later batch");
        gate.release();
        assert_eq!(wait_receipt(&mut first), CommitStatus::Failed);
        assert_eq!(wait_receipt(&mut later), CommitStatus::Failed);
        assert!(writer.had_storage_error());
        assert!(diagnostics.snapshot().contains(HistoryReason::StorageError));
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::RetryExhausted));
        assert_eq!(
            writer
                .try_submit(empty_batch(target, 1, 3))
                .unwrap_err()
                .reason(),
            SubmitReason::Failed
        );
        assert_eq!(
            control
                .calls()
                .iter()
                .filter(|call| **call == Call::Append)
                .count(),
            3,
            "the queued later batch must not reach storage"
        );
        assert_eq!(control.history().unwrap().3, 0);
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Failed
        );
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::Failed
        );
    }

    #[test]
    fn timeout_cancels_between_stages_and_worker_retains_lock_until_return() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut first = writer
            .try_submit(one_row_batch(target, 1, 1))
            .expect("admit blocked batch");
        gate.wait_until_entered();
        let mut queued = writer
            .try_submit(empty_batch(target, 1, 2))
            .expect("queue second batch");
        let started = Instant::now();
        let report = writer.shutdown(Duration::from_secs(5));
        assert_eq!(report.outcome, ShutdownOutcome::TimedOut);
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::ShutdownTimedOut));
        assert!(started.elapsed() <= MAX_SHUTDOWN_WAIT + Duration::from_millis(100));
        drop(storage);
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(first.poll(), CommitStatus::Pending);
        gate.release();
        assert_eq!(wait_receipt(&mut first), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued), CommitStatus::Canceled);
        assert!(diagnostics.snapshot().contains(HistoryReason::WorkCanceled));
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut,
            "late cancellation must not replace the actual timed-out result"
        );
        wait_for_lock(&fixture.root);
    }

    #[test]
    fn timeout_finalizes_only_known_ends_in_fifo_without_payload_calls() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        control.schedule(Call::Finalize, Behavior::Fail);
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut current = writer.try_submit(ended_batch(target, 1, 1, 500)).unwrap();
        gate.wait_until_entered();
        let mut queued_end = writer.try_submit(ended_batch(target, 1, 2, 600)).unwrap();
        let mut queued_unknown = writer
            .try_submit(one_row_batch(target, 1, 3))
            .expect("queue batch without a known end");

        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        gate.release();
        assert_eq!(wait_receipt(&mut current), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued_end), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued_unknown), CommitStatus::Canceled);
        assert_eq!(
            control.finalizations(),
            vec![(target, 1, 500), (target, 1, 600)],
            "finalizers follow accepted FIFO order and omit unknown end times"
        );
        assert_eq!(
            control.calls(),
            vec![Call::Load, Call::Finalize, Call::Finalize, Call::Finalize],
            "only the failed finalizer is retried after timeout cancellation"
        );
        assert_eq!(control.history().unwrap(), (1, true, 500, 0));
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert!(snapshot.contains(HistoryReason::StorageError));
        assert!(!snapshot.contains(HistoryReason::RetryExhausted));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    }

    #[test]
    fn timeout_finalization_retries_are_bounded_and_fail_closed() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        for _ in 0..RETRIES {
            control.schedule(Call::Finalize, Behavior::Fail);
        }
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut current = writer.try_submit(ended_batch(target, 1, 1, 700)).unwrap();
        gate.wait_until_entered();
        let mut queued = writer.try_submit(ended_batch(target, 1, 2, 800)).unwrap();
        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        gate.release();
        assert_eq!(wait_receipt(&mut current), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued), CommitStatus::Canceled);
        assert_eq!(
            control.calls(),
            vec![Call::Load, Call::Finalize, Call::Finalize, Call::Finalize]
        );
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.contains(HistoryReason::StorageError));
        assert!(snapshot.contains(HistoryReason::RetryExhausted));
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    }

    #[test]
    fn unsupported_finalizer_fails_immediately_without_storage_error() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        control.schedule(Call::Finalize, Behavior::Unsupported);
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut current = writer.try_submit(ended_batch(target, 1, 1, 850)).unwrap();
        gate.wait_until_entered();
        let mut queued = writer.try_submit(ended_batch(target, 1, 2, 860)).unwrap();
        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        gate.release();
        assert_eq!(wait_receipt(&mut current), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued), CommitStatus::Canceled);
        assert_eq!(control.calls(), vec![Call::Load, Call::Finalize]);
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(!snapshot.contains(HistoryReason::StorageError));
        assert!(!snapshot.contains(HistoryReason::RetryExhausted));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    }

    #[test]
    fn finalizer_panic_fails_closed_and_skips_later_metadata() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        control.schedule(Call::Finalize, Behavior::Panic);
        let (_storage, mut writer, target) = fake_writer(&fixture, control.clone());
        let diagnostics = writer.diagnostics();
        let mut current = writer.try_submit(ended_batch(target, 1, 1, 870)).unwrap();
        gate.wait_until_entered();
        let mut queued = writer.try_submit(ended_batch(target, 1, 2, 880)).unwrap();
        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        gate.release();
        assert_eq!(wait_receipt(&mut current), CommitStatus::Canceled);
        assert_eq!(wait_receipt(&mut queued), CommitStatus::Canceled);
        assert_eq!(control.calls(), vec![Call::Load, Call::Finalize]);
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.contains(HistoryReason::WorkerFailure));
        assert!(!snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalizeFailed));
        assert!(!snapshot.contains(HistoryReason::StorageError));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    }

    #[test]
    fn timed_out_finalizer_keeps_singleton_until_late_diagnostics_are_published() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let load_gate = Arc::new(Gate::new());
        let finalize_gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&load_gate)));
        control.schedule(Call::Finalize, Behavior::Block(Arc::clone(&finalize_gate)));
        let (storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut receipt = writer.try_submit(ended_batch(target, 1, 1, 900)).unwrap();
        load_gate.wait_until_entered();
        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        drop(storage);
        load_gate.release();
        finalize_gate.wait_until_entered();
        assert_eq!(receipt.poll(), CommitStatus::Pending);
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::CanceledEndMetadataFinalized));
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock,
            "the detached worker retains the singleton through finalization"
        );
        finalize_gate.release();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Canceled);
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::CanceledEndMetadataFinalized));
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
        wait_for_lock(&fixture.root);
    }

    #[test]
    fn executing_final_stage_can_commit_after_timeout() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::End, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut receipt = writer
            .try_submit(
                HistoryBatch::new(target, 1, 1, false, Vec::new(), Vec::new(), Some(900)).unwrap(),
            )
            .unwrap();
        gate.wait_until_entered();
        assert_eq!(
            writer.shutdown(Duration::from_millis(10)).outcome,
            ShutdownOutcome::TimedOut
        );
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
        assert_eq!(
            storage
                .claim_history_writer()
                .err()
                .expect("timed-out worker retains its writer claim")
                .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(storage);
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        gate.release();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Committed);
        assert!(!diagnostics.snapshot().contains(HistoryReason::WorkCanceled));
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut,
            "a late final commit must not replace the shutdown result"
        );
        wait_for_lock(&fixture.root);
    }

    #[test]
    fn plain_drop_detaches_and_cancels_without_joining() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::Block(Arc::clone(&gate)));
        let (storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut receipt = writer
            .try_submit(ended_batch(target, 1, 1, 901))
            .expect("admit blocked batch");
        gate.wait_until_entered();
        drop(storage);
        let started = Instant::now();
        drop(writer);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(
            ServerStorage::lock_for_maintenance(&fixture.root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        gate.release();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Canceled);
        assert_eq!(
            diagnostics.snapshot().writer_drain_outcome(),
            HistoryDrainOutcome::NotRequested
        );
        assert!(diagnostics.snapshot().contains(HistoryReason::WorkCanceled));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::CanceledEndMetadataFinalized));
        wait_for_lock(&fixture.root);
    }

    #[test]
    fn backend_panic_fails_current_receipt_and_closes_writer() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::BlockThenPanic(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);
        let diagnostics = writer.diagnostics();
        let mut receipt = writer
            .try_submit(empty_batch(target, 1, 1))
            .expect("admit batch while startup is gated");
        gate.wait_until_entered();
        gate.release();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Failed);
        assert!(writer.had_storage_error());
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::WorkerFailure));
        assert!(!diagnostics.snapshot().contains(HistoryReason::StorageError));
        assert_eq!(
            writer
                .try_submit(empty_batch(target, 1, 2))
                .unwrap_err()
                .reason(),
            SubmitReason::Failed
        );
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Failed
        );
    }

    #[test]
    fn blocked_backend_panic_fails_current_and_queued_receipts_and_refunds_budget() {
        let fixture = Fixture::new();
        let control = FakeControl::default();
        let gate = Arc::new(Gate::new());
        control.schedule(Call::Load, Behavior::BlockThenPanic(Arc::clone(&gate)));
        let (_storage, mut writer, target) = fake_writer(&fixture, control);

        let mut current = writer
            .try_submit(one_row_batch(target, 1, 1))
            .expect("admit work that blocks before panicking");
        gate.wait_until_entered();
        let mut queued = writer
            .try_submit(one_row_batch(target, 1, 2))
            .expect("queue work behind the blocked batch");
        {
            let state = writer.shared.lock();
            assert_eq!(state.batches, 2);
            assert!(state.payload_bytes > 0);
        }

        gate.release();
        assert_eq!(wait_receipt(&mut current), CommitStatus::Failed);
        assert_eq!(wait_receipt(&mut queued), CommitStatus::Failed);
        assert!(writer.had_storage_error());
        {
            let state = writer.shared.lock();
            assert_eq!(state.batches, 0);
            assert_eq!(state.payload_bytes, 0);
            assert!(state.failed_closed);
        }
        assert_eq!(
            writer
                .try_submit(empty_batch(target, 1, 3))
                .unwrap_err()
                .reason(),
            SubmitReason::Failed
        );
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Failed
        );
    }

    #[test]
    fn dropping_receiver_terminalizes_accepted_work_and_refunds_budget() {
        let fixture = Fixture::new();
        let (_storage, _store, target) = fixture.prepare();
        let shared = Arc::new(Shared {
            state: Mutex::new(SharedState {
                accepting: true,
                cancel: false,
                finalize_canceled_ends: false,
                finalize_reserved_ends: false,
                failed_closed: false,
                batches: 0,
                payload_bytes: 0,
                ends: Vec::new(),
            }),
            had_storage_error: AtomicBool::new(false),
            diagnostics: HistoryDiagnostics::new(),
        });
        let batch = empty_batch(target, 1, 1);
        let bytes = batch.payload_bytes;
        shared.reserve(bytes).expect("reserve accepted work budget");
        let status = Arc::new(AtomicU8::new(STATUS_PENDING));
        let mut receipt = Receipt {
            target,
            epoch: 1,
            token: 1,
            status: Arc::clone(&status),
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(Work {
                batch: Some(batch),
                receipt: status,
                shared: Arc::clone(&shared),
                finalized: false,
            })
            .expect("receiver accepts queued work");

        // A worker panic may race after queue draining but before receiver
        // teardown. Dropping the channel must settle any accepted remainder.
        drop(receiver);

        assert_eq!(receipt.poll(), CommitStatus::Failed);
        let state = shared.lock();
        assert_eq!(state.batches, 0);
        assert_eq!(state.payload_bytes, 0);
        assert!(state.failed_closed);
        drop(state);
        drop(sender);
    }

    struct EndSyncFaultBackend {
        store: HistoryStore,
        calls: Arc<Mutex<Vec<Call>>>,
        inject_end_fault: bool,
    }

    impl HistoryBackend for EndSyncFaultBackend {
        fn load_rows(&mut self, target: TargetRef) -> io::Result<Option<SavedHistory>> {
            self.calls.lock().unwrap().push(Call::Load);
            self.store.load_rows(target)
        }

        fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
            self.calls.lock().unwrap().push(Call::Begin);
            self.store.begin_history(target, epoch, gap)
        }

        fn mark_history(
            &mut self,
            target: TargetRef,
            epoch: u64,
            disabled: bool,
        ) -> io::Result<()> {
            self.calls.lock().unwrap().push(Call::Mark);
            self.store.mark_history(target, epoch, disabled)
        }

        fn append_rows(
            &mut self,
            target: TargetRef,
            rows: &[Vec<u8>],
            gap: bool,
        ) -> io::Result<()> {
            self.calls.lock().unwrap().push(Call::Append);
            self.store.append_rows(target, rows, gap)
        }

        fn save_screen(
            &mut self,
            target: TargetRef,
            buffer: ScreenBuffer,
            screen: &[u8],
        ) -> io::Result<()> {
            self.calls.lock().unwrap().push(Call::Screen);
            self.store.save(target, buffer, screen)
        }

        fn mark_ended(&mut self, target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
            self.calls.lock().unwrap().push(Call::End);
            if self.inject_end_fault {
                self.inject_end_fault = false;
                self.store
                    .inject_test_fault(TestFaultPoint::DirectorySync, 1);
            }
            self.store.mark_ended(target, at_unix_ms)
        }
    }

    #[test]
    fn real_store_end_directory_sync_retry_repeats_only_end_and_keeps_first_time() {
        let fixture = Fixture::new();
        let (storage, store, target) = fixture.prepare();
        storage.verify_history_store(&store).unwrap();
        let expected_rows = row_records(target, 1);
        let expected_screen = screen_record(
            target,
            ScreenBuffer::Normal,
            TerminalSize::new(8, 20).unwrap(),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = EndSyncFaultBackend {
            store,
            calls: Arc::clone(&calls),
            inject_end_fault: true,
        };
        let mut writer = HistoryWriter::start_with_backend(Box::new(backend), &storage).unwrap();
        let mut receipt = writer
            .try_submit(
                HistoryBatch::new(
                    target,
                    1,
                    1,
                    false,
                    expected_rows.clone(),
                    vec![expected_screen.clone()],
                    Some(1000),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(wait_receipt(&mut receipt), CommitStatus::Committed);
        assert!(writer.had_storage_error());
        assert_eq!(
            writer.shutdown(Duration::from_millis(250)).outcome,
            ShutdownOutcome::Drained
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                Call::Load,
                Call::Begin,
                Call::Append,
                Call::Screen,
                Call::End,
                Call::End
            ]
        );
        let reopened = HistoryStore::open(&fixture.history).unwrap();
        let saved = reopened.load_rows(target).unwrap().unwrap();
        assert_eq!(saved.records, expected_rows);
        assert_eq!(saved.ended_at_unix_ms, 1000);
        assert_eq!(
            reopened.load(target, ScreenBuffer::Normal).unwrap(),
            Some(expected_screen)
        );
    }
}
