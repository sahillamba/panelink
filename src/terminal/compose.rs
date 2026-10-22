//! Pure, bounded composition of up to four pane snapshots and dividers
//! and an optional ASCII overlay into one host terminal frame.
//!
//! The compositor writes ANSI straight to a caller-supplied writer and keeps
//! only fixed, preallocated baselines. It performs no host IO of its own and
//! contains no menu logic; callers describe each frame with [`ComposeFrame`].

use std::io::{self, Write};

use super::render::{paint_row, paint_row_diff, validate_snapshot, Placement};
use super::{io_error, ScreenSnapshot, TerminalError, TerminalSize};
use crate::pane_layout::Rect;

/// Largest pane or overlay height accepted by the compositor.
pub const MAX_RECT_ROWS: u16 = 80;
/// Largest pane or overlay width accepted by the compositor.
pub const MAX_RECT_COLUMNS: u16 = 240;

const MAX_ROWS: usize = MAX_RECT_ROWS as usize;
const MAX_COLUMNS: usize = MAX_RECT_COLUMNS as usize;
const MAX_OVERLAY_CELLS: usize = MAX_ROWS * MAX_COLUMNS;

/// Content shown inside one pane rectangle.
#[derive(Clone, Copy, Debug)]
pub enum PaneContent<'a> {
    /// A sanitized terminal snapshot whose dimensions equal the pane rectangle.
    Snapshot(&'a ScreenSnapshot),
    /// A blank pane with one centered, clipped status line (for example a
    /// starting, launch-failed or resize-pending pane). Only printable ASCII
    /// is shown; every other scalar is drawn as `?`.
    Placeholder(&'a str),
}

/// One pane placed at a host rectangle.
#[derive(Clone, Copy, Debug)]
pub struct PaneView<'a> {
    /// Host cells occupied by the pane.
    pub rect: Rect,
    /// What to draw inside the rectangle.
    pub content: PaneContent<'a>,
}

/// Presentation of one overlay row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OverlayStyle {
    /// Default attributes.
    #[default]
    Normal,
    /// Reverse video, for the highlighted item.
    Selected,
    /// Dim, for an unavailable item.
    Disabled,
    /// Default attributes, for frame or title rows.
    Border,
    /// Bold, for a status or reason row.
    Status,
}

/// One overlay row of text. Only printable ASCII is shown; every other scalar
/// is drawn as `?`. Text is clipped to the overlay width and never wraps.
#[derive(Clone, Copy, Debug)]
pub struct OverlayLine<'a> {
    /// Row text.
    pub text: &'a str,
    /// Row presentation; it applies to the full overlay width.
    pub style: OverlayStyle,
}

/// An opaque overlay painted above every pane and the divider.
#[derive(Clone, Copy, Debug)]
pub struct OverlayView<'a> {
    /// Host cells covered by the overlay. Every cell is painted.
    pub rect: Rect,
    /// Rows from the top of `rect`; missing rows are blank and extra rows are
    /// ignored.
    pub lines: &'a [OverlayLine<'a>],
}

/// Borrowed description of one complete host frame.
#[derive(Clone, Copy, Debug)]
pub struct ComposeFrame<'a> {
    /// Host viewport height.
    pub host_rows: u16,
    /// Host viewport width.
    pub host_columns: u16,
    /// Up to four non-overlapping panes, in stable slot order.
    pub panes: [Option<PaneView<'a>>; crate::pane_layout::MAX_PANES],
    /// First one-cell-thick divider, retained for compatibility.
    pub divider: Option<Rect>,
    /// All one-cell-thick dividers in stable traversal order.
    pub dividers: [Option<Rect>; crate::pane_layout::MAX_PANES - 1],
    /// Slot whose cursor is shown, when it holds a snapshot.
    pub focused: Option<usize>,
    /// Overlay drawn last; while present the host cursor is hidden.
    pub overlay: Option<OverlayView<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotKind {
    Empty,
    Snapshot,
    Placeholder,
}

#[derive(Clone, Copy)]
struct SlotMeta {
    rect: Option<Rect>,
    kind: SlotKind,
    placeholder: [u8; MAX_COLUMNS],
    placeholder_len: u16,
}

impl SlotMeta {
    const EMPTY: Self = Self {
        rect: None,
        kind: SlotKind::Empty,
        placeholder: [0; MAX_COLUMNS],
        placeholder_len: 0,
    };

    fn placeholder_text(&self) -> &[u8] {
        &self.placeholder[..usize::from(self.placeholder_len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorState {
    Hidden,
    Visible { row: u16, column: u16 },
}

#[derive(Clone, Copy)]
struct FrameMeta {
    host_rows: u16,
    host_columns: u16,
    slots: [SlotMeta; crate::pane_layout::MAX_PANES],
    divider: Option<Rect>,
    dividers: [Option<Rect>; crate::pane_layout::MAX_PANES - 1],
    overlay: Option<Rect>,
    overlay_styles: [OverlayStyle; MAX_ROWS],
    cursor: CursorState,
}

impl FrameMeta {
    const EMPTY: Self = Self {
        host_rows: 0,
        host_columns: 0,
        slots: [SlotMeta::EMPTY; crate::pane_layout::MAX_PANES],
        divider: None,
        dividers: [None; crate::pane_layout::MAX_PANES - 1],
        overlay: None,
        overlay_styles: [OverlayStyle::Normal; MAX_ROWS],
        cursor: CursorState::Hidden,
    };
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PaneWork {
    None,
    Full,
    Diff,
}

/// Incremental multi-pane compositor with fixed retained state.
///
/// [`Compositor::render`] writes one frame against the last *committed*
/// baseline. The caller calls [`Compositor::commit`] only after every byte of
/// that frame was accepted by the host; until then the baseline is unchanged
/// and a later render re-emits the same difference. If a frame is abandoned
/// after a partial write, call [`Compositor::invalidate`] so the next frame is
/// a full repaint. A writer error inside `render` invalidates automatically.
pub struct Compositor {
    displayed: [Option<ScreenSnapshot>; crate::pane_layout::MAX_PANES],
    target: [Option<ScreenSnapshot>; crate::pane_layout::MAX_PANES],
    displayed_overlay: Box<[u8]>,
    target_overlay: Box<[u8]>,
    displayed_meta: FrameMeta,
    pending_meta: FrameMeta,
    valid: bool,
    pending_ready: bool,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("valid", &self.valid)
            .field("pending_ready", &self.pending_ready)
            .finish_non_exhaustive()
    }
}

impl Compositor {
    /// Reserve fixed slot arrays and the maximum-size overlay. Pane snapshots
    /// allocate only to the active pane dimensions as frames are staged.
    pub fn try_new() -> Result<Self, TerminalError> {
        Ok(Self {
            displayed: std::array::from_fn(|_| None),
            target: std::array::from_fn(|_| None),
            displayed_overlay: overlay_buffer()?,
            target_overlay: overlay_buffer()?,
            displayed_meta: FrameMeta::EMPTY,
            pending_meta: FrameMeta::EMPTY,
            valid: false,
            pending_ready: false,
        })
    }

    /// Force the next rendered frame to clear and repaint the whole host.
    pub fn invalidate(&mut self) {
        self.valid = false;
        self.pending_ready = false;
    }

    /// Advance the baseline to the most recently rendered frame.
    ///
    /// Returns `false`, changing nothing, when no successfully rendered frame
    /// is pending (none rendered, already committed, or the render failed).
    pub fn commit(&mut self) -> bool {
        if !self.pending_ready {
            return false;
        }
        for slot in 0..crate::pane_layout::MAX_PANES {
            if self.pending_meta.slots[slot].kind == SlotKind::Snapshot {
                let (displayed, target) = (&mut self.displayed, &mut self.target);
                std::mem::swap(&mut displayed[slot], &mut target[slot]);
            }
        }
        std::mem::swap(&mut self.displayed_overlay, &mut self.target_overlay);
        self.displayed_meta = self.pending_meta;
        self.valid = true;
        self.pending_ready = false;
        true
    }

    /// Validate `frame` and write it as ANSI to `output`.
    ///
    /// Invalid geometry is rejected with [`io::ErrorKind::InvalidInput`] (or
    /// `InvalidData` for inconsistent snapshot storage) before any byte is
    /// written. The first frame, an invalidated baseline, or any host size,
    /// pane rectangle or divider change clears and repaints everything;
    /// otherwise only differences against the committed baseline are written.
    pub fn render<W: Write>(&mut self, frame: &ComposeFrame<'_>, output: &mut W) -> io::Result<()> {
        self.pending_ready = false;
        validate_frame(frame)?;
        self.stage(frame);
        match self.write_frame(output) {
            Ok(()) => {
                self.pending_ready = true;
                Ok(())
            }
            Err(error) => {
                self.valid = false;
                Err(error)
            }
        }
    }

    fn stage(&mut self, frame: &ComposeFrame<'_>) {
        let mut meta = FrameMeta::EMPTY;
        meta.host_rows = frame.host_rows;
        meta.host_columns = frame.host_columns;
        meta.divider = frame.divider;
        meta.dividers = if frame.dividers.iter().any(Option::is_some) {
            frame.dividers
        } else {
            let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
            dividers[0] = frame.divider;
            dividers
        };

        for (slot, pane) in frame.panes.iter().enumerate() {
            let Some(pane) = pane else {
                continue;
            };
            let slot_meta = &mut meta.slots[slot];
            slot_meta.rect = Some(pane.rect);
            match pane.content {
                PaneContent::Snapshot(snapshot) => {
                    slot_meta.kind = SlotKind::Snapshot;
                    match self.target[slot].as_mut() {
                        Some(target) => target.copy_from(snapshot),
                        None => {
                            let size = TerminalSize::new(pane.rect.rows, pane.rect.columns)
                                .expect("validated pane dimensions");
                            let mut target = ScreenSnapshot::try_new(size)
                                .expect("bounded compositor pane allocation");
                            target.copy_from(snapshot);
                            self.target[slot] = Some(target);
                        }
                    }
                }
                PaneContent::Placeholder(text) => {
                    slot_meta.kind = SlotKind::Placeholder;
                    let limit = usize::from(pane.rect.columns).min(MAX_COLUMNS);
                    let mut len = 0;
                    for byte in text.chars().take(limit).map(ascii_cell) {
                        slot_meta.placeholder[len] = byte;
                        len += 1;
                    }
                    slot_meta.placeholder_len = u16::try_from(len).unwrap_or(0);
                }
            }
        }

        let overlay = frame
            .overlay
            .map(|overlay| (overlay, clip_to_host(overlay.rect, frame)));
        if let Some((overlay, rect)) = overlay {
            meta.overlay = Some(rect);
            let columns = usize::from(rect.columns);
            for row in 0..usize::from(rect.rows) {
                let line = overlay.lines.get(row);
                meta.overlay_styles[row] = line.map_or(OverlayStyle::Normal, |line| line.style);
                let cells = &mut self.target_overlay[row * columns..(row + 1) * columns];
                cells.fill(b' ');
                if let Some(line) = line {
                    for (cell, scalar) in cells.iter_mut().zip(line.text.chars()) {
                        *cell = ascii_cell(scalar);
                    }
                }
            }
        }

        meta.cursor = match (frame.overlay, frame.focused) {
            (None, Some(slot)) => match frame.panes.get(slot).copied().flatten() {
                Some(PaneView {
                    rect,
                    content: PaneContent::Snapshot(snapshot),
                }) if !snapshot.cursor_hidden() => CursorState::Visible {
                    row: rect.y + snapshot.cursor_row().min(snapshot.rows() - 1),
                    column: rect.x + snapshot.cursor_column().min(snapshot.columns() - 1),
                },
                _ => CursorState::Hidden,
            },
            _ => CursorState::Hidden,
        };
        self.pending_meta = meta;
    }

    fn needs_full(&self) -> bool {
        let pending = &self.pending_meta;
        let displayed = &self.displayed_meta;
        !self.valid
            || pending.host_rows != displayed.host_rows
            || pending.host_columns != displayed.host_columns
            || pending.divider != displayed.divider
            || pending.dividers != displayed.dividers
            || pending
                .slots
                .iter()
                .zip(displayed.slots.iter())
                .any(|(pending, displayed)| pending.rect != displayed.rect)
    }

    fn overlay_unchanged(&self) -> bool {
        let pending = &self.pending_meta;
        let displayed = &self.displayed_meta;
        if pending.overlay != displayed.overlay {
            return false;
        }
        let Some(rect) = pending.overlay else {
            return true;
        };
        let rows = usize::from(rect.rows);
        let cells = rows * usize::from(rect.columns);
        pending.overlay_styles[..rows] == displayed.overlay_styles[..rows]
            && self.target_overlay[..cells] == self.displayed_overlay[..cells]
    }

    fn pane_work(&self, slot: usize, overlay_moved: bool) -> PaneWork {
        let pending = &self.pending_meta.slots[slot];
        let displayed = &self.displayed_meta.slots[slot];
        match (pending.kind, displayed.kind) {
            (SlotKind::Empty, _) => PaneWork::None,
            (SlotKind::Snapshot, SlotKind::Snapshot) => {
                if overlay_moved
                    || self.target[slot]
                        .as_ref()
                        .zip(self.displayed[slot].as_ref())
                        .is_none_or(|(target, displayed)| target.cells() != displayed.cells())
                {
                    PaneWork::Diff
                } else {
                    PaneWork::None
                }
            }
            (SlotKind::Placeholder, SlotKind::Placeholder)
                if pending.placeholder_text() == displayed.placeholder_text() =>
            {
                if overlay_moved {
                    PaneWork::Full
                } else {
                    PaneWork::None
                }
            }
            _ => PaneWork::Full,
        }
    }

    fn write_frame<W: Write>(&self, output: &mut W) -> io::Result<()> {
        if self.needs_full() {
            return self.write_full(output);
        }

        let pending = &self.pending_meta;
        let displayed = &self.displayed_meta;
        let overlay_moved = pending.overlay != displayed.overlay;
        let overlay_changed = !self.overlay_unchanged();
        let damage = if overlay_moved {
            [displayed.overlay, pending.overlay]
        } else {
            [None, None]
        };
        let work: [PaneWork; crate::pane_layout::MAX_PANES] =
            std::array::from_fn(|slot| self.pane_work(slot, overlay_moved));

        if !overlay_changed && work.iter().all(|work| *work == PaneWork::None) {
            return write_cursor_update(output, displayed.cursor, pending.cursor);
        }

        if displayed.cursor != CursorState::Hidden {
            output.write_all(b"\x1b[?25l")?;
        }
        output.write_all(b"\x1b[?7l")?;
        for (slot, work) in work.into_iter().enumerate() {
            match work {
                PaneWork::None => {}
                PaneWork::Full => self.paint_pane(output, slot, None, damage)?,
                PaneWork::Diff => {
                    self.paint_pane(output, slot, self.displayed[slot].as_ref(), damage)?;
                }
            }
        }
        if let (true, Some(old)) = (overlay_moved, displayed.overlay) {
            self.paint_divider(output, Some(old))?;
            self.paint_exposed_exterior(output, old)?;
        }
        if overlay_changed {
            self.paint_overlay(output)?;
        }
        self.finish(output)
    }

    fn write_full<W: Write>(&self, output: &mut W) -> io::Result<()> {
        output.write_all(b"\x1b[?25l")?;
        output.write_all(b"\x1b[?7l")?;
        output.write_all(b"\x1b[0m")?;
        output.write_all(b"\x1b[2J")?;
        for slot in 0..crate::pane_layout::MAX_PANES {
            self.paint_pane(output, slot, None, [None, None])?;
        }
        self.paint_divider(output, None)?;
        self.paint_overlay(output)?;
        self.finish(output)
    }

    fn finish<W: Write>(&self, output: &mut W) -> io::Result<()> {
        output.write_all(b"\x1b[0m")?;
        output.write_all(b"\x1b[?7h")?;
        if let CursorState::Visible { row, column } = self.pending_meta.cursor {
            write_position(output, row, column)?;
            output.write_all(b"\x1b[?25h")?;
        }
        Ok(())
    }

    /// Paint one pane. With `previous`, rows outside `damage` are diffed and
    /// rows intersecting a damaged host row range are repainted in full.
    fn paint_pane<W: Write>(
        &self,
        output: &mut W,
        slot: usize,
        previous: Option<&ScreenSnapshot>,
        damage: [Option<Rect>; 2],
    ) -> io::Result<()> {
        let meta = &self.pending_meta.slots[slot];
        let Some(rect) = meta.rect else {
            return Ok(());
        };
        let exclude = self.pending_meta.overlay;
        match meta.kind {
            SlotKind::Empty => Ok(()),
            SlotKind::Snapshot => {
                let snapshot = self.target[slot]
                    .as_ref()
                    .ok_or_else(|| invalid("snapshot slot missing"))?;
                let placement = Placement {
                    row: rect.y,
                    column: rect.x,
                    exclude,
                };
                for row in 0..snapshot.rows() {
                    let host_row = rect.y + row;
                    let damaged = damage
                        .iter()
                        .flatten()
                        .any(|damage| row_in(*damage, host_row));
                    match previous {
                        Some(previous) if !damaged => {
                            paint_row_diff(output, snapshot, previous, row, placement)?;
                        }
                        _ => paint_row(output, snapshot, row, placement)?,
                    }
                }
                Ok(())
            }
            SlotKind::Placeholder => {
                output.write_all(b"\x1b[0m")?;
                let text = meta.placeholder_text();
                let text_len = u16::try_from(text.len()).unwrap_or(0);
                let text_row = (rect.rows - 1) / 2;
                let text_column = (rect.columns - text_len) / 2;
                for row in 0..rect.rows {
                    let host_row = rect.y + row;
                    write_ascii_run(
                        output,
                        host_row,
                        rect.x,
                        rect.columns,
                        |column| excluded(exclude, host_row, column),
                        |offset| {
                            if row == text_row && offset >= text_column {
                                text.get(usize::from(offset - text_column))
                                    .copied()
                                    .unwrap_or(b' ')
                            } else {
                                b' '
                            }
                        },
                    )?;
                }
                Ok(())
            }
        }
    }

    /// Paint the divider, or with `only`, just its cells inside that rectangle.
    fn paint_divider<W: Write>(&self, output: &mut W, only: Option<Rect>) -> io::Result<()> {
        for divider in self.pending_meta.dividers.iter().flatten().copied() {
            output.write_all(b"\x1b[0m")?;
            let glyph = if divider.columns == 1 { b'|' } else { b'-' };
            let exclude = self.pending_meta.overlay;
            for row in divider.y..divider.y + divider.rows {
                write_ascii_run(
                    output,
                    row,
                    divider.x,
                    divider.columns,
                    |column| {
                        excluded(exclude, row, column)
                            || only.is_some_and(|only| !only.contains(column, row))
                    },
                    |_| glyph,
                )?;
            }
        }
        Ok(())
    }

    /// Blank cells of a removed or moved overlay that no pane, divider or new
    /// overlay will repaint (host area outside every pane).
    fn paint_exposed_exterior<W: Write>(&self, output: &mut W, old: Rect) -> io::Result<()> {
        let meta = &self.pending_meta;
        let covered = |row: u16, column: u16| {
            excluded(meta.overlay, row, column)
                || meta
                    .dividers
                    .iter()
                    .flatten()
                    .any(|divider| excluded(Some(*divider), row, column))
                || meta
                    .slots
                    .iter()
                    .any(|slot| excluded(slot.rect, row, column))
        };
        output.write_all(b"\x1b[0m")?;
        for row in old.y..old.y + old.rows {
            write_ascii_run(
                output,
                row,
                old.x,
                old.columns,
                |column| covered(row, column),
                |_| b' ',
            )?;
        }
        Ok(())
    }

    fn paint_overlay<W: Write>(&self, output: &mut W) -> io::Result<()> {
        let Some(rect) = self.pending_meta.overlay else {
            return Ok(());
        };
        let columns = usize::from(rect.columns);
        for row in 0..rect.rows {
            let style = self.pending_meta.overlay_styles[usize::from(row)];
            output.write_all(style_sgr(style))?;
            let start = usize::from(row) * columns;
            let cells = &self.target_overlay[start..start + columns];
            write_ascii_run(
                output,
                rect.y + row,
                rect.x,
                rect.columns,
                |_| false,
                |offset| cells[usize::from(offset)],
            )?;
        }
        Ok(())
    }
}

fn overlay_buffer() -> Result<Box<[u8]>, TerminalError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(MAX_OVERLAY_CELLS).map_err(|_| {
        io_error(
            "compositor allocation",
            io::Error::from(io::ErrorKind::OutOfMemory),
        )
    })?;
    bytes.resize(MAX_OVERLAY_CELLS, b' ');
    Ok(bytes.into_boxed_slice())
}

fn ascii_cell(scalar: char) -> u8 {
    if scalar.is_ascii_graphic() || scalar == ' ' {
        scalar as u8
    } else {
        b'?'
    }
}

fn style_sgr(style: OverlayStyle) -> &'static [u8] {
    match style {
        OverlayStyle::Normal | OverlayStyle::Border => b"\x1b[0m",
        OverlayStyle::Selected => b"\x1b[0;7m",
        OverlayStyle::Disabled => b"\x1b[0;2m",
        OverlayStyle::Status => b"\x1b[0;1m",
    }
}

fn excluded(rect: Option<Rect>, row: u16, column: u16) -> bool {
    rect.is_some_and(|rect| rect.contains(column, row))
}

fn row_in(rect: Rect, row: u16) -> bool {
    row >= rect.y && u32::from(row) < u32::from(rect.y) + u32::from(rect.rows)
}

fn write_position<W: Write>(output: &mut W, row: u16, column: u16) -> io::Result<()> {
    write!(
        output,
        "\x1b[{};{}H",
        u32::from(row) + 1,
        u32::from(column) + 1
    )
}

/// Write `len` single-width ASCII cells starting at host `(row, column)`,
/// skipping host columns for which `skip` is true and repositioning after
/// each skipped run.
fn write_ascii_run<W: Write>(
    output: &mut W,
    row: u16,
    column: u16,
    len: u16,
    skip: impl Fn(u16) -> bool,
    byte_at: impl Fn(u16) -> u8,
) -> io::Result<()> {
    let mut positioned = false;
    for offset in 0..len {
        let host_column = column + offset;
        if skip(host_column) {
            positioned = false;
            continue;
        }
        if !positioned {
            write_position(output, row, host_column)?;
            positioned = true;
        }
        output.write_all(&[byte_at(offset)])?;
    }
    Ok(())
}

fn write_cursor_update<W: Write>(
    output: &mut W,
    displayed: CursorState,
    pending: CursorState,
) -> io::Result<()> {
    match (displayed, pending) {
        (CursorState::Hidden, CursorState::Hidden) => Ok(()),
        (CursorState::Visible { .. }, CursorState::Hidden) => output.write_all(b"\x1b[?25l"),
        (CursorState::Hidden, CursorState::Visible { row, column }) => {
            write_position(output, row, column)?;
            output.write_all(b"\x1b[?25h")
        }
        (CursorState::Visible { .. }, CursorState::Visible { row, column }) => {
            if displayed != pending {
                write_position(output, row, column)?;
            }
            Ok(())
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Clip a rectangle's right and bottom edges to the host viewport. The origin
/// is kept, so overlay lines stay anchored to the requested top-left cell.
fn clip_to_host(rect: Rect, frame: &ComposeFrame<'_>) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        rows: rect.rows.min(frame.host_rows.saturating_sub(rect.y)),
        columns: rect.columns.min(frame.host_columns.saturating_sub(rect.x)),
    }
}

fn rect_fits(rect: Rect, host_rows: u16, host_columns: u16) -> bool {
    rect.rows > 0
        && rect.columns > 0
        && u32::from(rect.x) + u32::from(rect.columns) <= u32::from(host_columns)
        && u32::from(rect.y) + u32::from(rect.rows) <= u32::from(host_rows)
}

fn within_limits(rect: Rect) -> bool {
    rect.rows <= MAX_RECT_ROWS && rect.columns <= MAX_RECT_COLUMNS
}

fn overlaps(left: Rect, right: Rect) -> bool {
    let (left_x, left_y) = (u32::from(left.x), u32::from(left.y));
    let (right_x, right_y) = (u32::from(right.x), u32::from(right.y));
    left_x < right_x + u32::from(right.columns)
        && right_x < left_x + u32::from(left.columns)
        && left_y < right_y + u32::from(right.rows)
        && right_y < left_y + u32::from(left.rows)
}

fn validate_frame(frame: &ComposeFrame<'_>) -> io::Result<()> {
    let (host_rows, host_columns) = (frame.host_rows, frame.host_columns);
    if host_rows == 0 || host_columns == 0 {
        return Err(invalid("compose host viewport is empty"));
    }

    for pane in frame.panes.iter().flatten() {
        if !rect_fits(pane.rect, host_rows, host_columns) {
            return Err(invalid("pane rectangle is outside the host viewport"));
        }
        if !within_limits(pane.rect) {
            return Err(invalid("pane rectangle exceeds terminal bounds"));
        }
        if let PaneContent::Snapshot(snapshot) = pane.content {
            if snapshot.rows() != pane.rect.rows || snapshot.columns() != pane.rect.columns {
                return Err(invalid("pane snapshot does not match its rectangle"));
            }
            validate_snapshot(snapshot, pane.rect.rows, pane.rect.columns)?;
        }
    }

    let dividers = if frame.dividers.iter().any(Option::is_some) {
        frame.dividers
    } else {
        let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
        dividers[0] = frame.divider;
        dividers
    };
    for divider in dividers.iter().flatten() {
        if !rect_fits(*divider, host_rows, host_columns)
            || (divider.columns != 1 && divider.rows != 1)
            || frame
                .panes
                .iter()
                .flatten()
                .any(|pane| overlaps(pane.rect, *divider))
        {
            return Err(invalid("divider rectangle is invalid"));
        }
    }
    let mut covered = [false; MAX_RECT_ROWS as usize * MAX_RECT_COLUMNS as usize];
    for pane in frame.panes.iter().flatten() {
        for row in pane.rect.y..pane.rect.y + pane.rect.rows {
            for column in pane.rect.x..pane.rect.x + pane.rect.columns {
                let index = usize::from(row) * MAX_RECT_COLUMNS as usize + usize::from(column);
                if std::mem::replace(&mut covered[index], true) {
                    return Err(invalid("pane rectangles overlap"));
                }
            }
        }
    }
    for divider in dividers.iter().flatten() {
        for row in divider.y..divider.y + divider.rows {
            for column in divider.x..divider.x + divider.columns {
                let index = usize::from(row) * MAX_RECT_COLUMNS as usize + usize::from(column);
                if std::mem::replace(&mut covered[index], true) {
                    return Err(invalid("divider rectangles overlap"));
                }
            }
        }
    }
    if frame.panes.iter().flatten().count() <= 2 {
        return Ok(());
    }
    let mut cells = [false; 80 * 240];
    for pane in frame.panes.iter().flatten() {
        for row in pane.rect.y..pane.rect.y + pane.rect.rows {
            for column in pane.rect.x..pane.rect.x + pane.rect.columns {
                cells[usize::from(row) * 240 + usize::from(column)] = true;
            }
        }
    }
    for divider in dividers.iter().flatten() {
        for row in divider.y..divider.y + divider.rows {
            for column in divider.x..divider.x + divider.columns {
                cells[usize::from(row) * 240 + usize::from(column)] = true;
            }
        }
    }
    for row in 0..host_rows {
        for column in 0..host_columns {
            if !cells[usize::from(row) * 240 + usize::from(column)] {
                return Err(invalid(
                    "pane and divider geometry does not partition the host",
                ));
            }
        }
    }
    for divider in dividers.iter().flatten() {
        let full_span = (divider.columns == 1 && divider.rows == host_rows)
            || (divider.rows == 1 && divider.columns == host_columns);
        if !full_span
            && frame.dividers.iter().flatten().count() <= 1
            && frame.panes.iter().flatten().count() > 2
        {
            return Err(invalid("divider must span its complete parent region"));
        }
    }

    if let Some(overlay) = frame.overlay {
        let clipped = clip_to_host(overlay.rect, frame);
        if overlay.rect.x >= host_columns
            || overlay.rect.y >= host_rows
            || overlay.rect.rows == 0
            || overlay.rect.columns == 0
            || !within_limits(clipped)
        {
            return Err(invalid("overlay rectangle is invalid"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{CoreCallbacks, SnapshotCell};
    use vt100::Parser;

    fn rect(y: u16, x: u16, rows: u16, columns: u16) -> Rect {
        Rect {
            x,
            y,
            rows,
            columns,
        }
    }

    fn snapshot_from(input: &str, rows: u16, columns: u16) -> ScreenSnapshot {
        let size = TerminalSize::new(rows, columns).unwrap();
        let mut parser = Parser::new_with_callbacks(rows, columns, 0, CoreCallbacks::new());
        parser.process(input.as_bytes());
        let mut snapshot = ScreenSnapshot::new(size);
        snapshot.refresh(parser.screen(), size);
        snapshot
    }

    fn set_text(cell: &mut SnapshotCell, text: &str) {
        assert!(text.len() <= cell.text.len());
        cell.text.fill(0);
        cell.text[..text.len()].copy_from_slice(text.as_bytes());
        cell.text_len = u8::try_from(text.len()).unwrap();
    }

    /// Host 10x41: left pane, one-column divider at column 20, right pane.
    fn side_by_side<'a>(left: &'a ScreenSnapshot, right: &'a ScreenSnapshot) -> ComposeFrame<'a> {
        ComposeFrame {
            host_rows: 10,
            host_columns: 41,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 10, 20),
                    content: PaneContent::Snapshot(left),
                }),
                Some(PaneView {
                    rect: rect(0, 21, 10, 20),
                    content: PaneContent::Snapshot(right),
                }),
                None,
                None,
            ],
            divider: Some(rect(0, 20, 10, 1)),
            dividers: [Some(rect(0, 20, 10, 1)), None, None],
            focused: None,
            overlay: None,
        }
    }

    fn rendered(compositor: &mut Compositor, frame: &ComposeFrame<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        compositor.render(frame, &mut bytes).unwrap();
        bytes
    }

    fn rendered_committed(compositor: &mut Compositor, frame: &ComposeFrame<'_>) -> Vec<u8> {
        let bytes = rendered(compositor, frame);
        assert!(compositor.commit());
        bytes
    }

    fn fresh_render(frame: &ComposeFrame<'_>) -> Vec<u8> {
        rendered(&mut Compositor::try_new().unwrap(), frame)
    }

    fn screen(bytes: &[u8], frame: &ComposeFrame<'_>) -> Parser {
        let mut parser = Parser::new(frame.host_rows, frame.host_columns, 0);
        parser.process(bytes);
        parser
    }

    fn cell_text(parser: &Parser, row: u16, column: u16) -> String {
        parser
            .screen()
            .cell(row, column)
            .unwrap()
            .contents()
            .to_owned()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    /// Minimal test interpreter: every `ESC [ row ; column H` in the output.
    fn positions(bytes: &[u8]) -> Vec<(u16, u16)> {
        let text = std::str::from_utf8(bytes).unwrap();
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("\x1b[") {
            rest = &rest[start + 2..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit() && c != ';' && c != '?')
                .unwrap();
            if rest[end..].starts_with('H') {
                let (row, column) = rest[..end].split_once(';').unwrap();
                found.push((row.parse().unwrap(), column.parse().unwrap()));
            }
            rest = &rest[end..];
        }
        found
    }

    fn assert_same_screen(actual: &Parser, expected: &Parser, frame: &ComposeFrame<'_>) {
        let (actual, expected) = (actual.screen(), expected.screen());
        assert_eq!(actual.contents(), expected.contents());
        assert_eq!(actual.hide_cursor(), expected.hide_cursor());
        if !expected.hide_cursor() {
            assert_eq!(actual.cursor_position(), expected.cursor_position());
        }
        for row in 0..frame.host_rows {
            for column in 0..frame.host_columns {
                let (left, right) = (
                    actual.cell(row, column).unwrap(),
                    expected.cell(row, column).unwrap(),
                );
                assert_eq!(left.contents(), right.contents(), "cell {row},{column}");
                assert_eq!(left.is_wide(), right.is_wide());
                assert_eq!(left.fgcolor(), right.fgcolor());
                assert_eq!(left.bgcolor(), right.bgcolor());
                assert_eq!(left.bold(), right.bold());
                assert_eq!(left.dim(), right.dim());
                assert_eq!(left.inverse(), right.inverse());
            }
        }
    }

    /// Replay `history` (each committed) then `target`, and compare with a
    /// fresh full render of `target`.
    fn assert_incremental_matches_full(frames: &[ComposeFrame<'_>]) {
        let mut compositor = Compositor::try_new().unwrap();
        let last = frames.last().unwrap();
        let mut parser = Parser::new(last.host_rows, last.host_columns, 0);
        for frame in frames {
            parser.process(&rendered_committed(&mut compositor, frame));
        }
        assert_same_screen(&parser, &screen(&fresh_render(last), last), last);
    }

    #[derive(Default)]
    struct CountingSink(usize);

    impl Write for CountingSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn left_right_panes_render_at_translated_positions_with_divider() {
        let left = snapshot_from("LEFT", 10, 20);
        let right = snapshot_from("RIGHT\r\nrow2", 10, 20);
        let frame = side_by_side(&left, &right);
        let output = fresh_render(&frame);

        assert!(output.starts_with(b"\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J"));
        assert!(contains(&output, b"\x1b[1;22H\x1b[0;39;49mR"));
        assert!(contains(&output, b"\x1b[2;22H\x1b[0;39;49mr"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 0, 0), "L");
        assert_eq!(cell_text(&parser, 0, 3), "T");
        assert_eq!(cell_text(&parser, 0, 21), "R");
        assert_eq!(cell_text(&parser, 1, 21), "r");
        for row in 0..10 {
            assert_eq!(cell_text(&parser, row, 20), "|");
        }
        assert!(positions(&output)
            .iter()
            .all(|&(row, column)| row <= 10 && column <= 41));
    }

    #[test]
    fn top_bottom_panes_render_at_translated_rows_with_horizontal_divider() {
        let top = snapshot_from("TOP", 5, 20);
        let bottom = snapshot_from("BOTTOM", 5, 20);
        let frame = ComposeFrame {
            host_rows: 11,
            host_columns: 20,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 5, 20),
                    content: PaneContent::Snapshot(&top),
                }),
                Some(PaneView {
                    rect: rect(6, 0, 5, 20),
                    content: PaneContent::Snapshot(&bottom),
                }),
                None,
                None,
            ],
            divider: Some(rect(5, 0, 1, 20)),
            dividers: [Some(rect(5, 0, 1, 20)), None, None],
            focused: None,
            overlay: None,
        };
        let output = fresh_render(&frame);

        assert!(contains(&output, b"\x1b[7;1H\x1b[0;39;49mB"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 0, 0), "T");
        assert_eq!(cell_text(&parser, 6, 0), "B");
        assert_eq!(cell_text(&parser, 6, 5), "M");
        for column in 0..20 {
            assert_eq!(cell_text(&parser, 5, column), "-");
        }
    }

    #[test]
    fn three_and_four_pane_layouts_render_every_divider_and_placeholder() {
        let panes = [
            snapshot_from("P1", 10, 20),
            snapshot_from("P2", 10, 20),
            snapshot_from("P3", 10, 20),
            snapshot_from("P4", 10, 20),
        ];
        let frame = ComposeFrame {
            host_rows: 21,
            host_columns: 41,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 10, 20),
                    content: PaneContent::Snapshot(&panes[0]),
                }),
                Some(PaneView {
                    rect: rect(0, 21, 10, 20),
                    content: PaneContent::Snapshot(&panes[1]),
                }),
                Some(PaneView {
                    rect: rect(11, 0, 10, 20),
                    content: PaneContent::Snapshot(&panes[2]),
                }),
                Some(PaneView {
                    rect: rect(11, 21, 10, 20),
                    content: PaneContent::Snapshot(&panes[3]),
                }),
            ],
            divider: Some(rect(0, 20, 21, 1)),
            dividers: [
                Some(rect(0, 20, 21, 1)),
                Some(rect(10, 0, 1, 20)),
                Some(rect(10, 21, 1, 20)),
            ],
            focused: None,
            overlay: None,
        };
        let output = fresh_render(&frame);
        let rendered = screen(&output, &frame);
        assert_eq!(cell_text(&rendered, 0, 0), "P");
        assert_eq!(cell_text(&rendered, 0, 21), "P");
        assert_eq!(cell_text(&rendered, 11, 0), "P");
        assert_eq!(cell_text(&rendered, 11, 21), "P");
        for row in 0..21 {
            assert_eq!(cell_text(&rendered, row, 20), "|");
        }
        for column in 0..20 {
            assert_eq!(cell_text(&rendered, 10, column), "-");
            assert_eq!(cell_text(&rendered, 10, column + 21), "-");
        }
    }

    #[test]
    fn one_changed_cell_in_second_pane_emits_only_that_cell() {
        let left = snapshot_from("LEFT", 10, 20);
        let before = snapshot_from("abc", 10, 20);
        let after = snapshot_from("aXc", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &side_by_side(&left, &before));

        let output = rendered(&mut compositor, &side_by_side(&left, &after));
        assert!(!contains(&output, b"\x1b[2J"));
        assert_eq!(positions(&output), vec![(1, 23)]);
        assert!(contains(&output, b"\x1b[1;23H\x1b[0;39;49mX"));
        assert!(!contains(&output, b"LEFT"));
        assert_incremental_matches_full(&[
            side_by_side(&left, &before),
            side_by_side(&left, &after),
        ]);
    }

    #[test]
    fn identical_frame_emits_nothing_and_cursor_only_change_moves_cursor() {
        let left = snapshot_from("same", 10, 20);
        let right = snapshot_from("same", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        let mut frame = side_by_side(&left, &right);
        frame.focused = Some(1);
        rendered_committed(&mut compositor, &frame);
        assert!(rendered_committed(&mut compositor, &frame).is_empty());

        let moved = snapshot_from("same\x1b[3;4H", 10, 20);
        let mut frame = side_by_side(&left, &moved);
        frame.focused = Some(1);
        let output = rendered_committed(&mut compositor, &frame);
        assert_eq!(output, b"\x1b[3;25H");
    }

    #[test]
    fn rect_or_host_change_and_invalidate_force_full_repaint() {
        let left = snapshot_from("L", 10, 20);
        let right = snapshot_from("R", 10, 20);
        let narrow = snapshot_from("R", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &side_by_side(&left, &right));

        let mut moved = side_by_side(&left, &narrow);
        moved.host_columns = 42;
        moved.panes[1] = Some(PaneView {
            rect: rect(0, 22, 10, 20),
            content: PaneContent::Snapshot(&narrow),
        });
        moved.divider = Some(rect(0, 21, 10, 1));
        moved.dividers = [Some(rect(0, 21, 10, 1)), None, None];
        let output = rendered_committed(&mut compositor, &moved);
        assert!(output.starts_with(b"\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J"));

        let mut grown = moved;
        grown.host_columns = 43;
        assert!(contains(
            &rendered_committed(&mut compositor, &grown),
            b"\x1b[2J"
        ));
        assert!(rendered_committed(&mut compositor, &grown).is_empty());

        compositor.invalidate();
        assert!(contains(
            &rendered_committed(&mut compositor, &grown),
            b"\x1b[2J"
        ));
    }

    #[test]
    fn overlay_is_painted_last_with_styles_and_clipped_to_host() {
        let left = snapshot_from("LLLLLLLLLLLLLLLLLLLL", 10, 20);
        let right = snapshot_from("RRRRRRRRRRRRRRRRRRRR", 10, 20);
        let lines = [
            OverlayLine {
                text: "Split Left that is far too long",
                style: OverlayStyle::Selected,
            },
            OverlayLine {
                text: "Split Right",
                style: OverlayStyle::Disabled,
            },
            OverlayLine {
                text: "status \x1b]52;c;X\x07",
                style: OverlayStyle::Status,
            },
        ];
        let mut frame = side_by_side(&left, &right);
        frame.overlay = Some(OverlayView {
            rect: rect(0, 30, 4, 20),
            lines: &lines,
        });
        let output = fresh_render(&frame);

        assert!(positions(&output)
            .iter()
            .all(|&(row, column)| row <= 10 && column <= 41));
        assert!(contains(&output, b"\x1b[0;7m\x1b[1;31HSplit Left "));
        assert!(contains(&output, b"\x1b[0;2m\x1b[2;31HSplit Right"));
        assert!(!contains(&output, b"\x1b]52"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 0, 29), "R");
        assert_eq!(cell_text(&parser, 0, 30), "S");
        assert_eq!(cell_text(&parser, 0, 40), " ");
        assert!(parser.screen().cell(0, 30).unwrap().inverse());
        assert!(parser.screen().cell(1, 30).unwrap().dim());
        assert!(parser.screen().cell(2, 30).unwrap().bold());
        assert_eq!(cell_text(&parser, 2, 37), "?");
        assert_eq!(cell_text(&parser, 3, 30), " ");

        // Pane changes under an unchanged overlay never overwrite it.
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &frame);
        let changed = snapshot_from("xxxxxxxxxxxxxxxxxxxx", 10, 20);
        let mut next = side_by_side(&left, &changed);
        next.overlay = frame.overlay;
        let output = rendered_committed(&mut compositor, &next);
        assert!(positions(&output).iter().all(|&(_, column)| column <= 30));
        assert!(!contains(&output, b"Split"));
    }

    #[test]
    fn overlay_removal_or_move_repaints_covered_cells() {
        let left = snapshot_from("left pane text here", 10, 20);
        let right = snapshot_from("right pane text\r\nsecond row", 10, 20);
        let lines = [OverlayLine {
            text: "menu",
            style: OverlayStyle::Normal,
        }];
        let base = side_by_side(&left, &right);
        let mut with_overlay = base;
        with_overlay.overlay = Some(OverlayView {
            rect: rect(0, 15, 3, 12),
            lines: &lines,
        });
        let mut moved = base;
        moved.overlay = Some(OverlayView {
            rect: rect(5, 2, 2, 8),
            lines: &lines,
        });

        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &base);
        rendered_committed(&mut compositor, &with_overlay);
        let output = rendered_committed(&mut compositor, &base);
        assert!(!contains(&output, b"\x1b[2J"));
        assert!(positions(&output).iter().all(|&(row, _)| row <= 3));

        assert_incremental_matches_full(&[base, with_overlay, base]);
        assert_incremental_matches_full(&[base, with_overlay, moved]);
        assert_incremental_matches_full(&[with_overlay, moved, base]);
    }

    #[test]
    fn cursor_is_translated_for_focus_and_hidden_for_overlay_or_placeholder() {
        let left = snapshot_from("L", 10, 20);
        let right = snapshot_from("R\x1b[3;4H", 10, 20);
        let mut frame = side_by_side(&left, &right);
        frame.focused = Some(1);
        let output = fresh_render(&frame);
        assert!(output.ends_with(b"\x1b[3;25H\x1b[?25h"));
        let parser = screen(&output, &frame);
        assert_eq!(parser.screen().cursor_position(), (2, 24));
        assert!(!parser.screen().hide_cursor());

        let mut overlaid = frame;
        let lines: [OverlayLine<'_>; 0] = [];
        overlaid.overlay = Some(OverlayView {
            rect: rect(0, 0, 2, 5),
            lines: &lines,
        });
        let output = fresh_render(&overlaid);
        assert!(!contains(&output, b"\x1b[?25h"));
        assert!(screen(&output, &overlaid).screen().hide_cursor());

        let mut placeholder = frame;
        placeholder.panes[1] = Some(PaneView {
            rect: rect(0, 21, 10, 20),
            content: PaneContent::Placeholder("starting"),
        });
        assert!(!contains(&fresh_render(&placeholder), b"\x1b[?25h"));

        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &frame);
        assert_eq!(
            rendered_committed(&mut compositor, &overlaid)[..6],
            *b"\x1b[?25l"
        );
        let output = rendered_committed(&mut compositor, &frame);
        assert!(output.ends_with(b"\x1b[3;25H\x1b[?25h"));
    }

    #[test]
    fn placeholder_is_centered_sanitized_and_clipped() {
        let left = snapshot_from("L", 10, 20);
        let mut frame = side_by_side(&left, &left);
        frame.panes[1] = Some(PaneView {
            rect: rect(0, 21, 10, 20),
            content: PaneContent::Placeholder("starting"),
        });
        let parser = screen(&fresh_render(&frame), &frame);
        // Rows 10 -> text row 4; 20 - 8 columns -> offset 6 -> host column 27.
        assert_eq!(cell_text(&parser, 4, 27), "s");
        assert_eq!(cell_text(&parser, 4, 34), "g");
        assert_eq!(cell_text(&parser, 4, 26), " ");

        frame.panes[1] = Some(PaneView {
            rect: rect(0, 21, 10, 20),
            content: PaneContent::Placeholder("launch_failed: \x1b]52;c;Q\x07 é and much more"),
        });
        let output = fresh_render(&frame);
        assert!(!contains(&output, b"\x1b]52"));
        assert!(!contains(&output, "é".as_bytes()));
        assert!(positions(&output).iter().all(|&(_, column)| column <= 41));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 4, 21), "l");
        assert_eq!(cell_text(&parser, 4, 36), "?");
        assert_eq!(cell_text(&parser, 4, 40), ";");
        assert_eq!(cell_text(&parser, 4, 20), "|");

        // Changing the placeholder text repaints only that pane.
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &frame);
        frame.panes[1] = Some(PaneView {
            rect: rect(0, 21, 10, 20),
            content: PaneContent::Placeholder("resize_pending"),
        });
        let output = rendered_committed(&mut compositor, &frame);
        assert!(!contains(&output, b"\x1b[2J"));
        assert!(positions(&output).iter().all(|&(_, column)| column >= 22));
        assert!(rendered_committed(&mut compositor, &frame).is_empty());
    }

    #[test]
    fn invalid_geometry_is_rejected_before_any_byte() {
        let left = snapshot_from("L", 10, 20);
        let small = snapshot_from("S", 5, 20);
        let valid = side_by_side(&left, &left);
        let pane = |rect, snapshot| {
            Some(PaneView {
                rect,
                content: PaneContent::Snapshot(snapshot),
            })
        };

        let mut cases = Vec::new();
        let mut frame = valid;
        frame.host_columns = 40;
        cases.push(frame);
        let mut frame = valid;
        frame.panes[1] = pane(rect(0, 21, 10, 20), &small);
        cases.push(frame);
        let mut frame = valid;
        frame.panes[1] = pane(rect(0, 15, 10, 20), &left);
        frame.divider = None;
        cases.push(frame);
        let mut frame = valid;
        frame.divider = Some(rect(0, 19, 10, 1));
        frame.dividers = [frame.divider, None, None];
        cases.push(frame);
        let mut frame = valid;
        frame.divider = Some(rect(0, 20, 10, 2));
        frame.dividers = [frame.divider, None, None];
        cases.push(frame);
        let mut frame = valid;
        frame.panes[1] = Some(PaneView {
            rect: rect(0, 21, 0, 20),
            content: PaneContent::Placeholder("x"),
        });
        cases.push(frame);
        let mut frame = valid;
        frame.divider = Some(rect(0, 20, 10, 2));
        frame.dividers = [Some(rect(0, 20, 10, 2)), None, None];
        cases.push(frame);
        let mut frame = valid;
        frame.panes[1] = Some(PaneView {
            rect: rect(0, u16::MAX, 10, 20),
            content: PaneContent::Placeholder("x"),
        });
        cases.push(frame);

        let mut compositor = Compositor::try_new().unwrap();
        for (index, frame) in cases.into_iter().enumerate() {
            let mut sink = CountingSink::default();
            let error = match compositor.render(&frame, &mut sink) {
                Err(error) => error,
                Ok(()) => panic!("invalid case {index} accepted"),
            };
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(sink.0, 0);
            assert!(!compositor.commit());
        }
    }

    #[test]
    fn malicious_cell_content_is_never_emitted_raw() {
        let parsed = snapshot_from(
            "\x1b]0;TITLE-CANARY\x07\x1b]52;c;CLIP-CANARY\x07A\u{202e}B",
            10,
            20,
        );
        let mut synthetic = snapshot_from("", 10, 20);
        set_text(&mut synthetic.cells[0], "\x1b]52;SYNTH-CANARY");
        set_text(&mut synthetic.cells[1], "\u{0085}");
        let mut compositor = Compositor::try_new().unwrap();
        let first = rendered_committed(&mut compositor, &side_by_side(&parsed, &synthetic));
        let mut changed = snapshot_from("", 10, 20);
        set_text(&mut changed.cells[2], "\x1b[2J\x1b]52;c;DIFF-CANARY");
        let diff = rendered_committed(&mut compositor, &side_by_side(&changed, &synthetic));

        for output in [first, diff] {
            let text = String::from_utf8(output).unwrap();
            for canary in ["TITLE-CANARY", "CLIP-CANARY", "SYNTH-CANARY", "DIFF-CANARY"] {
                assert!(!text.contains(canary));
            }
            assert!(!text.contains("\x1b]"));
            assert!(!text.contains('\u{202e}'));
            assert!(!text.contains('\u{0085}'));
        }
    }

    #[test]
    fn uncommitted_frame_is_re_emitted_and_writer_failure_invalidates() {
        let left = snapshot_from("L", 10, 20);
        let before = snapshot_from("abc", 10, 20);
        let after = snapshot_from("aXc", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        assert!(!compositor.commit());
        rendered_committed(&mut compositor, &side_by_side(&left, &before));

        let first = rendered(&mut compositor, &side_by_side(&left, &after));
        let again = rendered(&mut compositor, &side_by_side(&left, &after));
        assert_eq!(first, again);
        assert!(!first.is_empty());
        assert!(compositor.commit());
        assert!(!compositor.commit());
        assert!(rendered(&mut compositor, &side_by_side(&left, &after)).is_empty());

        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                }
                let count = self.0.min(bytes.len());
                self.0 -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = compositor
            .render(&side_by_side(&left, &before), &mut FailAfter(4))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(!compositor.commit());
        let recovery = rendered(&mut compositor, &side_by_side(&left, &before));
        assert!(contains(&recovery, b"\x1b[2J"));
    }

    #[test]
    fn wide_cells_stay_inside_the_pane_and_outside_the_overlay() {
        let mut left = snapshot_from("", 10, 20);
        left.cells[19].wide = true;
        set_text(&mut left.cells[19], "界");
        let right = snapshot_from("界X", 10, 20);
        let frame = side_by_side(&left, &right);
        let output = fresh_render(&frame);
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 0, 19), "?");
        assert_eq!(cell_text(&parser, 0, 20), "|");
        assert_eq!(cell_text(&parser, 0, 21), "界");
        assert_eq!(cell_text(&parser, 0, 23), "X");

        // A wide glyph straddling the overlay's left edge keeps a fallback.
        let lines = [OverlayLine {
            text: "MENU",
            style: OverlayStyle::Normal,
        }];
        let mut overlaid = frame;
        overlaid.overlay = Some(OverlayView {
            rect: rect(0, 22, 1, 4),
            lines: &lines,
        });
        let output = fresh_render(&overlaid);
        assert!(!contains(&output, "界X".as_bytes()));
        let parser = screen(&output, &overlaid);
        assert_eq!(cell_text(&parser, 0, 21), "?");
        assert_eq!(cell_text(&parser, 0, 22), "M");
        assert_eq!(cell_text(&parser, 0, 25), "U");
        assert_incremental_matches_full(&[frame, overlaid, frame]);
    }

    #[test]
    fn incremental_sequences_match_full_renders() {
        let a = snapshot_from("\x1b[1;31mred\x1b[0m 界 e\u{301}\r\nline", 10, 20);
        let b = snapshot_from("\x1b[1;31mred\x1b[0m 語 o\u{301}\r\nLINE\x1b[?25l", 10, 20);
        let c = snapshot_from("\x1b[2J\x1b[5;5Hz", 10, 20);
        let lines = [OverlayLine {
            text: "overlay",
            style: OverlayStyle::Selected,
        }];
        let mut focused = side_by_side(&a, &b);
        focused.focused = Some(0);
        let mut overlaid = side_by_side(&b, &c);
        overlaid.overlay = Some(OverlayView {
            rect: rect(3, 18, 3, 10),
            lines: &lines,
        });
        let mut placeholder = side_by_side(&c, &a);
        placeholder.panes[0] = Some(PaneView {
            rect: rect(0, 0, 10, 20),
            content: PaneContent::Placeholder("starting"),
        });
        assert_incremental_matches_full(&[
            side_by_side(&a, &b),
            focused,
            overlaid,
            side_by_side(&c, &a),
            placeholder,
            side_by_side(&a, &c),
        ]);
    }

    #[test]
    fn maximum_frame_streams_under_four_mib_without_a_frame_buffer() {
        let mut first = snapshot_from("", 80, 119);
        let maximum_cell_text = format!("A{}", "\u{20d0}".repeat(7));
        for cell in &mut first.cells {
            set_text(cell, &maximum_cell_text);
            cell.bold = true;
            cell.inverse = true;
            cell.foreground = crate::terminal::Color::Rgb(255, 255, 255);
            cell.background = crate::terminal::Color::Rgb(255, 255, 255);
        }
        let mut second = snapshot_from("", 80, 120);
        second.copy_from(&first);
        second.columns = 120;
        second.cells.resize(80 * 120, first.cells[0]);
        let frame = ComposeFrame {
            host_rows: 80,
            host_columns: 240,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 80, 119),
                    content: PaneContent::Snapshot(&first),
                }),
                Some(PaneView {
                    rect: rect(0, 120, 80, 120),
                    content: PaneContent::Snapshot(&second),
                }),
                None,
                None,
            ],
            divider: Some(rect(0, 119, 80, 1)),
            dividers: [Some(rect(0, 119, 80, 1)), None, None],
            focused: Some(0),
            overlay: None,
        };
        let mut compositor = Compositor::try_new().unwrap();
        let mut sink = CountingSink::default();
        compositor.render(&frame, &mut sink).unwrap();
        assert!(sink.0 > 0);
        assert!(sink.0 <= 4 * 1024 * 1024);
    }
}
