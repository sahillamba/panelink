//! Bounded adapters between rendered snapshot cells and the pure selection model.

use panelink::control_protocol::TargetRef;
use panelink::pane_layout::Rect;
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use panelink::terminal_selection::{
    self, Cell, Point, Row, Selection, SelectionError, SoftWrap, ViewIdentity,
};

/// One attached-client drag. Its debug form deliberately excludes frozen cells.
pub(super) struct FrozenDrag {
    pub(super) target: TargetRef,
    pub(super) slot: usize,
    pub(super) content_rect: Rect,
    pub(super) source_history: bool,
    pub(super) row_wraps: Vec<bool>,
    pub(super) identity: ViewIdentity,
    pub(super) frozen: ScreenSnapshot,
    pub(super) selection: Selection,
    pub(super) anchor: Point,
    pub(super) end: Point,
}

impl std::fmt::Debug for FrozenDrag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrozenDrag")
            .field("target", &self.target)
            .field("slot", &self.slot)
            .field("content_rect", &self.content_rect)
            .field("source_history", &self.source_history)
            .field("row_wraps", &self.row_wraps.len())
            .field("identity", &self.identity)
            .field("text", &"<redacted>")
            .finish()
    }
}

pub(super) fn capture(
    target: TargetRef,
    slot: usize,
    epoch: u64,
    content_rect: Rect,
    source_history: bool,
    snapshot: &ScreenSnapshot,
    point: Point,
) -> Result<FrozenDrag, SelectionError> {
    let source_wraps = snapshot.row_wraps().ok_or(SelectionError::UnknownWrap)?;
    if source_wraps.len() != usize::from(snapshot.rows()) {
        return Err(SelectionError::InvalidShape);
    }
    let mut row_wraps = Vec::new();
    row_wraps
        .try_reserve_exact(source_wraps.len())
        .map_err(|_| SelectionError::TooLarge)?;
    row_wraps.extend_from_slice(source_wraps);
    let cell_rows = cell_rows(snapshot)?;
    let model_rows = model_rows(snapshot, &cell_rows)?;
    let identity = identity(target, slot, epoch, snapshot);
    let selection = terminal_selection::begin(identity, &model_rows, point)?;
    let mut frozen = ScreenSnapshot::try_new(
        TerminalSize::new(snapshot.rows(), snapshot.columns())
            .map_err(|_| SelectionError::InvalidShape)?,
    )
    .map_err(|_| SelectionError::TooLarge)?;
    frozen.copy_from(snapshot);
    Ok(FrozenDrag {
        target,
        slot,
        content_rect,
        source_history,
        row_wraps,
        identity,
        frozen,
        selection,
        anchor: point,
        end: point,
    })
}

/// Reset the pure extraction range when keyboard copy mode starts a new
/// anchor. The frozen source and its identity remain unchanged.
pub(super) fn reanchor(drag: &mut FrozenDrag, point: Point) -> Result<(), SelectionError> {
    let cells = cell_rows(&drag.frozen)?;
    let rows = model_rows(&drag.frozen, &cells)?;
    drag.selection = terminal_selection::begin(drag.identity, &rows, point)?;
    drag.anchor = point;
    drag.end = point;
    Ok(())
}

fn cell_rows(snapshot: &ScreenSnapshot) -> Result<Vec<Vec<Cell<'_>>>, SelectionError> {
    let rows = usize::from(snapshot.rows());
    let columns = usize::from(snapshot.columns());
    let mut cell_rows = Vec::new();
    cell_rows
        .try_reserve_exact(rows)
        .map_err(|_| SelectionError::TooLarge)?;
    for row in 0..snapshot.rows() {
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(columns)
            .map_err(|_| SelectionError::TooLarge)?;
        for column in 0..snapshot.columns() {
            let cell = snapshot
                .cell(row, column)
                .ok_or(SelectionError::InvalidShape)?;
            cells.push(Cell {
                text: cell.text(),
                wide_lead: cell.is_wide(),
                continuation: cell.is_wide_continuation(),
            });
        }
        cell_rows.push(cells);
    }
    Ok(cell_rows)
}

fn model_rows<'a>(
    snapshot: &ScreenSnapshot,
    cell_rows: &'a [Vec<Cell<'a>>],
) -> Result<Vec<Row<'a>>, SelectionError> {
    let mut model_rows = Vec::new();
    model_rows
        .try_reserve_exact(cell_rows.len())
        .map_err(|_| SelectionError::TooLarge)?;
    for (index, cells) in cell_rows.iter().enumerate() {
        let wrap_after = if index + 1 == cell_rows.len() {
            SoftWrap::No
        } else {
            match snapshot.row_wrapped(index as u16) {
                Some(true) => SoftWrap::Yes,
                Some(false) => SoftWrap::No,
                None => SoftWrap::Unknown,
            }
        };
        model_rows.push(Row { cells, wrap_after });
    }
    Ok(model_rows)
}

fn identity(target: TargetRef, slot: usize, epoch: u64, snapshot: &ScreenSnapshot) -> ViewIdentity {
    ViewIdentity {
        target,
        view_id: slot as u64,
        epoch,
        rows: snapshot.rows(),
        columns: snapshot.columns(),
        buffer_id: u64::from(snapshot.alternate_screen()),
    }
}
