//! Bounded, read-only codec for normalized visible-screen checkpoints.
//!
//! This module stores sanitized cell data, never raw terminal control streams,
//! and never reconstructs a live terminal emulator from a checkpoint.

use crate::control_protocol::TargetRef;
use crate::terminal::{ScreenSnapshot, SnapshotCell};
use std::error::Error;
use std::fmt;
use std::str;
use vt100::Color;

/// Fixed header size in the version-one checkpoint format.
pub const SCREEN_CHECKPOINT_HEADER_BYTES: usize = 96;

/// Serialized size of one normalized row-major screen cell.
pub const SCREEN_CHECKPOINT_CELL_BYTES: usize = 48;

/// Maximum encoded checkpoint at the largest supported viewport.
pub const MAX_SCREEN_CHECKPOINT_BYTES: usize =
    SCREEN_CHECKPOINT_HEADER_BYTES + 80 * 240 * SCREEN_CHECKPOINT_CELL_BYTES;

const MAGIC: &[u8; 8] = b"PLSCRN01";
const TARGET_OFFSET: usize = 8;
const OUTPUT_SEQUENCE_OFFSET: usize = 48;
const CAPTURE_TIME_OFFSET: usize = 56;
const SCREEN_REVISION_OFFSET: usize = 64;
const ROWS_OFFSET: usize = 72;
const COLUMNS_OFFSET: usize = 74;
const CURSOR_ROW_OFFSET: usize = 76;
const CURSOR_COLUMN_OFFSET: usize = 78;
const BUFFER_OFFSET: usize = 80;
const CURSOR_HIDDEN_OFFSET: usize = 81;
const HEADER_RESERVED_OFFSET: usize = 82;
const HEADER_RESERVED_BYTES: usize = 14;

const CELL_TEXT_LENGTH_OFFSET: usize = 0;
const CELL_FLAGS_OFFSET: usize = 1;
const CELL_FOREGROUND_OFFSET: usize = 2;
const CELL_BACKGROUND_OFFSET: usize = 6;
const CELL_TEXT_OFFSET: usize = 10;
const CELL_TEXT_BYTES: usize = 32;
const CELL_RESERVED_OFFSET: usize = 42;
const CELL_RESERVED_BYTES: usize = 6;

const FLAG_WIDE_LEAD: u8 = 1 << 0;
const FLAG_WIDE_CONTINUATION: u8 = 1 << 1;
const FLAG_BOLD: u8 = 1 << 2;
const FLAG_DIM: u8 = 1 << 3;
const FLAG_ITALIC: u8 = 1 << 4;
const FLAG_UNDERLINE: u8 = 1 << 5;
const FLAG_INVERSE: u8 = 1 << 6;
const KNOWN_FLAGS: u8 = FLAG_WIDE_LEAD
    | FLAG_WIDE_CONTINUATION
    | FLAG_BOLD
    | FLAG_DIM
    | FLAG_ITALIC
    | FLAG_UNDERLINE
    | FLAG_INVERSE;

/// Fixed codec errors. No variant includes target, text, or color data.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ScreenCheckpointError {
    /// A checkpoint is shorter than its fixed header.
    TooShort,
    /// A checkpoint exceeds the maximum supported record size.
    TooLarge,
    /// Metadata or a source snapshot is malformed.
    InvalidInput,
    /// Serialized checkpoint bytes are malformed or unsafe.
    InvalidRecord,
    /// A bounded output allocation could not be reserved.
    AllocationFailure,
}

impl fmt::Debug for ScreenCheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScreenCheckpointError { <redacted> }")
    }
}

impl fmt::Display for ScreenCheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TooShort => "screen checkpoint is too short",
            Self::TooLarge => "screen checkpoint is too large",
            Self::InvalidInput => "invalid screen checkpoint input",
            Self::InvalidRecord => "invalid screen checkpoint record",
            Self::AllocationFailure => "screen checkpoint allocation failed",
        };
        f.write_str(message)
    }
}

impl Error for ScreenCheckpointError {}

/// Metadata attached to one captured screen.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CheckpointMetadata {
    target: TargetRef,
    output_sequence: u64,
    captured_at_unix_ms: u64,
    screen_revision: u64,
}

impl CheckpointMetadata {
    /// Validate metadata for one complete immutable target.
    ///
    /// The initial output sequence may be zero. Target IDs, capture time, and
    /// screen revision must be nonzero.
    pub fn new(
        target: TargetRef,
        output_sequence: u64,
        captured_at_unix_ms: u64,
        screen_revision: u64,
    ) -> Result<Self, ScreenCheckpointError> {
        if !valid_target(target) || captured_at_unix_ms == 0 || screen_revision == 0 {
            return Err(ScreenCheckpointError::InvalidInput);
        }
        Ok(Self {
            target,
            output_sequence,
            captured_at_unix_ms,
            screen_revision,
        })
    }

    /// Complete immutable target reference.
    pub const fn target(self) -> TargetRef {
        self.target
    }

    /// Captured child-output sequence; zero is allowed before first output.
    pub const fn output_sequence(self) -> u64 {
        self.output_sequence
    }

    /// Capture timestamp supplied by the caller in Unix milliseconds.
    pub const fn captured_at_unix_ms(self) -> u64 {
        self.captured_at_unix_ms
    }

    /// Nonzero revision of the captured screen.
    pub const fn screen_revision(self) -> u64 {
        self.screen_revision
    }
}

impl fmt::Debug for CheckpointMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckpointMetadata { <redacted> }")
    }
}

/// Normal or alternate terminal buffer represented by a checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenBuffer {
    /// Visible normal screen.
    Normal,
    /// Visible alternate screen.
    Alternate,
}

/// Validated color value returned by a borrowed checkpoint cell.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CheckpointColor {
    /// Terminal default color.
    Default,
    /// Palette index from zero through 255.
    Indexed(u8),
    /// Explicit red, green, and blue components.
    Rgb(u8, u8, u8),
}

impl fmt::Debug for CheckpointColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckpointColor { <redacted> }")
    }
}

/// Immutable borrowed view over one fully validated screen checkpoint.
#[derive(Eq, PartialEq)]
pub struct ScreenCheckpoint<'a> {
    metadata: CheckpointMetadata,
    rows: u16,
    columns: u16,
    cursor_row: u16,
    cursor_column: u16,
    buffer: ScreenBuffer,
    cursor_hidden: bool,
    cell_bytes: &'a [u8],
}

impl ScreenCheckpoint<'_> {
    /// Validated record metadata.
    pub const fn metadata(&self) -> CheckpointMetadata {
        self.metadata
    }

    /// Visible row count.
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Visible column count.
    pub const fn columns(&self) -> u16 {
        self.columns
    }

    /// Zero-based cursor row.
    pub const fn cursor_row(&self) -> u16 {
        self.cursor_row
    }

    /// Zero-based cursor column.
    pub const fn cursor_column(&self) -> u16 {
        self.cursor_column
    }

    /// Which visible terminal buffer the checkpoint captured.
    pub const fn buffer(&self) -> ScreenBuffer {
        self.buffer
    }

    /// Whether the captured screen requested a hidden cursor.
    pub const fn cursor_hidden(&self) -> bool {
        self.cursor_hidden
    }

    /// Return one cell at a zero-based visible position.
    pub fn cell(&self, row: u16, column: u16) -> Option<CheckpointCell<'_>> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        let index = usize::from(row)
            .checked_mul(usize::from(self.columns))?
            .checked_add(usize::from(column))?;
        self.cell_at(index)
    }

    /// Iterate over validated cells in row-major order.
    pub fn cells(&self) -> CheckpointCells<'_> {
        CheckpointCells {
            cell_bytes: self.cell_bytes,
            offset: 0,
        }
    }

    fn cell_at(&self, index: usize) -> Option<CheckpointCell<'_>> {
        let start = index.checked_mul(SCREEN_CHECKPOINT_CELL_BYTES)?;
        let end = start.checked_add(SCREEN_CHECKPOINT_CELL_BYTES)?;
        Some(CheckpointCell {
            bytes: self.cell_bytes.get(start..end)?,
        })
    }
}

impl fmt::Debug for ScreenCheckpoint<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScreenCheckpoint { <redacted> }")
    }
}

/// Iterator over validated borrowed checkpoint cells.
pub struct CheckpointCells<'a> {
    cell_bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for CheckpointCells<'a> {
    type Item = CheckpointCell<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.offset.checked_add(SCREEN_CHECKPOINT_CELL_BYTES)?;
        let bytes = self.cell_bytes.get(self.offset..end)?;
        self.offset = end;
        Some(CheckpointCell { bytes })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining =
            self.cell_bytes.len().saturating_sub(self.offset) / SCREEN_CHECKPOINT_CELL_BYTES;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CheckpointCells<'_> {}

impl std::iter::FusedIterator for CheckpointCells<'_> {}

/// Immutable borrowed view of one fully validated fixed-size cell.
pub struct CheckpointCell<'a> {
    bytes: &'a [u8],
}

impl<'a> CheckpointCell<'a> {
    /// Cell text as validated UTF-8; empty ordinary cells are allowed.
    pub fn text(&self) -> &'a str {
        let length = usize::from(self.bytes.first().copied().unwrap_or(0));
        let end = CELL_TEXT_OFFSET.saturating_add(length);
        self.bytes
            .get(CELL_TEXT_OFFSET..end)
            .and_then(|text| str::from_utf8(text).ok())
            .unwrap_or("")
    }

    /// Whether this is a wide-glyph lead cell.
    pub fn is_wide(&self) -> bool {
        self.flag(FLAG_WIDE_LEAD)
    }

    /// Whether this is a wide-glyph continuation cell.
    pub fn is_wide_continuation(&self) -> bool {
        self.flag(FLAG_WIDE_CONTINUATION)
    }

    /// Foreground color, always present for a decoded cell.
    pub fn foreground(&self) -> Option<CheckpointColor> {
        let bytes = self
            .bytes
            .get(CELL_FOREGROUND_OFFSET..CELL_BACKGROUND_OFFSET)?;
        decode_color(bytes)
    }

    /// Background color, always present for a decoded cell.
    pub fn background(&self) -> Option<CheckpointColor> {
        let start = CELL_BACKGROUND_OFFSET;
        decode_color(self.bytes.get(start..start + 4)?)
    }

    /// Bold rendition flag.
    pub fn bold(&self) -> bool {
        self.flag(FLAG_BOLD)
    }

    /// Dim rendition flag.
    pub fn dim(&self) -> bool {
        self.flag(FLAG_DIM)
    }

    /// Italic rendition flag.
    pub fn italic(&self) -> bool {
        self.flag(FLAG_ITALIC)
    }

    /// Underline rendition flag.
    pub fn underline(&self) -> bool {
        self.flag(FLAG_UNDERLINE)
    }

    /// Inverse rendition flag.
    pub fn inverse(&self) -> bool {
        self.flag(FLAG_INVERSE)
    }

    fn flag(&self, flag: u8) -> bool {
        self.bytes
            .get(CELL_FLAGS_OFFSET)
            .is_some_and(|flags| flags & flag != 0)
    }
}

impl fmt::Debug for CheckpointCell<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckpointCell { <redacted> }")
    }
}

/// Encode one normalized visible screen plus bounded metadata.
///
/// The complete metadata and every cell are validated before allocating the
/// output. The returned bytes contain no raw host control stream or input.
pub fn encode(
    snapshot: &ScreenSnapshot,
    metadata: &CheckpointMetadata,
) -> Result<Vec<u8>, ScreenCheckpointError> {
    validate_metadata(metadata)?;
    let rows = snapshot.rows();
    let columns = snapshot.columns();
    let cursor_row = snapshot.cursor_row();
    let cursor_column = snapshot.cursor_column();
    let cells = snapshot.cells();
    let count = validate_snapshot_geometry(rows, columns, cursor_row, cursor_column)?;
    if cells.len() != count {
        return Err(ScreenCheckpointError::InvalidInput);
    }
    validate_snapshot_cells(cells, columns)?;

    let cell_bytes = count
        .checked_mul(SCREEN_CHECKPOINT_CELL_BYTES)
        .ok_or(ScreenCheckpointError::InvalidInput)?;
    let total_bytes = SCREEN_CHECKPOINT_HEADER_BYTES
        .checked_add(cell_bytes)
        .filter(|length| *length <= MAX_SCREEN_CHECKPOINT_BYTES)
        .ok_or(ScreenCheckpointError::InvalidInput)?;

    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(total_bytes)
        .map_err(|_| ScreenCheckpointError::AllocationFailure)?;
    encoded.extend_from_slice(MAGIC);
    for id in target_fields(metadata.target) {
        encoded.extend_from_slice(&id.to_be_bytes());
    }
    encoded.extend_from_slice(&metadata.output_sequence.to_be_bytes());
    encoded.extend_from_slice(&metadata.captured_at_unix_ms.to_be_bytes());
    encoded.extend_from_slice(&metadata.screen_revision.to_be_bytes());
    encoded.extend_from_slice(&rows.to_be_bytes());
    encoded.extend_from_slice(&columns.to_be_bytes());
    encoded.extend_from_slice(&cursor_row.to_be_bytes());
    encoded.extend_from_slice(&cursor_column.to_be_bytes());
    encoded.push(u8::from(snapshot.alternate_screen()));
    encoded.push(u8::from(snapshot.cursor_hidden()));
    encoded.extend_from_slice(&[0; HEADER_RESERVED_BYTES]);

    for cell in cells {
        let text = cell.text().as_bytes();
        encoded.push(u8::try_from(text.len()).map_err(|_| ScreenCheckpointError::InvalidInput)?);
        encoded.push(snapshot_cell_flags(cell));
        encoded.extend_from_slice(&encode_color(cell.foreground()));
        encoded.extend_from_slice(&encode_color(cell.background()));
        encoded.extend_from_slice(text);
        encoded.resize(encoded.len() + CELL_TEXT_BYTES - text.len(), 0);
        encoded.extend_from_slice(&[0; CELL_RESERVED_BYTES]);
    }
    debug_assert_eq!(encoded.len(), total_bytes);
    Ok(encoded)
}

/// Decode a borrowed, read-only checkpoint after validating the complete record.
///
/// The input-size bound, header, dimensions, checked exact body size, all cell
/// padding, UTF-8, colors, flags, and wide-cell topology are checked before a
/// view is returned. No data is copied or allocated while decoding.
pub fn decode(record: &[u8]) -> Result<ScreenCheckpoint<'_>, ScreenCheckpointError> {
    if record.len() > MAX_SCREEN_CHECKPOINT_BYTES {
        return Err(ScreenCheckpointError::TooLarge);
    }
    if record.len() < SCREEN_CHECKPOINT_HEADER_BYTES {
        return Err(ScreenCheckpointError::TooShort);
    }
    if &record[..MAGIC.len()] != MAGIC
        || record[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_BYTES]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(ScreenCheckpointError::InvalidRecord);
    }

    let target = TargetRef {
        generation: read_u64(record, TARGET_OFFSET)?,
        workspace: read_u64(record, TARGET_OFFSET + 8)?,
        pane: read_u64(record, TARGET_OFFSET + 16)?,
        session: read_u64(record, TARGET_OFFSET + 24)?,
        occupant_revision: read_u64(record, TARGET_OFFSET + 32)?,
    };
    let output_sequence = read_u64(record, OUTPUT_SEQUENCE_OFFSET)?;
    let captured_at_unix_ms = read_u64(record, CAPTURE_TIME_OFFSET)?;
    let screen_revision = read_u64(record, SCREEN_REVISION_OFFSET)?;
    let rows = read_u16(record, ROWS_OFFSET)?;
    let columns = read_u16(record, COLUMNS_OFFSET)?;
    let cursor_row = read_u16(record, CURSOR_ROW_OFFSET)?;
    let cursor_column = read_u16(record, CURSOR_COLUMN_OFFSET)?;
    let buffer = match record[BUFFER_OFFSET] {
        0 => ScreenBuffer::Normal,
        1 => ScreenBuffer::Alternate,
        _ => return Err(ScreenCheckpointError::InvalidRecord),
    };
    let cursor_hidden = match record[CURSOR_HIDDEN_OFFSET] {
        0 => false,
        1 => true,
        _ => return Err(ScreenCheckpointError::InvalidRecord),
    };
    let metadata = CheckpointMetadata::new(
        target,
        output_sequence,
        captured_at_unix_ms,
        screen_revision,
    )
    .map_err(|_| ScreenCheckpointError::InvalidRecord)?;
    let count = validate_snapshot_geometry(rows, columns, cursor_row, cursor_column)
        .map_err(|_| ScreenCheckpointError::InvalidRecord)?;
    let body_bytes = count
        .checked_mul(SCREEN_CHECKPOINT_CELL_BYTES)
        .ok_or(ScreenCheckpointError::InvalidRecord)?;
    let expected_bytes = SCREEN_CHECKPOINT_HEADER_BYTES
        .checked_add(body_bytes)
        .filter(|length| *length <= MAX_SCREEN_CHECKPOINT_BYTES)
        .ok_or(ScreenCheckpointError::InvalidRecord)?;
    if expected_bytes != record.len() {
        return Err(ScreenCheckpointError::InvalidRecord);
    }

    // Do not examine a cell until dimensions and exact payload length agree.
    let cell_bytes = &record[SCREEN_CHECKPOINT_HEADER_BYTES..expected_bytes];
    validate_record_cells(cell_bytes, rows, columns)?;

    Ok(ScreenCheckpoint {
        metadata,
        rows,
        columns,
        cursor_row,
        cursor_column,
        buffer,
        cursor_hidden,
        cell_bytes,
    })
}

fn validate_metadata(metadata: &CheckpointMetadata) -> Result<(), ScreenCheckpointError> {
    if !valid_target(metadata.target)
        || metadata.captured_at_unix_ms == 0
        || metadata.screen_revision == 0
    {
        return Err(ScreenCheckpointError::InvalidInput);
    }
    Ok(())
}

fn valid_target(target: TargetRef) -> bool {
    target_fields(target).iter().all(|field| *field != 0)
}

const fn target_fields(target: TargetRef) -> [u64; 5] {
    [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
}

fn validate_snapshot_geometry(
    rows: u16,
    columns: u16,
    cursor_row: u16,
    cursor_column: u16,
) -> Result<usize, ScreenCheckpointError> {
    if !(5..=80).contains(&rows)
        || !(20..=240).contains(&columns)
        || cursor_row >= rows
        || cursor_column >= columns
    {
        return Err(ScreenCheckpointError::InvalidInput);
    }
    usize::from(rows)
        .checked_mul(usize::from(columns))
        .ok_or(ScreenCheckpointError::InvalidInput)
}

fn validate_snapshot_cells(
    cells: &[SnapshotCell],
    columns: u16,
) -> Result<(), ScreenCheckpointError> {
    let column_count = usize::from(columns);
    for (index, cell) in cells.iter().enumerate() {
        let text = cell.text();
        if text.len() > CELL_TEXT_BYTES || text.chars().any(unsafe_display_scalar) {
            return Err(ScreenCheckpointError::InvalidInput);
        }
        let wide = cell.is_wide();
        let continuation = cell.is_wide_continuation();
        if wide && continuation || continuation && !text.is_empty() {
            return Err(ScreenCheckpointError::InvalidInput);
        }
        let column = index % column_count;
        if wide {
            if column + 1 >= column_count {
                return Err(ScreenCheckpointError::InvalidInput);
            }
            let Some(next) = cells.get(index + 1) else {
                return Err(ScreenCheckpointError::InvalidInput);
            };
            if !next.is_wide_continuation() || next.is_wide() || !next.text().is_empty() {
                return Err(ScreenCheckpointError::InvalidInput);
            }
        }
        if continuation {
            if column == 0 {
                return Err(ScreenCheckpointError::InvalidInput);
            }
            let Some(previous) = cells.get(index - 1) else {
                return Err(ScreenCheckpointError::InvalidInput);
            };
            if !previous.is_wide() || previous.is_wide_continuation() {
                return Err(ScreenCheckpointError::InvalidInput);
            }
        }
    }
    Ok(())
}

fn validate_record_cells(
    cells: &[u8],
    rows: u16,
    columns: u16,
) -> Result<(), ScreenCheckpointError> {
    let count = usize::from(rows)
        .checked_mul(usize::from(columns))
        .ok_or(ScreenCheckpointError::InvalidRecord)?;
    if cells.len() != count * SCREEN_CHECKPOINT_CELL_BYTES {
        return Err(ScreenCheckpointError::InvalidRecord);
    }
    let column_count = usize::from(columns);
    for index in 0..count {
        let start = index * SCREEN_CHECKPOINT_CELL_BYTES;
        let cell = &cells[start..start + SCREEN_CHECKPOINT_CELL_BYTES];
        let text_length = usize::from(cell[CELL_TEXT_LENGTH_OFFSET]);
        let flags = cell[CELL_FLAGS_OFFSET];
        if text_length > CELL_TEXT_BYTES
            || flags & !KNOWN_FLAGS != 0
            || flags & FLAG_WIDE_LEAD != 0 && flags & FLAG_WIDE_CONTINUATION != 0
            || !valid_color(&cell[CELL_FOREGROUND_OFFSET..CELL_BACKGROUND_OFFSET])
            || !valid_color(&cell[CELL_BACKGROUND_OFFSET..CELL_TEXT_OFFSET])
            || cell[CELL_TEXT_OFFSET + text_length..CELL_RESERVED_OFFSET]
                .iter()
                .any(|byte| *byte != 0)
            || cell[CELL_RESERVED_OFFSET..].iter().any(|byte| *byte != 0)
        {
            return Err(ScreenCheckpointError::InvalidRecord);
        }
        let text = str::from_utf8(&cell[CELL_TEXT_OFFSET..CELL_TEXT_OFFSET + text_length])
            .map_err(|_| ScreenCheckpointError::InvalidRecord)?;
        if text.chars().any(unsafe_display_scalar) {
            return Err(ScreenCheckpointError::InvalidRecord);
        }

        let column = index % column_count;
        let is_wide = flags & FLAG_WIDE_LEAD != 0;
        let is_continuation = flags & FLAG_WIDE_CONTINUATION != 0;
        if is_continuation && text_length != 0 {
            return Err(ScreenCheckpointError::InvalidRecord);
        }
        if is_wide {
            if column + 1 >= column_count {
                return Err(ScreenCheckpointError::InvalidRecord);
            }
            let next_start = start + SCREEN_CHECKPOINT_CELL_BYTES;
            let next = &cells[next_start..next_start + SCREEN_CHECKPOINT_CELL_BYTES];
            let next_flags = next[CELL_FLAGS_OFFSET];
            if next_flags & FLAG_WIDE_CONTINUATION == 0
                || next_flags & FLAG_WIDE_LEAD != 0
                || next[CELL_TEXT_LENGTH_OFFSET] != 0
            {
                return Err(ScreenCheckpointError::InvalidRecord);
            }
        }
        if is_continuation {
            if column == 0 {
                return Err(ScreenCheckpointError::InvalidRecord);
            }
            let previous_start = start - SCREEN_CHECKPOINT_CELL_BYTES;
            let previous_flags = cells[previous_start + CELL_FLAGS_OFFSET];
            if previous_flags & FLAG_WIDE_LEAD == 0 || previous_flags & FLAG_WIDE_CONTINUATION != 0
            {
                return Err(ScreenCheckpointError::InvalidRecord);
            }
        }
    }
    Ok(())
}

fn snapshot_cell_flags(cell: &SnapshotCell) -> u8 {
    (u8::from(cell.is_wide()) * FLAG_WIDE_LEAD)
        | (u8::from(cell.is_wide_continuation()) * FLAG_WIDE_CONTINUATION)
        | (u8::from(cell.bold()) * FLAG_BOLD)
        | (u8::from(cell.dim()) * FLAG_DIM)
        | (u8::from(cell.italic()) * FLAG_ITALIC)
        | (u8::from(cell.underline()) * FLAG_UNDERLINE)
        | (u8::from(cell.inverse()) * FLAG_INVERSE)
}

fn encode_color(color: Color) -> [u8; 4] {
    match color {
        Color::Default => [0, 0, 0, 0],
        Color::Idx(index) => [1, index, 0, 0],
        Color::Rgb(red, green, blue) => [2, red, green, blue],
    }
}

fn valid_color(bytes: &[u8]) -> bool {
    matches!(bytes, [0, 0, 0, 0] | [1, _, 0, 0] | [2, _, _, _])
}

fn decode_color(bytes: &[u8]) -> Option<CheckpointColor> {
    match bytes {
        [0, 0, 0, 0] => Some(CheckpointColor::Default),
        [1, index, 0, 0] => Some(CheckpointColor::Indexed(*index)),
        [2, red, green, blue] => Some(CheckpointColor::Rgb(*red, *green, *blue)),
        _ => None,
    }
}

fn unsafe_display_scalar(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{2028}' | '\u{2029}' | '\u{061c}' | '\u{200e}' | '\u{200f}'
        )
        || matches!(character as u32, 0x202a..=0x202e | 0x2066..=0x206f)
}

fn read_u16(record: &[u8], offset: usize) -> Result<u16, ScreenCheckpointError> {
    let bytes = record
        .get(offset..offset + 2)
        .ok_or(ScreenCheckpointError::InvalidRecord)?;
    Ok(u16::from_be_bytes(
        bytes
            .try_into()
            .map_err(|_| ScreenCheckpointError::InvalidRecord)?,
    ))
}

fn read_u64(record: &[u8], offset: usize) -> Result<u64, ScreenCheckpointError> {
    let bytes = record
        .get(offset..offset + 8)
        .ok_or(ScreenCheckpointError::InvalidRecord)?;
    Ok(u64::from_be_bytes(
        bytes
            .try_into()
            .map_err(|_| ScreenCheckpointError::InvalidRecord)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_record(rows: u16, columns: u16) -> Vec<u8> {
        let mut record = Vec::with_capacity(
            SCREEN_CHECKPOINT_HEADER_BYTES
                + usize::from(rows) * usize::from(columns) * SCREEN_CHECKPOINT_CELL_BYTES,
        );
        record.extend_from_slice(MAGIC);
        for id in 1..=5 {
            record.extend_from_slice(&id_u64(id));
        }
        record.extend_from_slice(&0_u64.to_be_bytes());
        record.extend_from_slice(&1_u64.to_be_bytes());
        record.extend_from_slice(&1_u64.to_be_bytes());
        record.extend_from_slice(&rows.to_be_bytes());
        record.extend_from_slice(&columns.to_be_bytes());
        record.extend_from_slice(&0_u16.to_be_bytes());
        record.extend_from_slice(&0_u16.to_be_bytes());
        record.push(0);
        record.push(0);
        record.extend_from_slice(&[0; HEADER_RESERVED_BYTES]);
        record.resize(
            record.len() + usize::from(rows) * usize::from(columns) * SCREEN_CHECKPOINT_CELL_BYTES,
            0,
        );
        record
    }

    const fn id_u64(value: u64) -> [u8; 8] {
        value.to_be_bytes()
    }

    fn golden_record() -> Vec<u8> {
        let mut record = vec![
            // PLSCRN01.
            0x50, 0x4c, 0x53, 0x43, 0x52, 0x4e, 0x30, 0x31,
            // TargetRef values 1, 2, 3, 4, 5 in network byte order.
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0,
            0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5,
            // Output sequence 0, capture time 6, screen revision 7.
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 7,
            // Rows 5, columns 20, cursor (1, 2), alternate buffer, hidden cursor.
            0, 5, 0, 20, 0, 1, 0, 2, 1, 1, // Fourteen reserved bytes.
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(record.len(), SCREEN_CHECKPOINT_HEADER_BYTES);

        // Representative independent cell bytes: text "A", all five style
        // flags, indexed foreground 196, and RGB background (10, 20, 30).
        let mut cell = [0_u8; SCREEN_CHECKPOINT_CELL_BYTES];
        cell[0] = 1;
        cell[1] = FLAG_BOLD | FLAG_DIM | FLAG_ITALIC | FLAG_UNDERLINE | FLAG_INVERSE;
        cell[2..6].copy_from_slice(&[1, 196, 0, 0]);
        cell[6..10].copy_from_slice(&[2, 10, 20, 30]);
        cell[10] = b'A';
        record.extend_from_slice(&cell);
        record.resize(
            record.len() + (5 * 20 - 1) * SCREEN_CHECKPOINT_CELL_BYTES,
            0,
        );
        record
    }

    fn cell_start(row: u16, column: u16) -> usize {
        SCREEN_CHECKPOINT_HEADER_BYTES
            + (usize::from(row) * 20 + usize::from(column)) * SCREEN_CHECKPOINT_CELL_BYTES
    }

    fn put_cell_text(record: &mut [u8], row: u16, column: u16, flags: u8, text: &[u8]) {
        let start = cell_start(row, column);
        record[start + CELL_TEXT_LENGTH_OFFSET] = u8::try_from(text.len()).unwrap();
        record[start + CELL_FLAGS_OFFSET] = flags;
        record[start + CELL_TEXT_OFFSET..start + CELL_TEXT_OFFSET + text.len()]
            .copy_from_slice(text);
    }

    fn put_u16(record: &mut [u8], offset: usize, value: u16) {
        record[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u64(record: &mut [u8], offset: usize, value: u64) {
        record[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn golden_header_and_cell_offsets_decode_exact_values() {
        let record = golden_record();
        assert_eq!(&record[0..8], b"PLSCRN01");
        assert_eq!(&record[8..16], &[0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&record[16..24], &[0, 0, 0, 0, 0, 0, 0, 2]);
        assert_eq!(&record[24..32], &[0, 0, 0, 0, 0, 0, 0, 3]);
        assert_eq!(&record[32..40], &[0, 0, 0, 0, 0, 0, 0, 4]);
        assert_eq!(&record[40..48], &[0, 0, 0, 0, 0, 0, 0, 5]);
        assert_eq!(&record[48..56], &[0; 8]);
        assert_eq!(&record[56..64], &[0, 0, 0, 0, 0, 0, 0, 6]);
        assert_eq!(&record[64..72], &[0, 0, 0, 0, 0, 0, 0, 7]);
        assert_eq!(&record[72..82], &[0, 5, 0, 20, 0, 1, 0, 2, 1, 1]);
        assert!(record[82..96].iter().all(|byte| *byte == 0));
        assert_eq!(&record[96..106], &[1, 0x7c, 1, 196, 0, 0, 2, 10, 20, 30]);
        assert_eq!(&record[106..107], b"A");
        assert!(record[107..138].iter().all(|byte| *byte == 0));
        assert!(record[138..144].iter().all(|byte| *byte == 0));

        let checkpoint = decode(&record).unwrap();
        let metadata = checkpoint.metadata();
        assert_eq!(metadata.target().generation, 1);
        assert_eq!(metadata.target().workspace, 2);
        assert_eq!(metadata.target().pane, 3);
        assert_eq!(metadata.target().session, 4);
        assert_eq!(metadata.target().occupant_revision, 5);
        assert_eq!(metadata.output_sequence(), 0);
        assert_eq!(metadata.captured_at_unix_ms(), 6);
        assert_eq!(metadata.screen_revision(), 7);
        assert_eq!((checkpoint.rows(), checkpoint.columns()), (5, 20));
        assert_eq!(
            (checkpoint.cursor_row(), checkpoint.cursor_column()),
            (1, 2)
        );
        assert_eq!(checkpoint.buffer(), ScreenBuffer::Alternate);
        assert!(checkpoint.cursor_hidden());
        let first = checkpoint.cell(0, 0).unwrap();
        assert_eq!(first.text(), "A");
        assert_eq!(first.foreground(), Some(CheckpointColor::Indexed(196)));
        assert_eq!(first.background(), Some(CheckpointColor::Rgb(10, 20, 30)));
        assert!(
            first.bold() && first.dim() && first.italic() && first.underline() && first.inverse()
        );
        assert_eq!(checkpoint.cells().len(), 100);
        assert!(checkpoint.cell(5, 0).is_none());

        // Numeric metadata and color bytes may legitimately equal ESC's byte
        // value. Only cell text is subject to the display-scalar restriction.
        let mut numeric_esc = record;
        put_u64(&mut numeric_esc, TARGET_OFFSET, 27);
        numeric_esc[SCREEN_CHECKPOINT_HEADER_BYTES + CELL_BACKGROUND_OFFSET + 1] = 27;
        let decoded = decode(&numeric_esc).unwrap();
        assert_eq!(decoded.metadata().target().generation, 27);
        assert_eq!(
            decoded.cell(0, 0).unwrap().background(),
            Some(CheckpointColor::Rgb(27, 20, 30))
        );
    }

    #[test]
    fn minimum_and_maximum_viewports_and_exact_maximum_record_are_supported() {
        let minimum = empty_record(5, 20);
        let decoded_minimum = decode(&minimum).unwrap();
        assert_eq!((decoded_minimum.rows(), decoded_minimum.columns()), (5, 20));
        assert_eq!(decoded_minimum.cells().len(), 100);

        let maximum = empty_record(80, 240);
        assert_eq!(maximum.len(), MAX_SCREEN_CHECKPOINT_BYTES);
        let decoded_maximum = decode(&maximum).unwrap();
        assert_eq!(
            (decoded_maximum.rows(), decoded_maximum.columns()),
            (80, 240)
        );
        assert_eq!(decoded_maximum.cells().len(), 80 * 240);

        let mut one_too_many = maximum;
        one_too_many.push(0);
        assert_eq!(decode(&one_too_many), Err(ScreenCheckpointError::TooLarge));
    }

    #[test]
    fn rejects_every_truncated_prefix_and_trailing_body_bytes() {
        let record = golden_record();
        for length in 0..record.len() {
            assert!(
                decode(&record[..length]).is_err(),
                "accepted prefix {length}"
            );
        }
        let mut trailing = record;
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(ScreenCheckpointError::InvalidRecord));
        assert_eq!(
            decode(&[0; SCREEN_CHECKPOINT_HEADER_BYTES - 1]),
            Err(ScreenCheckpointError::TooShort)
        );
    }

    #[test]
    fn validates_all_header_ids_dimensions_cursor_flags_and_reserved_bytes() {
        for offset in [
            8,
            16,
            24,
            32,
            40,
            CAPTURE_TIME_OFFSET,
            SCREEN_REVISION_OFFSET,
        ] {
            let mut record = golden_record();
            put_u64(&mut record, offset, 0);
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
        let mut initial_sequence = empty_record(5, 20);
        put_u64(&mut initial_sequence, OUTPUT_SEQUENCE_OFFSET, u64::MAX);
        assert_eq!(
            decode(&initial_sequence)
                .unwrap()
                .metadata()
                .output_sequence(),
            u64::MAX
        );

        for (offset, value) in [
            (ROWS_OFFSET, 4),
            (ROWS_OFFSET, 81),
            (COLUMNS_OFFSET, 19),
            (COLUMNS_OFFSET, 241),
            (CURSOR_ROW_OFFSET, 5),
            (CURSOR_COLUMN_OFFSET, 20),
        ] {
            let mut record = golden_record();
            put_u16(&mut record, offset, value);
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
        for (offset, value) in [(BUFFER_OFFSET, 2), (CURSOR_HIDDEN_OFFSET, 2)] {
            let mut record = golden_record();
            record[offset] = value;
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
        for offset in HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_BYTES {
            let mut record = golden_record();
            record[offset] = 1;
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
        assert_eq!(
            CheckpointMetadata::new(
                TargetRef {
                    generation: 0,
                    workspace: 2,
                    pane: 3,
                    session: 4,
                    occupant_revision: 5,
                },
                0,
                1,
                1,
            ),
            Err(ScreenCheckpointError::InvalidInput)
        );
        assert_eq!(
            CheckpointMetadata::new(
                TargetRef {
                    generation: 1,
                    workspace: 2,
                    pane: 3,
                    session: 4,
                    occupant_revision: 5,
                },
                0,
                0,
                1,
            ),
            Err(ScreenCheckpointError::InvalidInput)
        );
    }

    #[test]
    fn rejects_invalid_cell_flags_colors_text_padding_and_reserved_bytes() {
        let mut invalid = golden_record();
        let first_cell = cell_start(0, 0);
        invalid[first_cell + CELL_FLAGS_OFFSET] = 0x80;
        assert_eq!(decode(&invalid), Err(ScreenCheckpointError::InvalidRecord));

        let mut both_wide_flags = golden_record();
        both_wide_flags[first_cell + CELL_FLAGS_OFFSET] = FLAG_WIDE_LEAD | FLAG_WIDE_CONTINUATION;
        assert_eq!(
            decode(&both_wide_flags),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        for bad_color in [[3, 1, 2, 3], [0, 1, 0, 0], [1, 1, 1, 0]] {
            let mut record = empty_record(5, 20);
            let start = cell_start(0, 0);
            record[start + CELL_FOREGROUND_OFFSET..start + CELL_BACKGROUND_OFFSET]
                .copy_from_slice(&bad_color);
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
        let mut rgb_color = empty_record(5, 20);
        let start = cell_start(0, 0);
        rgb_color[start + CELL_FOREGROUND_OFFSET..start + CELL_BACKGROUND_OFFSET]
            .copy_from_slice(&[2, 255, 128, 0]);
        assert_eq!(
            decode(&rgb_color).unwrap().cell(0, 0).unwrap().foreground(),
            Some(CheckpointColor::Rgb(255, 128, 0))
        );

        let mut too_long = empty_record(5, 20);
        too_long[first_cell + CELL_TEXT_LENGTH_OFFSET] = 33;
        assert_eq!(decode(&too_long), Err(ScreenCheckpointError::InvalidRecord));

        let mut invalid_utf8 = empty_record(5, 20);
        let invalid_start = first_cell;
        invalid_utf8[invalid_start] = 1;
        invalid_utf8[invalid_start + CELL_TEXT_OFFSET] = 0xff;
        assert_eq!(
            decode(&invalid_utf8),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        let mut bad_tail = empty_record(5, 20);
        let tail_start = first_cell;
        for offset in CELL_TEXT_OFFSET..CELL_TEXT_OFFSET + CELL_TEXT_BYTES {
            bad_tail[tail_start + offset] = 1;
            assert_eq!(decode(&bad_tail), Err(ScreenCheckpointError::InvalidRecord));
            bad_tail[tail_start + offset] = 0;
        }
        for offset in CELL_RESERVED_OFFSET..CELL_RESERVED_OFFSET + CELL_RESERVED_BYTES {
            let mut record = empty_record(5, 20);
            record[first_cell + offset] = 1;
            assert_eq!(decode(&record), Err(ScreenCheckpointError::InvalidRecord));
        }
    }

    #[test]
    fn accepts_valid_unicode_without_inferring_width_and_rejects_unsafe_scalars() {
        let supported = "λe\u{301}界";
        let mut record = empty_record(5, 20);
        put_cell_text(&mut record, 0, 0, 0, supported.as_bytes());
        assert_eq!(
            decode(&record).unwrap().cell(0, 0).unwrap().text(),
            supported
        );

        let mut unsafe_characters = vec![
            '\0', '\u{1b}', '\u{7f}', '\u{2028}', '\u{2029}', '\u{061c}', '\u{200e}', '\u{200f}',
        ];
        unsafe_characters.extend((0x202a..=0x202e).filter_map(char::from_u32));
        unsafe_characters.extend((0x2066..=0x206f).filter_map(char::from_u32));
        for character in unsafe_characters {
            let mut record = empty_record(5, 20);
            let mut encoded = [0; 4];
            put_cell_text(
                &mut record,
                0,
                0,
                0,
                character.encode_utf8(&mut encoded).as_bytes(),
            );
            assert_eq!(
                decode(&record),
                Err(ScreenCheckpointError::InvalidRecord),
                "accepted U+{:04X}",
                character as u32
            );
        }
        let mut osc = empty_record(5, 20);
        put_cell_text(&mut osc, 0, 0, 0, b"\x1b]52;c;U0NDUkVUIg==");
        assert_eq!(decode(&osc), Err(ScreenCheckpointError::InvalidRecord));
    }

    #[test]
    fn wide_cells_require_explicit_same_row_topology_without_unicode_guessing() {
        let mut valid_pair = empty_record(5, 20);
        put_cell_text(&mut valid_pair, 0, 18, FLAG_WIDE_LEAD, b"x");
        put_cell_text(&mut valid_pair, 0, 19, FLAG_WIDE_CONTINUATION, b"");
        let valid = decode(&valid_pair).unwrap();
        assert!(valid.cell(0, 18).unwrap().is_wide());
        assert!(valid.cell(0, 19).unwrap().is_wide_continuation());

        let mut lead_without_continuation = empty_record(5, 20);
        put_cell_text(
            &mut lead_without_continuation,
            0,
            18,
            FLAG_WIDE_LEAD,
            "界".as_bytes(),
        );
        assert_eq!(
            decode(&lead_without_continuation),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        let mut lead_at_edge = empty_record(5, 20);
        put_cell_text(&mut lead_at_edge, 0, 19, FLAG_WIDE_LEAD, "界".as_bytes());
        assert_eq!(
            decode(&lead_at_edge),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        let mut orphan_continuation = empty_record(5, 20);
        put_cell_text(&mut orphan_continuation, 0, 0, FLAG_WIDE_CONTINUATION, b"");
        assert_eq!(
            decode(&orphan_continuation),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        let mut nonempty_continuation = empty_record(5, 20);
        put_cell_text(&mut nonempty_continuation, 0, 0, FLAG_WIDE_LEAD, b"x");
        put_cell_text(
            &mut nonempty_continuation,
            0,
            1,
            FLAG_WIDE_CONTINUATION,
            b"y",
        );
        assert_eq!(
            decode(&nonempty_continuation),
            Err(ScreenCheckpointError::InvalidRecord)
        );

        let mut cross_row = empty_record(5, 20);
        put_cell_text(&mut cross_row, 0, 19, FLAG_WIDE_LEAD, b"x");
        put_cell_text(&mut cross_row, 1, 0, FLAG_WIDE_CONTINUATION, b"");
        assert_eq!(
            decode(&cross_row),
            Err(ScreenCheckpointError::InvalidRecord)
        );
    }

    #[test]
    fn debug_output_redacts_target_text_and_colors() {
        let mut record = empty_record(5, 20);
        put_cell_text(&mut record, 0, 0, 0, b"private-value");
        let checkpoint = decode(&record).unwrap();
        let cell = checkpoint.cell(0, 0).unwrap();
        assert!(!format!("{checkpoint:?}").contains("private-value"));
        assert!(!format!("{cell:?}").contains("private-value"));
        assert!(!format!("{:?}", cell.foreground().unwrap()).contains("Default"));
        assert_eq!(
            format!("{:?}", ScreenCheckpointError::InvalidRecord),
            "ScreenCheckpointError { <redacted> }"
        );
        assert_eq!(
            format!(
                "{:?}",
                CheckpointMetadata::new(
                    TargetRef {
                        generation: 1,
                        workspace: 2,
                        pane: 3,
                        session: 4,
                        occupant_revision: 5,
                    },
                    0,
                    1,
                    1,
                )
                .unwrap()
            ),
            "CheckpointMetadata { <redacted> }"
        );
    }
}
