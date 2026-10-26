//! Bounded, client-local text selection over a frozen terminal cell view.
//!
//! This module performs no host IO and has no clipboard access. Callers adapt
//! rendered/history cells and provide wrap metadata explicitly; unknown wrap
//! boundaries are never inferred. Captured text is intentionally omitted from
//! `Debug` output.

use crate::control_protocol::TargetRef;
use std::fmt;

/// Maximum UTF-8 bytes retained or returned for one selection.
pub const MAX_SELECTION_BYTES: usize = 1_048_576;
/// Maximum frozen grid shape, matching the terminal snapshot's 80 by 240 cap.
pub const MAX_SELECTION_CELLS: usize = 80 * 240;

/// Identity and geometry of the view from which a drag began.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewIdentity {
    /// Exact pane occupant identity.
    pub target: TargetRef,
    /// Client-local view identity (changes when the displayed source changes).
    pub view_id: u64,
    /// Screen/buffer epoch supplied by the owner.
    pub epoch: u64,
    /// Number of rows in the frozen view.
    pub rows: u16,
    /// Number of columns in the frozen view.
    pub columns: u16,
    /// Buffer identity supplied by the owner (for example normal/alternate).
    pub buffer_id: u64,
}

/// Whether the next physical row continues this row without a line break.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftWrap {
    /// The following row is a known soft continuation.
    Yes,
    /// This row ends with a hard line boundary.
    No,
    /// The producer has no trustworthy wrap information for this boundary.
    Unknown,
}

/// Borrowed cell text and its terminal width topology.
#[derive(Clone, Copy)]
pub struct Cell<'a> {
    /// Sanitized printable cell text; blank cells use an empty string.
    pub text: &'a str,
    /// True for a width-two leading cell.
    pub wide_lead: bool,
    /// True for the continuation cell following a width-two lead.
    pub continuation: bool,
}

/// Borrowed row adapter. `wrap_after` describes the boundary after this row.
pub struct Row<'a> {
    /// Cells in display order.
    pub cells: &'a [Cell<'a>],
    /// Explicit metadata for the boundary after this row; ignored on the last row.
    pub wrap_after: SoftWrap,
}

/// Coordinates are zero-based and clipped within the captured pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    /// Row within the pane content.
    pub row: u16,
    /// Column within the pane content.
    pub column: u16,
}

/// Reasons a selection cannot be started or copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionError {
    /// View dimensions or row shapes do not describe a rectangular grid.
    InvalidShape,
    /// Cell text is malformed or contains terminal control characters.
    InvalidText,
    /// The frozen view or resulting selection exceeds the fixed byte bound.
    TooLarge,
    /// A required row boundary has unknown soft-wrap state.
    UnknownWrap,
    /// A zero-distance click does not form a copyable selection.
    Empty,
    /// Current target, view, geometry, epoch, or buffer differs from the drag start.
    StaleView,
}

impl fmt::Debug for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Selection")
            .field("identity", &self.identity)
            .field("anchor", &self.anchor)
            .field("end", &self.end)
            .field("captured_bytes", &self.captured_bytes)
            .field("text", &"<redacted>")
            .finish()
    }
}

#[derive(Clone)]
struct OwnedCell {
    text: String,
    wide_lead: bool,
    continuation: bool,
}

/// Frozen selection state owns the drag-start screen contents.
pub struct Selection {
    identity: ViewIdentity,
    rows: Vec<Vec<OwnedCell>>,
    wraps: Vec<SoftWrap>,
    anchor: Point,
    end: Point,
    captured_bytes: usize,
}

/// Freeze a cell grid at drag start and clip the anchor to its pane.
pub fn begin(
    identity: ViewIdentity,
    rows: &[Row<'_>],
    anchor: Point,
) -> Result<Selection, SelectionError> {
    if identity.rows == 0
        || identity.columns == 0
        || usize::from(identity.rows) != rows.len()
        || usize::from(identity.rows)
            .checked_mul(usize::from(identity.columns))
            .is_none_or(|count| count > MAX_SELECTION_CELLS)
    {
        return Err(SelectionError::InvalidShape);
    }

    let mut frozen = Vec::new();
    let mut wraps = Vec::new();
    let mut captured_bytes = 0usize;
    frozen
        .try_reserve_exact(rows.len())
        .map_err(|_| SelectionError::TooLarge)?;
    wraps
        .try_reserve_exact(rows.len())
        .map_err(|_| SelectionError::TooLarge)?;
    for row in rows {
        if row.cells.len() != usize::from(identity.columns) {
            return Err(SelectionError::InvalidShape);
        }
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(row.cells.len())
            .map_err(|_| SelectionError::TooLarge)?;
        for (column, cell) in row.cells.iter().enumerate() {
            if !valid_cell(cell) {
                return Err(SelectionError::InvalidText);
            }
            if (cell.wide_lead
                && !row
                    .cells
                    .get(column + 1)
                    .is_some_and(|next| next.continuation))
                || (cell.continuation && (column == 0 || !row.cells[column - 1].wide_lead))
            {
                return Err(SelectionError::InvalidShape);
            }
            captured_bytes = captured_bytes
                .checked_add(cell.text.len())
                .ok_or(SelectionError::TooLarge)?;
            if captured_bytes > MAX_SELECTION_BYTES {
                return Err(SelectionError::TooLarge);
            }
            cells.push(OwnedCell {
                text: cell.text.to_owned(),
                wide_lead: cell.wide_lead,
                continuation: cell.continuation,
            });
        }
        frozen.push(cells);
        wraps.push(row.wrap_after);
    }
    let anchor = clip(identity, anchor);
    Ok(Selection {
        identity,
        rows: frozen,
        wraps,
        anchor,
        end: anchor,
        captured_bytes,
    })
}

impl Selection {
    /// Extend the drag endpoint, clipped to the same captured pane.
    pub fn extend(&mut self, point: Point) {
        self.end = clip(self.identity, point);
    }

    /// The identity must still match at release; mismatch cancels the selection.
    pub fn finish(&self, current: ViewIdentity) -> Result<String, SelectionError> {
        if current != self.identity {
            return Err(SelectionError::StaleView);
        }
        if self.anchor == self.end {
            return Err(SelectionError::Empty);
        }
        let (start, end) = ordered(self.anchor, self.end);
        let mut output = String::new();
        for row_index in usize::from(start.row)..=usize::from(end.row) {
            let first = if row_index == usize::from(start.row) {
                usize::from(start.column)
            } else {
                0
            };
            let last = if row_index == usize::from(end.row) {
                usize::from(end.column)
            } else {
                usize::from(self.identity.columns - 1)
            };
            let cells = self
                .rows
                .get(row_index)
                .ok_or(SelectionError::InvalidShape)?;
            let first = normalize_start(cells, first);
            let last = normalize_end(cells, last);
            let mut row_text = String::new();
            let mut column = first;
            while column <= last {
                let cell = cells.get(column).ok_or(SelectionError::InvalidShape)?;
                if !cell.continuation {
                    if cell.text.is_empty() {
                        row_text.push(' ');
                    } else {
                        row_text.push_str(&cell.text);
                    }
                    if cell.wide_lead {
                        column = column.checked_add(2).ok_or(SelectionError::TooLarge)?;
                        continue;
                    }
                }
                column = column.checked_add(1).ok_or(SelectionError::TooLarge)?;
            }
            // A soft-wrapped row is part of the same logical line: preserve
            // every selected cell, including spaces. Trim padding only at a
            // hard boundary or at the final selected logical line.
            let trim_padding =
                row_index == usize::from(end.row) || self.wraps[row_index] != SoftWrap::Yes;
            if trim_padding {
                while row_text.ends_with(' ') {
                    row_text.pop();
                }
            }
            append_bounded(&mut output, &row_text)?;
            if row_index < usize::from(end.row) {
                match self.wraps[row_index] {
                    SoftWrap::Yes => {}
                    SoftWrap::No => append_bounded(&mut output, "\n")?,
                    SoftWrap::Unknown => return Err(SelectionError::UnknownWrap),
                }
            }
        }
        if output.is_empty() {
            return Err(SelectionError::Empty);
        }
        Ok(output)
    }
}

fn valid_cell(cell: &Cell<'_>) -> bool {
    if cell.wide_lead && cell.continuation {
        return false;
    }
    if cell.continuation && !cell.text.is_empty() {
        return false;
    }
    if cell.wide_lead && cell.text.is_empty() {
        return false;
    }
    cell.text.chars().all(|ch| !ch.is_control())
}

fn clip(identity: ViewIdentity, point: Point) -> Point {
    Point {
        row: point.row.min(identity.rows.saturating_sub(1)),
        column: point.column.min(identity.columns.saturating_sub(1)),
    }
}

fn ordered(a: Point, b: Point) -> (Point, Point) {
    if (a.row, a.column) <= (b.row, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

fn normalize_start(row: &[OwnedCell], mut column: usize) -> usize {
    if row.get(column).is_some_and(|cell| cell.continuation) && column > 0 {
        column -= 1;
    }
    column
}

fn normalize_end(row: &[OwnedCell], mut column: usize) -> usize {
    if row.get(column).is_some_and(|cell| cell.wide_lead) && column + 1 < row.len() {
        column += 1;
    }
    column
}

fn append_bounded(output: &mut String, text: &str) -> Result<(), SelectionError> {
    let total = output
        .len()
        .checked_add(text.len())
        .ok_or(SelectionError::TooLarge)?;
    if total > MAX_SELECTION_BYTES {
        return Err(SelectionError::TooLarge);
    }
    output.push_str(text);
    Ok(())
}
