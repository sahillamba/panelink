//! Versioned normalized normal-scrollback row records. Never decode them as
//! terminal instructions or as a live process state.

use crate::control_protocol::TargetRef;
use crate::screen_checkpoint::{self, SCREEN_CHECKPOINT_CELL_BYTES};
use crate::terminal::SnapshotCell;
use std::io;
use vt100::HistoryRow;

const MAGIC: &[u8; 8] = b"PLHROW01";
pub const HEADER_BYTES: usize = 80;
pub const MAX_ROW_BYTES: usize = HEADER_BYTES + 240 * SCREEN_CHECKPOINT_CELL_BYTES;

pub(crate) fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid saved scrollback")
}

/// Metadata and validated sanitized cells of a saved normal-history row.
pub struct SavedRow<'a> {
    pub target: TargetRef,
    pub epoch: u64,
    pub ordinal: u64,
    pub captured_at_unix_ms: u64,
    pub width: u16,
    pub wrapped: bool,
    cells: &'a [u8],
}

impl std::fmt::Debug for SavedRow<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SavedRow { <redacted> }")
    }
}

impl SavedRow<'_> {
    pub fn cell_text(&self, column: u16) -> Option<&str> {
        if column >= self.width {
            return None;
        }
        let start = usize::from(column) * SCREEN_CHECKPOINT_CELL_BYTES;
        let length = usize::from(self.cells[start]);
        std::str::from_utf8(&self.cells[start + 10..start + 10 + length]).ok()
    }
}

/// Convert borrowed engine cells through the same sanitizer and validation
/// used for screen checkpoints. The maximum scratch is one 240-cell row.
pub fn encode(
    target: TargetRef,
    epoch: u64,
    row: HistoryRow<'_>,
    captured_at_unix_ms: u64,
) -> io::Result<Vec<u8>> {
    if epoch == 0
        || row.ordinal == 0
        || captured_at_unix_ms == 0
        || !(1..=240).contains(&row.width)
        || !valid_target(target)
    {
        return Err(invalid());
    }
    let mut cells = Vec::with_capacity(usize::from(row.width));
    for column in 0..row.width {
        let mut sanitized = SnapshotCell::default();
        sanitized.set_from(row.cell(column).ok_or_else(invalid)?);
        cells.push(sanitized);
    }
    let body = screen_checkpoint::encode_history_cells(&cells).map_err(|_| invalid())?;
    let mut record = Vec::with_capacity(HEADER_BYTES + body.len());
    record.extend_from_slice(MAGIC);
    for value in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
        epoch,
        row.ordinal,
        captured_at_unix_ms,
    ] {
        record.extend_from_slice(&value.to_be_bytes());
    }
    record.extend_from_slice(&row.width.to_be_bytes());
    record.push(u8::from(row.wrapped));
    record.extend_from_slice(&[0; 5]);
    record.extend_from_slice(&body);
    Ok(record)
}

/// Validate the entire fixed-and-bounded record before offering read-only
/// cells to a caller. Reject partial tails, unsafe scalars and forged shape.
pub fn decode(record: &[u8]) -> io::Result<SavedRow<'_>> {
    if !(HEADER_BYTES..=MAX_ROW_BYTES).contains(&record.len())
        || &record[..8] != MAGIC
        || record[75..80] != [0; 5]
    {
        return Err(invalid());
    }
    let number = |offset: usize| u64::from_be_bytes(record[offset..offset + 8].try_into().unwrap());
    let target = TargetRef {
        generation: number(8),
        workspace: number(16),
        pane: number(24),
        session: number(32),
        occupant_revision: number(40),
    };
    let epoch = number(48);
    let ordinal = number(56);
    let captured_at_unix_ms = number(64);
    let width = u16::from_be_bytes(record[72..74].try_into().unwrap());
    let wrapped = match record[74] {
        0 => false,
        1 => true,
        _ => return Err(invalid()),
    };
    if !valid_target(target) || epoch == 0 || ordinal == 0 || captured_at_unix_ms == 0 {
        return Err(invalid());
    }
    let cells = &record[HEADER_BYTES..];
    screen_checkpoint::validate_history_cells(cells, width).map_err(|_| invalid())?;
    Ok(SavedRow {
        target,
        epoch,
        ordinal,
        captured_at_unix_ms,
        width,
        wrapped,
        cells,
    })
}

/// Two observations of one ordinal may have different capture timestamps
/// after a post-rename sync failure. Validate both records, then compare all
/// immutable identity, wrap, width and sanitized cell bytes, not wall time.
pub fn same_content(left: &[u8], right: &[u8]) -> io::Result<bool> {
    let a = decode(left)?;
    let b = decode(right)?;
    Ok(a.target == b.target
        && a.epoch == b.epoch
        && a.ordinal == b.ordinal
        && a.width == b.width
        && a.wrapped == b.wrapped
        && a.cells == b.cells)
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
