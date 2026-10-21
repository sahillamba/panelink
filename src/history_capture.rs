//! Bounded owner-thread scheduling for asynchronous saved-history writes.
//!
//! The coordinator owns the only `HistoryWriter` producer. Capture code may
//! encode sanitized records and call `try_submit`, but it never performs a
//! history filesystem operation or waits for a receipt. The worker owns all
//! storage mutation after startup.

use crate::control_protocol::TargetRef;
use crate::history_diagnostics::{HistoryDiagnostics, HistoryReason};
use crate::history_writer::{
    CapacityReason, CommitStatus, HistoryBatch, HistoryWriter, Receipt, ShutdownOutcome,
    SubmitReason,
};
use crate::screen_checkpoint::{
    self, CheckpointMetadata, ScreenBuffer, MAX_SCREEN_CHECKPOINT_BYTES,
};
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use vt100::HistoryCursor;

/// Retained metadata slots. This is independent of the writer's payload quota.
pub(crate) const MAX_CAPTURE_TARGETS: usize = 8;

pub(crate) type SharedHistoryCapture = Arc<Mutex<HistoryCaptureCoordinator>>;

/// A sanitized checkpoint candidate. Its debug output never includes cells.
pub(crate) struct CheckpointRecord {
    pub(crate) target: TargetRef,
    pub(crate) buffer: ScreenBuffer,
    pub(crate) metadata: CheckpointMetadata,
    bytes: Box<[u8]>,
}

impl CheckpointRecord {
    pub(crate) fn new(
        target: TargetRef,
        buffer: ScreenBuffer,
        metadata: CheckpointMetadata,
        bytes: Vec<u8>,
    ) -> io::Result<Self> {
        let decoded = screen_checkpoint::decode(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid screen checkpoint"))?;
        if metadata.target() != target
            || decoded.metadata().target() != target
            || decoded.metadata().output_sequence() != metadata.output_sequence()
            || decoded.metadata().captured_at_unix_ms() != metadata.captured_at_unix_ms()
            || decoded.metadata().screen_revision() != metadata.screen_revision()
            || decoded.buffer() != buffer
            || bytes.len() > MAX_SCREEN_CHECKPOINT_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid screen checkpoint candidate",
            ));
        }
        Ok(Self {
            target,
            buffer,
            metadata,
            bytes: bytes.into_boxed_slice(),
        })
    }

    fn identity(&self) -> ScreenIdentity {
        ScreenIdentity {
            target: self.target,
            buffer: self.buffer,
            output_sequence: self.metadata.output_sequence(),
            revision: self.metadata.screen_revision(),
            captured_at_unix_ms: self.metadata.captured_at_unix_ms(),
        }
    }

    fn clone_bytes(&self) -> Vec<u8> {
        self.bytes.to_vec()
    }
}

impl fmt::Debug for CheckpointRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckpointRecord { <redacted> }")
    }
}

/// One bounded observation taken from a live terminal owner.
pub(crate) struct CaptureObservation {
    pub(crate) cursor: HistoryCursor,
    /// True when the emulator ring reset or skipped rows before this suffix.
    pub(crate) gap: bool,
    /// Sanitized encoded rows after the target's submitted cursor.
    pub(crate) rows: Vec<Vec<u8>>,
    /// At most one latest candidate per observed screen buffer.
    pub(crate) screens: Vec<CheckpointRecord>,
}

impl fmt::Debug for CaptureObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaptureObservation")
            .field("epoch", &self.cursor.epoch)
            .field("ordinal", &self.cursor.ordinal)
            .field("gap", &self.gap)
            .field("rows_len", &self.rows.len())
            .field("screens_len", &self.screens.len())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct ScreenIdentity {
    target: TargetRef,
    buffer: ScreenBuffer,
    output_sequence: u64,
    revision: u64,
    captured_at_unix_ms: u64,
}

struct PendingCommit {
    receipt: Receipt,
    plan: CommitPlan,
}

struct CommitPlan {
    target: TargetRef,
    epoch: u64,
    cursor: HistoryCursor,
    token: u64,
    loss_revision: u64,
    gap: bool,
    screens: Vec<ScreenIdentity>,
}

struct EndIntent {
    epoch: u64,
    /// None is retained as an honest unknown; a timestamp is never invented.
    at_unix_ms: Option<u64>,
    gap: bool,
    pending_metadata: bool,
}

struct TargetCapture {
    target: TargetRef,
    submitted_cursor: HistoryCursor,
    durable_cursor: HistoryCursor,
    durable_token: u64,
    regular: Option<PendingCommit>,
    final_batch: Option<PendingCommit>,
    deferred_screens: [Option<CheckpointRecord>; 2],
    submitted_screens: [Option<(ScreenIdentity, u64)>; 2],
    durable_screens: [Option<(ScreenIdentity, u64)>; 2],
    loss_revision: u64,
    durable_loss_revision: u64,
    last_loss_marker: Option<(u64, u64, u64)>,
    ending: Option<EndIntent>,
    terminal_failure: bool,
    settled: bool,
}

impl TargetCapture {
    fn new(target: TargetRef, initial: HistoryCursor) -> Self {
        let missed_initial = initial.ordinal != 0;
        let cursor = HistoryCursor {
            epoch: initial.epoch,
            ordinal: 0,
        };
        let mut state = Self {
            target,
            submitted_cursor: cursor,
            durable_cursor: cursor,
            durable_token: 0,
            regular: None,
            final_batch: None,
            deferred_screens: [None, None],
            submitted_screens: [None, None],
            durable_screens: [None, None],
            loss_revision: 0,
            durable_loss_revision: 0,
            last_loss_marker: None,
            ending: None,
            terminal_failure: false,
            settled: false,
        };
        if missed_initial {
            state.note_loss(initial.epoch, 0, initial.ordinal);
        }
        state
    }

    fn note_loss(&mut self, epoch: u64, from: u64, through: u64) {
        let marker = (epoch, from, through);
        if self.last_loss_marker == Some(marker) {
            return;
        }
        self.last_loss_marker = Some(marker);
        match self.loss_revision.checked_add(1) {
            Some(revision) => self.loss_revision = revision,
            None => self.terminal_failure = true,
        }
    }

    fn gap_pending(&self) -> bool {
        self.loss_revision > self.durable_loss_revision
    }

    fn deferred(&self) -> Vec<&CheckpointRecord> {
        let mut result = Vec::with_capacity(2);
        for buffer in [ScreenBuffer::Normal, ScreenBuffer::Alternate] {
            if let Some(record) = &self.deferred_screens[buffer_index(buffer)] {
                result.push(record);
            }
        }
        result
    }

    fn merge_screens(&mut self, records: Vec<CheckpointRecord>) {
        for record in records {
            if record.target != self.target {
                self.terminal_failure = true;
                continue;
            }
            let index = buffer_index(record.buffer);
            let identity = record.identity();
            if self.submitted_screens[index]
                .is_some_and(|(saved, _)| !newer_screen(identity, saved))
                || self.durable_screens[index]
                    .is_some_and(|(saved, _)| !newer_screen(identity, saved))
            {
                continue;
            }
            let replace = self.deferred_screens[index]
                .as_ref()
                .is_none_or(|saved| newer_screen(identity, saved.identity()));
            if replace {
                self.deferred_screens[index] = Some(record);
            }
        }
    }

    fn screen_payload(&self) -> Vec<Vec<u8>> {
        self.deferred()
            .into_iter()
            .map(CheckpointRecord::clone_bytes)
            .collect()
    }

    fn screen_identities(&self) -> Vec<ScreenIdentity> {
        self.deferred()
            .into_iter()
            .map(CheckpointRecord::identity)
            .collect()
    }

    fn remove_submitted_screens(&mut self, identities: &[ScreenIdentity], token: u64) {
        for identity in identities {
            let index = buffer_index(identity.buffer);
            self.submitted_screens[index] = Some((*identity, token));
            if self.deferred_screens[index]
                .as_ref()
                .is_some_and(|record| record.identity() == *identity)
            {
                self.deferred_screens[index] = None;
            }
        }
    }

    fn apply_commit(&mut self, plan: CommitPlan) {
        if plan.target != self.target || plan.token <= self.durable_token {
            return;
        }
        self.durable_token = plan.token;
        self.durable_cursor = plan.cursor;
        if plan.gap && plan.loss_revision > self.durable_loss_revision {
            self.durable_loss_revision = plan.loss_revision;
        }
        for identity in plan.screens {
            let index = buffer_index(identity.buffer);
            if self.durable_screens[index].is_none_or(|(_, token)| plan.token > token) {
                self.durable_screens[index] = Some((identity, plan.token));
            }
        }
    }
}

fn buffer_index(buffer: ScreenBuffer) -> usize {
    match buffer {
        ScreenBuffer::Normal => 0,
        ScreenBuffer::Alternate => 1,
    }
}

fn newer_screen(candidate: ScreenIdentity, previous: ScreenIdentity) -> bool {
    (
        candidate.output_sequence,
        candidate.revision,
        candidate.captured_at_unix_ms,
    ) > (
        previous.output_sequence,
        previous.revision,
        previous.captured_at_unix_ms,
    )
}

/// The one owner-thread producer for a server's bounded history writer.
pub struct HistoryCaptureCoordinator {
    writer: Option<HistoryWriter>,
    failure: Arc<AtomicBool>,
    diagnostics: HistoryDiagnostics,
    targets: Vec<TargetCapture>,
    next_token: u64,
    accepting: bool,
    closing: bool,
    storage_error_seen: bool,
}

impl fmt::Debug for HistoryCaptureCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryCaptureCoordinator")
            .field("registered_targets", &self.targets.len())
            .field("accepting", &self.accepting)
            .field("closing", &self.closing)
            .finish()
    }
}

impl HistoryCaptureCoordinator {
    pub(crate) fn new(writer: HistoryWriter, failure: Arc<AtomicBool>) -> Self {
        let diagnostics = writer.diagnostics();
        Self {
            writer: Some(writer),
            failure,
            diagnostics,
            targets: Vec::with_capacity(MAX_CAPTURE_TARGETS),
            next_token: 0,
            accepting: true,
            closing: false,
            storage_error_seen: false,
        }
    }

    /// Register a full target identity and reserve its final metadata slot.
    pub(crate) fn register_target(&mut self, target: TargetRef, initial: HistoryCursor) -> bool {
        if !self.accepting
            || self.writer.is_none()
            || initial.epoch == 0
            || self.target_index(target).is_some()
        {
            self.fail();
            return false;
        }
        if self.targets.len() >= MAX_CAPTURE_TARGETS {
            self.fail_with(HistoryReason::CaptureRegistrationFull);
            return false;
        }
        self.targets.push(TargetCapture::new(target, initial));
        true
    }

    pub(crate) fn diagnostics(&self) -> HistoryDiagnostics {
        self.diagnostics.clone()
    }

    /// Current accepted row cursor. Rejected batches never advance it.
    pub(crate) fn submitted_cursor(&self, target: TargetRef) -> Option<HistoryCursor> {
        self.targets
            .iter()
            .find(|state| state.target == target)
            .map(|state| state.submitted_cursor)
    }

    /// Last row cursor acknowledged durable by its matching receipt.
    #[cfg(test)]
    pub(crate) fn durable_cursor(&self, target: TargetRef) -> Option<HistoryCursor> {
        self.targets
            .iter()
            .find(|state| state.target == target)
            .map(|state| state.durable_cursor)
    }

    pub(crate) fn has_regular_pending(&self, target: TargetRef) -> bool {
        self.targets
            .iter()
            .find(|state| state.target == target)
            .is_some_and(|state| state.regular.is_some())
    }

    /// Whether an ordinary candidate can still be accepted for this target.
    /// Sticky recoverable I/O errors do not close the target; failed receipts do.
    pub(crate) fn can_capture(&self, target: TargetRef) -> bool {
        self.targets
            .iter()
            .find(|state| state.target == target)
            .is_some_and(|state| {
                self.accepting
                    && !self.closing
                    && !state.terminal_failure
                    && !state.settled
                    && state.ending.is_none()
                    && self.writer.is_some()
            })
    }

    /// Coalesce bounded screens and try one ordinary batch for this target.
    /// Rejection drops rows; the engine is sampled again on the next owner turn.
    pub(crate) fn observe(&mut self, target: TargetRef, observation: CaptureObservation) {
        let Some(index) = self.target_index(target) else {
            self.fail();
            return;
        };
        let state = &mut self.targets[index];
        state.merge_screens(observation.screens);
        if state.terminal_failure {
            self.fail();
            return;
        }
        if observation.gap {
            state.note_loss(
                observation.cursor.epoch,
                state.submitted_cursor.ordinal,
                observation.cursor.ordinal,
            );
        }
        if state.terminal_failure {
            self.fail();
            return;
        }
        if !self.accepting || self.closing || state.ending.is_some() || state.terminal_failure {
            return;
        }
        if state.regular.is_some() || state.settled {
            return;
        }
        if observation.cursor.epoch == 0
            || observation.cursor.epoch < state.submitted_cursor.epoch
            || (observation.cursor.epoch == state.submitted_cursor.epoch
                && observation.cursor.ordinal < state.submitted_cursor.ordinal)
        {
            state.terminal_failure = true;
            self.fail();
            return;
        }
        let gap = state.gap_pending();
        let screens = state.screen_payload();
        let identities = state.screen_identities();
        if observation.rows.is_empty()
            && screens.is_empty()
            && !gap
            && observation.cursor == state.submitted_cursor
        {
            return;
        }
        let Some(token) = self.next_token.checked_add(1) else {
            state.terminal_failure = true;
            self.fail();
            return;
        };
        let loss_revision = state.loss_revision;
        let batch = match HistoryBatch::new(
            target,
            observation.cursor.epoch,
            token,
            gap,
            observation.rows,
            screens,
            None,
        ) {
            Ok(batch) => batch,
            Err(_) => {
                state.note_loss(
                    observation.cursor.epoch,
                    state.submitted_cursor.ordinal,
                    observation.cursor.ordinal,
                );
                self.fail();
                return;
            }
        };
        let result = self
            .writer
            .as_mut()
            .expect("accepting coordinator has a writer")
            .try_submit(batch);
        match result {
            Ok(receipt) => {
                self.next_token = token;
                let plan = CommitPlan {
                    target,
                    epoch: observation.cursor.epoch,
                    cursor: observation.cursor,
                    token,
                    loss_revision,
                    gap,
                    screens: identities.clone(),
                };
                let state = &mut self.targets[index];
                state.submitted_cursor = observation.cursor;
                state.remove_submitted_screens(&identities, token);
                state.regular = Some(PendingCommit { receipt, plan });
            }
            Err(error) if error.reason() == SubmitReason::Full => {
                // Screen bytes remain coalesced in the target slot. Rows are
                // deliberately not retained outside the writer quota.
            }
            Err(_) => {
                self.targets[index].terminal_failure = true;
                self.fail();
            }
        }
    }

    /// Retain the first known end time and queue final data or a reserved gap/end.
    /// This is idempotent after either accepted final work or an unknown time.
    pub(crate) fn finish_target(
        &mut self,
        target: TargetRef,
        mut observation: CaptureObservation,
        end_at_unix_ms: Option<u64>,
    ) -> bool {
        let Some(index) = self.target_index(target) else {
            self.fail();
            return false;
        };
        self.targets[index].merge_screens(std::mem::take(&mut observation.screens));
        if self.targets[index].terminal_failure {
            self.fail();
            return false;
        }
        if observation.gap {
            let from = self.targets[index].submitted_cursor.ordinal;
            self.targets[index].note_loss(
                observation.cursor.epoch,
                from,
                observation.cursor.ordinal,
            );
        }
        if self.targets[index].terminal_failure {
            self.fail();
            return false;
        }
        if self.targets[index].ending.is_some() {
            return true;
        }
        let unknown_end = end_at_unix_ms.is_none_or(|at| at == 0);
        if unknown_end {
            self.fail_with(HistoryReason::UnknownEndTime);
        }
        let gap = self.targets[index].gap_pending() || unknown_end;
        self.targets[index].ending = Some(EndIntent {
            epoch: observation
                .cursor
                .epoch
                .max(self.targets[index].submitted_cursor.epoch),
            at_unix_ms: end_at_unix_ms.filter(|at| *at != 0),
            gap,
            pending_metadata: true,
        });
        if observation.cursor.epoch == 0
            || observation.cursor.epoch < self.targets[index].submitted_cursor.epoch
        {
            self.targets[index].terminal_failure = true;
            self.fail();
            return false;
        }
        self.submit_final(index, observation);
        true
    }

    /// Record a known final-output loss without ending a live/detached session.
    pub(crate) fn mark_gap(&mut self, target: TargetRef, epoch: u64, through: u64) {
        if let Some(index) = self.target_index(target) {
            let state = &mut self.targets[index];
            state.note_loss(epoch, state.submitted_cursor.ordinal, through);
            if state.terminal_failure {
                self.fail();
            }
        } else {
            self.fail();
        }
    }

    /// Poll receipts and retry reserved end metadata, independent of pane polls.
    pub(crate) fn drive(&mut self) {
        self.note_storage_error();
        for state in &mut self.targets {
            Self::poll_one(state, true, &self.failure, &self.diagnostics);
            Self::poll_one(state, false, &self.failure, &self.diagnostics);
        }
        let mut index = 0;
        while index < self.targets.len() {
            let should_submit = self.targets[index]
                .ending
                .as_ref()
                .is_some_and(|end| end.pending_metadata)
                && self.targets[index].final_batch.is_none()
                && !self.targets[index].terminal_failure;
            if should_submit {
                self.submit_final_metadata(index);
            }
            index += 1;
        }
        self.targets.retain(|state| !state.settled);
        // A successful receipt can publish after the first check but before
        // its matching poll. Observe the writer flag again after receipts so
        // shutdown cannot settle every target and miss a recovered I/O error.
        self.note_storage_error();
    }

    fn note_storage_error(&mut self) {
        if self
            .writer
            .as_ref()
            .is_some_and(HistoryWriter::had_storage_error)
            && !self.storage_error_seen
        {
            self.storage_error_seen = true;
            self.failure.store(true, Ordering::Release);
        }
    }

    /// Stop accepting observations before the driver performs its unlocked
    /// receipt-drain loop.
    pub(crate) fn begin_shutdown(&mut self) {
        self.accepting = false;
        self.closing = true;
    }

    pub(crate) fn all_targets_settled(&self) -> bool {
        self.targets.is_empty()
    }

    /// Record only work that was still outstanding when the driver's one
    /// shared drain deadline was reached.
    pub(crate) fn note_drain_deadline(&mut self) {
        let mut pending_receipt = false;
        let mut unsubmitted_end = false;
        for state in &mut self.targets {
            if let Some(pending) = state.regular.as_mut() {
                pending_receipt |= pending.receipt.poll() == CommitStatus::Pending;
            }
            if let Some(pending) = state.final_batch.as_mut() {
                pending_receipt |= pending.receipt.poll() == CommitStatus::Pending;
            }
            unsubmitted_end |= state
                .ending
                .as_ref()
                .is_some_and(|end| end.pending_metadata)
                && state.final_batch.is_none();
        }
        if pending_receipt {
            self.diagnostics
                .record(HistoryReason::PendingReceiptsAtDeadline);
        }
        if unsubmitted_end {
            self.diagnostics
                .record(HistoryReason::FinalMetadataUnsubmitted);
        }
    }

    /// Take the writer after receipt polling has ended. The caller must shut
    /// it down without holding the shared coordinator mutex.
    pub(crate) fn take_writer_for_shutdown(&mut self) -> Option<HistoryWriter> {
        self.accepting = false;
        self.closing = true;
        if !self.targets.is_empty() {
            self.failure.store(true, Ordering::Release);
        }
        self.writer.take()
    }

    pub(crate) fn note_shutdown_outcome(&self, outcome: ShutdownOutcome) {
        if outcome != ShutdownOutcome::Drained {
            self.failure.store(true, Ordering::Release);
        }
    }

    fn submit_final(&mut self, index: usize, observation: CaptureObservation) {
        let target = self.targets[index].target;
        let mut epoch = observation.cursor.epoch;
        if epoch == 0 || epoch < self.targets[index].submitted_cursor.epoch {
            epoch = self.targets[index].submitted_cursor.epoch;
            self.targets[index].terminal_failure = true;
            self.fail();
        }
        let cursor = if observation.cursor.epoch == epoch {
            observation.cursor
        } else {
            self.targets[index].submitted_cursor
        };
        let rows = observation.rows;
        let screens = self.targets[index].screen_payload();
        let identities = self.targets[index].screen_identities();
        let end_at = self.targets[index]
            .ending
            .as_ref()
            .and_then(|end| end.at_unix_ms);
        let gap = self.targets[index]
            .ending
            .as_ref()
            .is_some_and(|end| end.gap)
            || self.targets[index].gap_pending();
        let Some(token) = self.next_token.checked_add(1) else {
            self.fail();
            self.targets[index].terminal_failure = true;
            return;
        };
        let batch = HistoryBatch::new(target, epoch, token, gap, rows, screens, end_at);
        let batch = match batch {
            Ok(batch) => batch,
            Err(_) => {
                self.final_payload_lost(
                    index,
                    epoch,
                    cursor.ordinal,
                    HistoryReason::FinalPayloadInvalid,
                );
                self.submit_final_metadata(index);
                return;
            }
        };
        let plan = CommitPlan {
            target,
            epoch,
            cursor,
            token,
            loss_revision: self.targets[index].loss_revision,
            gap,
            screens: identities,
        };
        let result = self
            .writer
            .as_mut()
            .filter(|_| self.accepting || self.closing)
            .map(|writer| writer.try_submit(batch));
        match result {
            Some(Ok(receipt)) => {
                self.final_payload_submitted(index, plan, receipt);
            }
            Some(Err(error)) if error.reason() == SubmitReason::Full => {
                let capacity_reason = error.capacity_reason();
                if capacity_reason == Some(CapacityReason::Bytes) {
                    let rejected = error.into_batch();
                    let available = self
                        .writer
                        .as_ref()
                        .filter(|_| self.accepting || self.closing)
                        .and_then(HistoryWriter::available_payload_bytes);
                    let Some(available) = available else {
                        self.final_payload_lost(
                            index,
                            epoch,
                            cursor.ordinal,
                            HistoryReason::FinalPayloadDroppedBytes,
                        );
                        self.submit_final_metadata(index);
                        return;
                    };
                    let Some((candidate, removed_rows, retained_rows)) =
                        rejected.retain_newest_suffix_within(available)
                    else {
                        self.final_payload_lost(
                            index,
                            epoch,
                            cursor.ordinal,
                            HistoryReason::FinalPayloadDroppedBytes,
                        );
                        self.submit_final_metadata(index);
                        return;
                    };

                    if removed_rows > 0 {
                        self.final_prefix_lost(index, epoch, cursor.ordinal);
                    }
                    let mut retry_plan = plan;
                    retry_plan.cursor = if retained_rows {
                        retry_plan.cursor
                    } else if self.targets[index].submitted_cursor.epoch == epoch {
                        self.targets[index].submitted_cursor
                    } else {
                        HistoryCursor { epoch, ordinal: 0 }
                    };
                    retry_plan.gap |= removed_rows > 0;
                    retry_plan.loss_revision = self.targets[index].loss_revision;
                    let retry_result = self
                        .writer
                        .as_mut()
                        .filter(|_| self.accepting || self.closing)
                        .map(|writer| writer.try_submit(candidate));
                    match retry_result {
                        Some(Ok(receipt)) => {
                            self.final_payload_submitted(index, retry_plan, receipt)
                        }
                        Some(Err(error)) if error.reason() == SubmitReason::Full => {
                            self.final_payload_lost(
                                index,
                                epoch,
                                cursor.ordinal,
                                final_drop_reason(error.capacity_reason()),
                            );
                            self.submit_final_metadata(index);
                        }
                        Some(Err(_)) | None => {
                            self.fail();
                            self.targets[index].terminal_failure = true;
                        }
                    }
                } else {
                    self.final_payload_lost(
                        index,
                        epoch,
                        cursor.ordinal,
                        final_drop_reason(capacity_reason),
                    );
                    self.submit_final_metadata(index);
                }
            }
            Some(Err(_)) | None => {
                self.fail();
                self.targets[index].terminal_failure = true;
            }
        }
    }

    fn final_payload_submitted(&mut self, index: usize, plan: CommitPlan, receipt: Receipt) {
        self.next_token = plan.token;
        let state = &mut self.targets[index];
        state.submitted_cursor = plan.cursor;
        state.remove_submitted_screens(&plan.screens, plan.token);
        if let Some(end) = state.ending.as_mut() {
            end.pending_metadata = false;
        }
        state.final_batch = Some(PendingCommit { receipt, plan });
    }

    fn final_prefix_lost(&mut self, index: usize, epoch: u64, through: u64) {
        self.fail_with(HistoryReason::FinalPayloadDroppedBytes);
        let state = &mut self.targets[index];
        let from = if state.submitted_cursor.epoch == epoch {
            state.submitted_cursor.ordinal
        } else {
            0
        };
        state.note_loss(epoch, from, through);
        if let Some(end) = state.ending.as_mut() {
            end.epoch = epoch;
            end.gap = true;
        }
    }

    fn final_payload_lost(
        &mut self,
        index: usize,
        epoch: u64,
        through: u64,
        reason: HistoryReason,
    ) {
        self.fail_with(reason);
        let state = &mut self.targets[index];
        let from = state.submitted_cursor.ordinal;
        state.note_loss(epoch, from, through);
        state.deferred_screens = [None, None];
        if let Some(end) = state.ending.as_mut() {
            end.epoch = epoch;
            end.gap = true;
            end.pending_metadata = true;
        }
    }

    fn submit_final_metadata(&mut self, index: usize) {
        let target = self.targets[index].target;
        let Some(end) = self.targets[index].ending.as_ref() else {
            return;
        };
        let epoch = end.epoch;
        let at = end.at_unix_ms;
        let gap = end.gap || self.targets[index].gap_pending();
        // A metadata-only reset can move into a new epoch after the final
        // payload was lost. Never report the old epoch's ordinal as durable
        // in the new epoch.
        let cursor = if self.targets[index].submitted_cursor.epoch == epoch {
            self.targets[index].submitted_cursor
        } else {
            HistoryCursor { epoch, ordinal: 0 }
        };
        let Some(token) = self.next_token.checked_add(1) else {
            self.fail();
            self.targets[index].terminal_failure = true;
            return;
        };
        let loss_revision = self.targets[index].loss_revision;
        let batch = match HistoryBatch::new(target, epoch, token, gap, Vec::new(), Vec::new(), at) {
            Ok(batch) => batch,
            Err(_) => {
                self.fail_with(HistoryReason::FinalPayloadInvalid);
                self.targets[index].terminal_failure = true;
                return;
            }
        };
        let result = self
            .writer
            .as_mut()
            .filter(|_| self.accepting || self.closing)
            .map(|writer| writer.try_submit(batch));
        match result {
            Some(Ok(receipt)) => {
                self.next_token = token;
                let state = &mut self.targets[index];
                state.submitted_cursor = cursor;
                if let Some(end) = state.ending.as_mut() {
                    end.pending_metadata = false;
                }
                state.final_batch = Some(PendingCommit {
                    receipt,
                    plan: CommitPlan {
                        target,
                        epoch,
                        cursor,
                        token,
                        loss_revision,
                        gap,
                        screens: Vec::new(),
                    },
                });
            }
            Some(Err(error)) if error.reason() == SubmitReason::Full => {}
            Some(Err(_)) | None => {
                self.fail();
                self.targets[index].terminal_failure = true;
            }
        }
    }

    fn poll_one(
        state: &mut TargetCapture,
        regular: bool,
        failure: &AtomicBool,
        diagnostics: &HistoryDiagnostics,
    ) {
        let mut commit = if regular {
            state.regular.take()
        } else {
            state.final_batch.take()
        };
        let Some(mut commit) = commit.take() else {
            return;
        };
        match commit.receipt.poll() {
            CommitStatus::Pending => {
                if regular {
                    state.regular = Some(commit);
                } else {
                    state.final_batch = Some(commit);
                }
            }
            CommitStatus::Committed => {
                if commit.receipt.target() != commit.plan.target
                    || commit.receipt.epoch() != commit.plan.epoch
                    || commit.receipt.token() != commit.plan.token
                {
                    state.terminal_failure = true;
                    diagnostics.record(HistoryReason::ReceiptFailed);
                    failure.store(true, Ordering::Release);
                } else {
                    state.apply_commit(commit.plan.clone());
                    if !regular {
                        state.settled = true;
                    }
                }
            }
            CommitStatus::Failed | CommitStatus::Canceled => {
                // End metadata may have been finalized after a timed-out
                // shutdown, but this receipt does not certify payload
                // completion and must never advance the durable cursor.
                state.terminal_failure = true;
                diagnostics.record(HistoryReason::ReceiptFailed);
                failure.store(true, Ordering::Release);
            }
        }
    }

    fn target_index(&self, target: TargetRef) -> Option<usize> {
        self.targets.iter().position(|state| state.target == target)
    }

    fn fail(&self) {
        self.fail_with(HistoryReason::CaptureInvalid);
    }

    fn fail_with(&self, reason: HistoryReason) {
        self.diagnostics.record(reason);
        self.failure.store(true, Ordering::Release);
    }
}

fn final_drop_reason(reason: Option<CapacityReason>) -> HistoryReason {
    match reason {
        Some(CapacityReason::Contended) => HistoryReason::FinalPayloadDroppedContended,
        Some(CapacityReason::Count) => HistoryReason::FinalPayloadDroppedCount,
        Some(CapacityReason::Bytes) => HistoryReason::FinalPayloadDroppedBytes,
        None => HistoryReason::CaptureInvalid,
    }
}

impl Clone for CommitPlan {
    fn clone(&self) -> Self {
        Self {
            target: self.target,
            epoch: self.epoch,
            cursor: self.cursor,
            token: self.token,
            loss_revision: self.loss_revision,
            gap: self.gap,
            screens: self.screens.clone(),
        }
    }
}

pub(crate) fn lock_capture(
    shared: &SharedHistoryCapture,
) -> MutexGuard<'_, HistoryCaptureCoordinator> {
    shared.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_diagnostics::HistoryDrainOutcome;
    use crate::history_rows;
    use crate::history_store::SavedHistory;
    use crate::history_writer::{
        HistoryBackend, MAX_BATCH_ROWS, MAX_QUEUED_PAYLOAD_BYTES, MAX_SHUTDOWN_WAIT,
    };
    use crate::server_storage::ServerStorage;
    use crate::terminal::{ScreenSnapshot, TerminalSize};
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    const TARGET: TargetRef = TargetRef {
        generation: 1,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    };

    struct Fixture {
        root: PathBuf,
        storage: Option<ServerStorage>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from("/private/tmp").join(format!(
                "panelink-history-capture-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .expect("create private fixture");
            let server = root.join("server");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&server)
                .expect("create server directory");
            let storage = ServerStorage::initialize(&server).expect("initialize storage");
            Self {
                root,
                storage: Some(storage),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.storage.take());
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Event {
        Began(TargetRef, u64, bool),
        Ended(TargetRef, u64),
    }

    struct GatedBackend {
        entered: SyncSender<(TargetRef, u64, bool)>,
        release: Receiver<()>,
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl HistoryBackend for GatedBackend {
        fn load_rows(&mut self, _target: TargetRef) -> io::Result<Option<SavedHistory>> {
            Ok(None)
        }

        fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(Event::Began(target, epoch, gap));
            let _ = self.entered.send((target, epoch, gap));
            self.release
                .recv()
                .map_err(|_| io::Error::other("test writer gate released"))
        }

        fn mark_history(
            &mut self,
            _target: TargetRef,
            _epoch: u64,
            _disabled: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn append_rows(
            &mut self,
            _target: TargetRef,
            _rows: &[Vec<u8>],
            _gap: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn save_screen(
            &mut self,
            _target: TargetRef,
            _buffer: ScreenBuffer,
            _screen: &[u8],
        ) -> io::Result<()> {
            Ok(())
        }

        fn mark_ended(&mut self, target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(Event::Ended(target, at_unix_ms));
            Ok(())
        }

        fn finalize_incomplete_end(
            &mut self,
            target: TargetRef,
            _epoch: u64,
            at_unix_ms: u64,
        ) -> io::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(Event::Ended(target, at_unix_ms));
            Ok(())
        }
    }

    struct RecordingBackend(Arc<Mutex<Vec<Event>>>);

    impl HistoryBackend for RecordingBackend {
        fn load_rows(&mut self, _target: TargetRef) -> io::Result<Option<SavedHistory>> {
            Ok(None)
        }

        fn begin_history(&mut self, target: TargetRef, epoch: u64, gap: bool) -> io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(Event::Began(target, epoch, gap));
            Ok(())
        }

        fn mark_history(
            &mut self,
            _target: TargetRef,
            _epoch: u64,
            _disabled: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn append_rows(
            &mut self,
            _target: TargetRef,
            _rows: &[Vec<u8>],
            _gap: bool,
        ) -> io::Result<()> {
            Ok(())
        }

        fn save_screen(
            &mut self,
            _target: TargetRef,
            _buffer: ScreenBuffer,
            _screen: &[u8],
        ) -> io::Result<()> {
            Ok(())
        }

        fn mark_ended(&mut self, target: TargetRef, at_unix_ms: u64) -> io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(Event::Ended(target, at_unix_ms));
            Ok(())
        }
    }

    fn observation(epoch: u64, ordinal: u64) -> CaptureObservation {
        CaptureObservation {
            cursor: HistoryCursor { epoch, ordinal },
            gap: false,
            rows: Vec::new(),
            screens: Vec::new(),
        }
    }

    fn maximum_rows(target: TargetRef, epoch: u64, count: usize) -> Vec<Vec<u8>> {
        let mut parser = vt100::Parser::new(1, 240, 2048);
        let cursor = parser.screen().history_cursor().expect("initial cursor");
        let mut output = Vec::with_capacity((count + 1) * 242);
        for _ in 0..count + 1 {
            output.extend(std::iter::repeat_n(b'x', 240));
            output.extend_from_slice(b"\r\n");
        }
        parser.process(&output);
        let rows = parser
            .screen()
            .history_after(cursor)
            .expect("history after cursor")
            .map(|row| history_rows::encode(target, epoch, row, 456).expect("encode row"))
            .take(count)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), count, "fixture has requested rows");
        assert!(rows
            .iter()
            .all(|record| record.len() == history_rows::MAX_ROW_BYTES));
        rows
    }

    fn maximum_screen(target: TargetRef, buffer: ScreenBuffer) -> CheckpointRecord {
        let size = TerminalSize::new(80, 240).unwrap();
        let snapshot = ScreenSnapshot::try_new(size).unwrap();
        let metadata = CheckpointMetadata::new(target, 1, 456, 2).unwrap();
        let mut bytes = screen_checkpoint::encode(&snapshot, &metadata).unwrap();
        bytes[80] = match buffer {
            ScreenBuffer::Normal => 0,
            ScreenBuffer::Alternate => 1,
        };
        CheckpointRecord::new(target, buffer, metadata, bytes).unwrap()
    }

    fn wait_for(deadline: Instant, mut condition: impl FnMut() -> bool, message: &str) {
        while !condition() {
            assert!(Instant::now() < deadline, "{message}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn submitted_and_durable_cursors_follow_receipts_across_epoch_reset() {
        let fixture = Fixture::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_send, entered_recv) = mpsc::sync_channel(2);
        let (release_send, release_recv) = mpsc::sync_channel(2);
        let writer = HistoryWriter::start_with_backend(
            Box::new(GatedBackend {
                entered: entered_send,
                release: release_recv,
                events: Arc::clone(&events),
            }),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        assert!(capture.register_target(
            TARGET,
            HistoryCursor {
                epoch: 1,
                ordinal: 0
            }
        ));

        capture.observe(TARGET, observation(1, 5));
        assert_eq!(
            entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
            (TARGET, 1, false)
        );
        assert_eq!(
            capture.submitted_cursor(TARGET),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 5
            })
        );
        assert_eq!(
            capture.durable_cursor(TARGET),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 0
            })
        );

        assert!(capture.finish_target(TARGET, observation(2, 3), Some(12_345)));
        assert!(
            capture.target_index(TARGET).is_some(),
            "pending final work retains its slot"
        );
        assert_eq!(
            capture.submitted_cursor(TARGET),
            Some(HistoryCursor {
                epoch: 2,
                ordinal: 3
            })
        );
        assert_eq!(
            capture.durable_cursor(TARGET),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 0
            })
        );

        release_send.send(()).unwrap();
        assert_eq!(
            entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
            (TARGET, 2, false)
        );
        let first_cursor = HistoryCursor {
            epoch: 1,
            ordinal: 5,
        };
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                capture.drive();
                capture.durable_cursor(TARGET) == Some(first_cursor)
            },
            "first matching receipt did not advance durable cursor",
        );
        assert_eq!(
            capture.submitted_cursor(TARGET),
            Some(HistoryCursor {
                epoch: 2,
                ordinal: 3
            })
        );
        assert_eq!(capture.durable_cursor(TARGET), Some(first_cursor));

        release_send.send(()).unwrap();
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                capture.drive();
                capture.targets.is_empty()
            },
            "final epoch receipt did not settle target",
        );
        let events = events.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            [
                Event::Began(TARGET, 1, false),
                Event::Began(TARGET, 2, false),
                Event::Ended(TARGET, 12_345),
            ]
        );
        assert!(capture.target_index(TARGET).is_none());
        assert!(capture.register_target(
            TARGET,
            HistoryCursor {
                epoch: 3,
                ordinal: 0
            }
        ));
        assert!(!failure.load(Ordering::Acquire));
        capture.targets.clear();
        drop(events);
        capture.begin_shutdown();
        let writer = capture.take_writer_for_shutdown().unwrap();
        let outcome = writer.shutdown(MAX_SHUTDOWN_WAIT).outcome;
        capture.note_shutdown_outcome(outcome);
        assert_eq!(outcome, ShutdownOutcome::Drained);
        drop(fixture);
    }

    #[test]
    fn canceled_final_receipt_does_not_advance_durable_cursor_after_end_finalization() {
        let fixture = Fixture::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_send, entered_recv) = mpsc::sync_channel(1);
        let (release_send, release_recv) = mpsc::sync_channel(1);
        let writer = HistoryWriter::start_with_backend(
            Box::new(GatedBackend {
                entered: entered_send,
                release: release_recv,
                events: Arc::clone(&events),
            }),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        let diagnostics = capture.diagnostics();
        let initial = HistoryCursor {
            epoch: 1,
            ordinal: 0,
        };
        let submitted = HistoryCursor {
            epoch: 1,
            ordinal: 10,
        };
        assert!(capture.register_target(TARGET, initial));
        assert!(capture.finish_target(TARGET, observation(1, 10), Some(12_345)));
        assert_eq!(
            entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
            (TARGET, 1, false)
        );
        assert_eq!(capture.submitted_cursor(TARGET), Some(submitted));
        assert_eq!(capture.durable_cursor(TARGET), Some(initial));

        capture.begin_shutdown();
        let writer = capture.take_writer_for_shutdown().unwrap();
        let outcome = writer.shutdown(Duration::from_millis(10)).outcome;
        assert_eq!(outcome, ShutdownOutcome::TimedOut);
        release_send.send(()).unwrap();
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                capture.drive();
                let snapshot = diagnostics.snapshot();
                snapshot.contains(HistoryReason::CanceledEndMetadataFinalized)
                    && snapshot.contains(HistoryReason::ReceiptFailed)
            },
            "canceled final receipt did not report end metadata and cancellation",
        );
        capture.note_shutdown_outcome(outcome);
        assert_eq!(capture.durable_cursor(TARGET), Some(initial));
        assert_eq!(capture.submitted_cursor(TARGET), Some(submitted));
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.contains(HistoryReason::CanceledEndMetadataFinalized));
        assert!(snapshot.contains(HistoryReason::ReceiptFailed));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
        assert!(failure.load(Ordering::Acquire));
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [Event::Began(TARGET, 1, false), Event::Ended(TARGET, 12_345)]
        );
        capture.targets.clear();
        drop(fixture);
    }

    #[test]
    fn rejected_final_payload_is_replaced_by_gap_and_end_metadata() {
        let fixture = Fixture::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let writer = HistoryWriter::start_with_backend(
            Box::new(RecordingBackend(Arc::clone(&events))),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        assert!(capture.register_target(
            TARGET,
            HistoryCursor {
                epoch: 1,
                ordinal: 0
            }
        ));

        let mut final_observation = observation(1, 1);
        final_observation.rows.push(vec![0]);
        assert!(capture.finish_target(TARGET, final_observation, Some(54_321)));
        assert!(failure.load(Ordering::Acquire));
        assert!(capture
            .diagnostics()
            .snapshot()
            .contains(HistoryReason::FinalPayloadInvalid));
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                capture.drive();
                capture.targets.is_empty()
            },
            "metadata-only final batch did not settle target",
        );
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [Event::Began(TARGET, 1, true), Event::Ended(TARGET, 54_321)]
        );
        capture.begin_shutdown();
        let writer = capture.take_writer_for_shutdown().unwrap();
        let outcome = writer.shutdown(MAX_SHUTDOWN_WAIT).outcome;
        capture.note_shutdown_outcome(outcome);
        assert_eq!(outcome, ShutdownOutcome::Drained);
        drop(fixture);
    }

    #[test]
    fn full_writer_payload_quota_preserves_reserved_final_gap_and_first_end_time() {
        let fixture = Fixture::new();
        let first = TARGET;
        let second = TargetRef {
            pane: 2,
            session: 2,
            ..TARGET
        };
        let final_target = TargetRef {
            pane: 3,
            session: 3,
            ..TARGET
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_send, entered_recv) = mpsc::sync_channel(5);
        let (release_send, release_recv) = mpsc::sync_channel(5);
        let writer = HistoryWriter::start_with_backend(
            Box::new(GatedBackend {
                entered: entered_send,
                release: release_recv,
                events: Arc::clone(&events),
            }),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        for target in [first, second, final_target] {
            assert!(capture.register_target(
                target,
                HistoryCursor {
                    epoch: 1,
                    ordinal: 0
                }
            ));
        }

        let first_rows = maximum_rows(first, 1, MAX_BATCH_ROWS);
        let second_rows = maximum_rows(second, 1, MAX_BATCH_ROWS);
        let final_rows = maximum_rows(final_target, 1, 900);
        let two_regular_payloads = first_rows
            .iter()
            .chain(&second_rows)
            .map(Vec::len)
            .sum::<usize>();
        let requested_final_payload = final_rows.iter().map(Vec::len).sum::<usize>();
        assert!(two_regular_payloads < MAX_QUEUED_PAYLOAD_BYTES);
        assert!(two_regular_payloads + requested_final_payload > MAX_QUEUED_PAYLOAD_BYTES);

        capture.observe(
            first,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: MAX_BATCH_ROWS as u64,
                },
                gap: false,
                rows: first_rows,
                screens: Vec::new(),
            },
        );
        assert_eq!(
            entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
            (first, 1, false)
        );
        capture.observe(
            second,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: MAX_BATCH_ROWS as u64,
                },
                gap: false,
                rows: second_rows,
                screens: Vec::new(),
            },
        );
        capture.note_drain_deadline();
        let diagnostics = capture.diagnostics();
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::PendingReceiptsAtDeadline));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::FinalMetadataUnsubmitted));
        // Ordinary rejection coalesces later and is not a capture failure.
        capture.observe(
            final_target,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: 900,
                },
                gap: false,
                rows: final_rows.clone(),
                screens: Vec::new(),
            },
        );
        assert!(!failure.load(Ordering::Acquire));
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionBytesFull));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedBytes));
        capture.finish_target(first, observation(1, MAX_BATCH_ROWS as u64), Some(10_001));
        capture.finish_target(second, observation(1, MAX_BATCH_ROWS as u64), Some(10_002));
        assert!(capture.finish_target(
            final_target,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: 900
                },
                gap: false,
                rows: final_rows,
                screens: Vec::new(),
            },
            Some(10_003),
        ));
        // Repeated close reports cannot replace the first retained end time.
        assert!(capture.finish_target(final_target, observation(1, 900), Some(99_999)));
        assert!(failure.load(Ordering::Acquire));
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedBytes));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedCount));
        assert_eq!(
            capture.submitted_cursor(final_target),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 900
            })
        );
        assert_eq!(
            capture.durable_cursor(final_target),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 0
            }),
            "admission advances the cursor before its receipt is durable"
        );

        let expected_begins = [
            (second, 1, false),
            (first, 1, false),
            (second, 1, false),
            (final_target, 1, true),
        ];
        for expected in expected_begins {
            release_send.send(()).unwrap();
            assert_eq!(
                entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
                expected
            );
        }
        release_send.send(()).unwrap();
        wait_for(
            Instant::now() + Duration::from_secs(5),
            || {
                capture.drive();
                capture.targets.is_empty()
            },
            "reserved final metadata did not settle after the writer drained",
        );
        let events = events.lock().unwrap();
        assert!(events.contains(&Event::Began(final_target, 1, true)));
        assert!(events.contains(&Event::Ended(final_target, 10_003)));
        assert!(!events.contains(&Event::Ended(final_target, 99_999)));
        drop(events);
        capture.begin_shutdown();
        let writer = capture.take_writer_for_shutdown().unwrap();
        let outcome = writer.shutdown(MAX_SHUTDOWN_WAIT).outcome;
        capture.note_shutdown_outcome(outcome);
        assert_eq!(outcome, ShutdownOutcome::Drained);
        drop(fixture);
    }

    #[test]
    fn final_screens_fall_back_to_gap_when_they_do_not_fit_the_snapshot() {
        let fixture = Fixture::new();
        let first = TARGET;
        let second = TargetRef {
            pane: 2,
            session: 2,
            ..TARGET
        };
        let third = TargetRef {
            pane: 3,
            session: 3,
            ..TARGET
        };
        let final_target = TargetRef {
            pane: 4,
            session: 4,
            ..TARGET
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_send, entered_recv) = mpsc::sync_channel(5);
        let (release_send, release_recv) = mpsc::sync_channel(5);
        let writer = HistoryWriter::start_with_backend(
            Box::new(GatedBackend {
                entered: entered_send,
                release: release_recv,
                events: Arc::clone(&events),
            }),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        for target in [first, second, third, final_target] {
            assert!(capture.register_target(
                target,
                HistoryCursor {
                    epoch: 1,
                    ordinal: 0
                }
            ));
        }

        let first_rows = maximum_rows(first, 1, MAX_BATCH_ROWS);
        let second_rows = maximum_rows(second, 1, MAX_BATCH_ROWS);
        let third_rows = maximum_rows(third, 1, 520);
        let first_payload =
            first_rows.iter().map(Vec::len).sum::<usize>() + 2 * MAX_SCREEN_CHECKPOINT_BYTES;
        let second_payload =
            second_rows.iter().map(Vec::len).sum::<usize>() + 2 * MAX_SCREEN_CHECKPOINT_BYTES;
        let third_payload = third_rows.iter().map(Vec::len).sum::<usize>();
        let occupied_bytes = first_payload + second_payload + third_payload;
        assert!(occupied_bytes < MAX_QUEUED_PAYLOAD_BYTES);
        assert!(MAX_QUEUED_PAYLOAD_BYTES - occupied_bytes < 2 * MAX_SCREEN_CHECKPOINT_BYTES);

        let submit_regular = |capture: &mut HistoryCaptureCoordinator,
                              target: TargetRef,
                              rows: &[Vec<u8>],
                              count: usize,
                              screens: Vec<CheckpointRecord>| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !capture.has_regular_pending(target) {
                capture.observe(
                    target,
                    CaptureObservation {
                        cursor: HistoryCursor {
                            epoch: 1,
                            ordinal: count as u64,
                        },
                        gap: false,
                        rows: rows.to_vec(),
                        screens: screens
                            .iter()
                            .map(|record| {
                                CheckpointRecord::new(
                                    record.target,
                                    record.buffer,
                                    record.metadata,
                                    record.clone_bytes(),
                                )
                                .unwrap()
                            })
                            .collect(),
                    },
                );
                assert!(Instant::now() < deadline, "regular batch did not admit");
                thread::yield_now();
            }
        };
        let first_screens = vec![
            maximum_screen(first, ScreenBuffer::Normal),
            maximum_screen(first, ScreenBuffer::Alternate),
        ];
        submit_regular(
            &mut capture,
            first,
            &first_rows,
            MAX_BATCH_ROWS,
            first_screens,
        );
        assert_eq!(
            entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
            (first, 1, false)
        );
        let second_screens = vec![
            maximum_screen(second, ScreenBuffer::Normal),
            maximum_screen(second, ScreenBuffer::Alternate),
        ];
        submit_regular(
            &mut capture,
            second,
            &second_rows,
            MAX_BATCH_ROWS,
            second_screens,
        );
        submit_regular(&mut capture, third, &third_rows, 520, Vec::new());

        assert!(capture.finish_target(
            final_target,
            CaptureObservation {
                cursor: HistoryCursor {
                    epoch: 1,
                    ordinal: 0
                },
                gap: false,
                rows: Vec::new(),
                screens: vec![
                    maximum_screen(final_target, ScreenBuffer::Normal),
                    maximum_screen(final_target, ScreenBuffer::Alternate),
                ],
            },
            Some(9_001),
        ));
        let diagnostics = capture.diagnostics();
        assert!(failure.load(Ordering::Acquire));
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::AdmissionBytesFull));
        assert!(diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedBytes));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedCount));
        assert!(!diagnostics
            .snapshot()
            .contains(HistoryReason::FinalPayloadDroppedContended));
        assert_eq!(
            capture.submitted_cursor(final_target),
            Some(HistoryCursor {
                epoch: 1,
                ordinal: 0
            }),
            "screen-only fallback must not manufacture row durability"
        );

        for expected in [
            (second, 1, false),
            (third, 1, false),
            (final_target, 1, true),
        ] {
            release_send.send(()).unwrap();
            assert_eq!(
                entered_recv.recv_timeout(Duration::from_secs(3)).unwrap(),
                expected
            );
        }
        release_send.send(()).unwrap();
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                events
                    .lock()
                    .unwrap()
                    .contains(&Event::Ended(final_target, 9_001))
            },
            "reserved end metadata did not commit after the writer drained",
        );
        capture.begin_shutdown();
        let writer = capture.take_writer_for_shutdown().unwrap();
        let outcome = writer.shutdown(MAX_SHUTDOWN_WAIT).outcome;
        capture.note_shutdown_outcome(outcome);
        assert_eq!(outcome, ShutdownOutcome::Drained);
        drop(fixture);
    }

    #[test]
    fn deadline_distinguishes_unsubmitted_end_from_accepted_receipts() {
        let fixture = Fixture::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_send, entered_recv) = mpsc::sync_channel(32);
        let (release_send, release_recv) = mpsc::sync_channel(32);
        let mut writer = HistoryWriter::start_with_backend(
            Box::new(GatedBackend {
                entered: entered_send,
                release: release_recv,
                events,
            }),
            fixture.storage.as_ref().unwrap(),
        )
        .unwrap();
        let mut receipts = Vec::new();
        let mut batch =
            HistoryBatch::new(TARGET, 1, 1, false, Vec::new(), Vec::new(), None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        // Worker startup briefly checks the same accounting mutex. Retry only
        // that legitimate nonblocking rejection before testing the count cap.
        loop {
            match writer.try_submit(batch) {
                Ok(receipt) => {
                    receipts.push(receipt);
                    break;
                }
                Err(error) => {
                    assert_eq!(error.capacity_reason(), Some(CapacityReason::Contended));
                    assert!(
                        Instant::now() < deadline,
                        "startup contention did not settle"
                    );
                    batch = error.into_batch();
                    thread::yield_now();
                }
            }
        }
        entered_recv.recv_timeout(Duration::from_secs(3)).unwrap();
        for token in 2..=crate::history_writer::MAX_QUEUED_BATCHES as u64 {
            receipts.push(
                writer
                    .try_submit(
                        HistoryBatch::new(TARGET, 1, token, false, Vec::new(), Vec::new(), None)
                            .unwrap(),
                    )
                    .unwrap(),
            );
        }
        let failure = Arc::new(AtomicBool::new(false));
        let mut capture = HistoryCaptureCoordinator::new(writer, Arc::clone(&failure));
        // Account for the deliberately prefilled writer's token namespace.
        capture.next_token = crate::history_writer::MAX_QUEUED_BATCHES as u64;
        assert!(capture.register_target(
            TARGET,
            HistoryCursor {
                epoch: 1,
                ordinal: 0
            }
        ));
        assert!(capture.finish_target(TARGET, observation(1, 0), Some(99)));
        capture.note_drain_deadline();
        let snapshot = capture.diagnostics().snapshot();
        assert!(failure.load(Ordering::Acquire));
        assert!(snapshot.contains(HistoryReason::FinalPayloadDroppedCount));
        assert!(snapshot.contains(HistoryReason::FinalMetadataUnsubmitted));
        // The full queue has accepted work, but this coordinator has no receipt.
        assert!(!snapshot.contains(HistoryReason::PendingReceiptsAtDeadline));
        for _ in 0..=crate::history_writer::MAX_QUEUED_BATCHES {
            release_send.send(()).unwrap();
        }
        wait_for(
            Instant::now() + Duration::from_secs(3),
            || {
                capture.drive();
                capture.all_targets_settled()
            },
            "metadata did not settle after capacity returned",
        );
        let writer = capture.take_writer_for_shutdown().unwrap();
        assert_eq!(
            writer.shutdown(MAX_SHUTDOWN_WAIT).outcome,
            ShutdownOutcome::Drained
        );
    }
}
