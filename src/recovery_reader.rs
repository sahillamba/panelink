//! Read-only loader for one explicitly selected historical recovery slot.
//!
//! This code deliberately avoids `ServerStorage`: it never creates runtime
//! paths, takes locks, initializes identities, launches commands, or writes
//! diagnostics. Layout and history sources are validated independently.

use crate::history_rows;
use crate::history_store::HistoryStore;
use crate::layout_store::{LayoutSlot, LayoutStore};
use crate::recovery_view::{
    CheckpointAvailability, CheckpointInfo, HistoryAvailability, HistoryRow, RecoveryPane,
    RecoverySlot, RecoveryView, ResetStatus,
};
use crate::screen_checkpoint::{self, ScreenBuffer};
use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;

/// Fixed, sanitized failures loading the selected layout.
#[derive(Debug)]
pub enum RecoveryReadError {
    /// Storage directory or layout slot cannot be opened safely.
    Storage,
    /// The selected slot has no committed layout.
    MissingLayout,
    /// The complete selected layout is invalid; no partial view is returned.
    InvalidLayout,
}

impl fmt::Display for RecoveryReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Storage => "recovery storage is unavailable or unsafe",
            Self::MissingLayout => "selected recovery layout is missing",
            Self::InvalidLayout => "selected recovery layout is invalid",
        })
    }
}

impl Error for RecoveryReadError {}

/// Load one exact saved-layout slot and its per-target history without writes.
///
/// `server_dir` must already exist and satisfy the storage modules' private
/// directory checks. A missing `history/` directory is represented as missing
/// data and is never created. Current and previous are selected independently.
pub fn read_recovery(
    server_dir: &Path,
    slot: RecoverySlot,
) -> Result<RecoveryView, RecoveryReadError> {
    let layout_dir = server_dir.join("layout");
    // An absent layout directory is "no saved layout", not unsafe storage.
    let layout_store = match LayoutStore::open(&layout_dir) {
        Ok(store) => store,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(RecoveryReadError::MissingLayout)
        }
        Err(_) => return Err(RecoveryReadError::Storage),
    };
    let layout_slot = match slot {
        RecoverySlot::Current => LayoutSlot::Current,
        RecoverySlot::Previous => LayoutSlot::Previous,
    };
    let layout = layout_store
        .load(layout_slot)
        .map_err(|_| RecoveryReadError::InvalidLayout)?
        .ok_or(RecoveryReadError::MissingLayout)?;

    let mut view = RecoveryView::from_layout(layout).in_slot(slot);
    let history_dir = server_dir.join("history");
    let (history_store, history_invalid) = match HistoryStore::open(&history_dir) {
        Ok(store) => (Some(store), false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (None, false),
        Err(_) => (None, true),
    };
    for pane in &mut view.panes {
        if history_invalid {
            pane.rows = HistoryAvailability::Invalid;
            pane.normal = CheckpointAvailability::Invalid;
            pane.alternate = CheckpointAvailability::Invalid;
        } else {
            load_pane_sources(history_store.as_ref(), pane);
        }
    }
    Ok(view)
}

fn load_pane_sources(store: Option<&HistoryStore>, pane: &mut RecoveryPane) {
    let Some(store) = store else {
        return;
    };
    pane.rows = match store.load_rows(pane.historical_target) {
        Ok(Some(saved)) => {
            let mut decoded = Vec::with_capacity(saved.records.len());
            let mut valid = true;
            for record in &saved.records {
                match history_rows::decode(record) {
                    Ok(row) if row.target == pane.historical_target && row.epoch == saved.epoch => {
                        decoded.push(HistoryRow {
                            captured_at_unix_ms: row.captured_at_unix_ms,
                            ordinal: row.ordinal,
                            width: row.width,
                            wrapped: row.wrapped,
                            record: record.clone(),
                        });
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if valid {
                HistoryAvailability::Available {
                    rows: decoded,
                    epoch: saved.epoch,
                    gap: saved.gap,
                    reset: if saved.gap {
                        ResetStatus::PossibleFromGap
                    } else {
                        ResetStatus::NotIndicated
                    },
                    capture_disabled: saved.capture_disabled,
                    ended_at_unix_ms: saved.ended_at_unix_ms,
                }
            } else {
                HistoryAvailability::Invalid
            }
        }
        Ok(None) => HistoryAvailability::Missing,
        Err(_) => HistoryAvailability::Invalid,
    };
    pane.normal = load_checkpoint(store, pane.historical_target, ScreenBuffer::Normal);
    pane.alternate = load_checkpoint(store, pane.historical_target, ScreenBuffer::Alternate);
}

fn load_checkpoint(
    store: &HistoryStore,
    target: crate::control_protocol::TargetRef,
    buffer: ScreenBuffer,
) -> CheckpointAvailability {
    match store.load(target, buffer) {
        Ok(Some(bytes)) => match screen_checkpoint::decode(&bytes) {
            Ok(checkpoint)
                if checkpoint.metadata().target() == target && checkpoint.buffer() == buffer =>
            {
                CheckpointAvailability::Available {
                    info: CheckpointInfo {
                        buffer,
                        output_sequence: checkpoint.metadata().output_sequence(),
                        screen_revision: checkpoint.metadata().screen_revision(),
                        captured_at_unix_ms: checkpoint.metadata().captured_at_unix_ms(),
                        rows: checkpoint.rows(),
                        columns: checkpoint.columns(),
                    },
                    record: bytes,
                }
            }
            _ => CheckpointAvailability::Invalid,
        },
        Ok(None) => CheckpointAvailability::Missing,
        Err(_) => CheckpointAvailability::Invalid,
    }
}
