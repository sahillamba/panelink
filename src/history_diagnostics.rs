//! Bounded, content-free observations for the live saved-history path.
//!
//! Diagnostics retain only a sticky set of fixed reason bits and the actual
//! writer shutdown result. They contain no target identity, payload, path, or
//! runtime error text, and can outlive the driver and writer that recorded them.

use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

const DRAIN_NOT_REQUESTED: u8 = 0;
const DRAINED: u8 = 1;
const DRAIN_FAILED: u8 = 2;
const DRAIN_TIMED_OUT: u8 = 3;

/// A fixed, content-free cause observed by saved-history capture or storage.
///
/// The discriminants are stable within this prototype diagnostic contract and
/// fit in the bounded 32-bit reason set.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryReason {
    AdmissionContended = 0,
    AdmissionCountFull = 1,
    AdmissionBytesFull = 2,
    FinalPayloadDroppedContended = 3,
    FinalPayloadDroppedCount = 4,
    FinalPayloadDroppedBytes = 5,
    FinalPayloadInvalid = 6,
    StorageError = 7,
    RetryExhausted = 8,
    ReceiptFailed = 9,
    WorkerFailure = 10,
    ShutdownTimedOut = 11,
    WorkCanceled = 12,
    PendingReceiptsAtDeadline = 13,
    FinalMetadataUnsubmitted = 14,
    CaptureSetupFailed = 15,
    CaptureInvalid = 16,
    CaptureRegistrationFull = 17,
    UnknownEndTime = 18,
    CanceledEndMetadataFinalized = 19,
    CanceledEndMetadataFinalizeFailed = 20,
}

impl HistoryReason {
    const fn bit(self) -> u32 {
        1_u32 << self as u32
    }

    const fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::AdmissionContended,
            1 => Self::AdmissionCountFull,
            2 => Self::AdmissionBytesFull,
            3 => Self::FinalPayloadDroppedContended,
            4 => Self::FinalPayloadDroppedCount,
            5 => Self::FinalPayloadDroppedBytes,
            6 => Self::FinalPayloadInvalid,
            7 => Self::StorageError,
            8 => Self::RetryExhausted,
            9 => Self::ReceiptFailed,
            10 => Self::WorkerFailure,
            11 => Self::ShutdownTimedOut,
            12 => Self::WorkCanceled,
            13 => Self::PendingReceiptsAtDeadline,
            14 => Self::FinalMetadataUnsubmitted,
            15 => Self::CaptureSetupFailed,
            16 => Self::CaptureInvalid,
            17 => Self::CaptureRegistrationFull,
            18 => Self::UnknownEndTime,
            19 => Self::CanceledEndMetadataFinalized,
            20 => Self::CanceledEndMetadataFinalizeFailed,
            _ => return None,
        })
    }

    /// Stable lowercase label for reports and operator-facing diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdmissionContended => "admission_contended",
            Self::AdmissionCountFull => "admission_count_full",
            Self::AdmissionBytesFull => "admission_bytes_full",
            Self::FinalPayloadDroppedContended => "final_payload_dropped_contended",
            Self::FinalPayloadDroppedCount => "final_payload_dropped_count",
            Self::FinalPayloadDroppedBytes => "final_payload_dropped_bytes",
            Self::FinalPayloadInvalid => "final_payload_invalid",
            Self::StorageError => "storage_error",
            Self::RetryExhausted => "retry_exhausted",
            Self::ReceiptFailed => "receipt_failed",
            Self::WorkerFailure => "worker_failure",
            Self::ShutdownTimedOut => "shutdown_timed_out",
            Self::WorkCanceled => "work_canceled",
            Self::PendingReceiptsAtDeadline => "pending_receipts_at_deadline",
            Self::FinalMetadataUnsubmitted => "final_metadata_unsubmitted",
            Self::CaptureSetupFailed => "capture_setup_failed",
            Self::CaptureInvalid => "capture_invalid",
            Self::CaptureRegistrationFull => "capture_registration_full",
            Self::UnknownEndTime => "unknown_end_time",
            Self::CanceledEndMetadataFinalized => "canceled_end_metadata_finalized",
            Self::CanceledEndMetadataFinalizeFailed => "canceled_end_metadata_finalize_failed",
        }
    }
}

/// The actual result of an explicit history-writer shutdown request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryDrainOutcome {
    /// No explicit writer shutdown was requested.
    NotRequested,
    /// The writer reported that its accepted work had settled successfully.
    Drained,
    /// The writer reported a failed worker or accepted batch.
    Failed,
    /// The writer's bounded wait expired; later worker completion cannot
    /// change this result.
    TimedOut,
}

impl HistoryDrainOutcome {
    /// Stable lowercase label for reports and operator-facing diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Drained => "drained",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }

    const fn as_u8(self) -> u8 {
        match self {
            Self::NotRequested => DRAIN_NOT_REQUESTED,
            Self::Drained => DRAINED,
            Self::Failed => DRAIN_FAILED,
            Self::TimedOut => DRAIN_TIMED_OUT,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            DRAINED => Self::Drained,
            DRAIN_FAILED => Self::Failed,
            DRAIN_TIMED_OUT => Self::TimedOut,
            _ => Self::NotRequested,
        }
    }
}

struct Inner {
    reasons: AtomicU32,
    writer_drain_outcome: AtomicU8,
}

/// Cheaply clonable owner of bounded, sanitized history diagnostics.
///
/// A handle is valid before capture is enabled and remains valid after the
/// driver, coordinator, and writer are dropped. Concurrent snapshots are
/// nontransactional; a stable snapshot after settlement includes late worker
/// events from a timed-out writer.
#[derive(Clone)]
pub struct HistoryDiagnostics {
    inner: Arc<Inner>,
}

impl HistoryDiagnostics {
    /// Create an empty diagnostic handle for one server generation.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                reasons: AtomicU32::new(0),
                writer_drain_outcome: AtomicU8::new(DRAIN_NOT_REQUESTED),
            }),
        }
    }

    /// Take a bounded, nontransactional snapshot of the sticky observations.
    pub fn snapshot(&self) -> HistoryDiagnosticSnapshot {
        HistoryDiagnosticSnapshot {
            reasons: self.inner.reasons.load(Ordering::Acquire),
            writer_drain_outcome: HistoryDrainOutcome::from_u8(
                self.inner.writer_drain_outcome.load(Ordering::Acquire),
            ),
        }
    }

    pub(crate) fn record(&self, reason: HistoryReason) {
        self.inner.reasons.fetch_or(reason.bit(), Ordering::AcqRel);
    }

    pub(crate) fn set_writer_drain_outcome(&self, outcome: HistoryDrainOutcome) {
        self.inner
            .writer_drain_outcome
            .store(outcome.as_u8(), Ordering::Release);
    }
}

impl Default for HistoryDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HistoryDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HistoryDiagnostics")
            .field(&self.snapshot())
            .finish()
    }
}

/// A small immutable snapshot of reason bits and the writer drain result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryDiagnosticSnapshot {
    reasons: u32,
    writer_drain_outcome: HistoryDrainOutcome,
}

impl HistoryDiagnosticSnapshot {
    /// Iterate over the fixed set of observed reasons in discriminant order.
    pub fn reasons(self) -> HistoryReasonIter {
        HistoryReasonIter {
            remaining: self.reasons,
        }
    }

    /// Whether this reason has been observed at least once.
    pub const fn contains(self, reason: HistoryReason) -> bool {
        self.reasons & reason.bit() != 0
    }

    /// The actual history-writer shutdown result, if shutdown was requested.
    pub const fn writer_drain_outcome(self) -> HistoryDrainOutcome {
        self.writer_drain_outcome
    }
}

/// Bounded iterator over the fixed history reason set.
pub struct HistoryReasonIter {
    remaining: u32,
}

impl Iterator for HistoryReasonIter {
    type Item = HistoryReason;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let index = self.remaining.trailing_zeros();
        self.remaining &= !(1_u32 << index);
        HistoryReason::from_index(index)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.remaining.count_ones() as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for HistoryReasonIter {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_sticky_bounded_and_use_fixed_labels() {
        let diagnostics = HistoryDiagnostics::new();
        diagnostics.record(HistoryReason::AdmissionContended);
        diagnostics.record(HistoryReason::UnknownEndTime);
        let snapshot = diagnostics.snapshot();

        assert!(snapshot.contains(HistoryReason::AdmissionContended));
        assert!(snapshot.contains(HistoryReason::UnknownEndTime));
        assert!(!snapshot.contains(HistoryReason::StorageError));
        assert_eq!(
            snapshot
                .reasons()
                .map(HistoryReason::as_str)
                .collect::<Vec<_>>(),
            ["admission_contended", "unknown_end_time"]
        );
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::NotRequested
        );
        assert_eq!(HistoryDrainOutcome::TimedOut.as_str(), "timed_out");
    }

    #[test]
    fn cloned_handle_keeps_late_reasons_and_drain_result() {
        let retained = HistoryDiagnostics::new();
        let worker = retained.clone();
        worker.set_writer_drain_outcome(HistoryDrainOutcome::TimedOut);
        worker.record(HistoryReason::ShutdownTimedOut);
        drop(worker);

        let snapshot = retained.snapshot();
        assert!(snapshot.contains(HistoryReason::ShutdownTimedOut));
        assert_eq!(
            snapshot.writer_drain_outcome(),
            HistoryDrainOutcome::TimedOut
        );
    }
}
