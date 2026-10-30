//! Pure, bounded composition of up to four pane snapshots and dividers
//! and an optional ASCII overlay into one host terminal frame.
//!
//! The compositor writes ANSI straight to a caller-supplied writer and keeps
//! only fixed, preallocated baselines. It performs no host IO of its own and
//! contains no menu logic; callers describe each frame with [`ComposeFrame`].

use std::io::{self, Write};

use super::controls::{layout_filled, style, ControlRole, ControlState};
use super::render::{paint_row, paint_row_diff, validate_snapshot, Placement};
use super::{io_error, ScreenSnapshot, SnapshotCell, TerminalError, TerminalSize};
use crate::pane_layout::{content_rect, Rect};
use crate::terminal_selection::Point;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Largest pane or overlay height accepted by the compositor.
pub const MAX_RECT_ROWS: u16 = 80;
/// Largest pane or overlay width accepted by the compositor.
pub const MAX_RECT_COLUMNS: u16 = 240;

const MAX_ROWS: usize = MAX_RECT_ROWS as usize;
const MAX_COLUMNS: usize = MAX_RECT_COLUMNS as usize;
const MAX_OVERLAY_CELLS: usize = MAX_ROWS * MAX_COLUMNS;
const MAX_PANE_LABEL_BYTES: usize = MAX_COLUMNS * 4;
const VERSION_LABEL: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const PANE_LABELS: [&str; crate::pane_layout::MAX_PANES] = ["Pane 1", "Pane 2", "Pane 3", "Pane 4"];

/// One sanitized cell in the persistent, fixed top row.
///
/// Glyph text is kept inline so the compositor can retain committed and
/// pending headers without allocating or borrowing caller-owned labels.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct HeaderCell {
    text: [u8; 32],
    len: u8,
    width: u8,
    selected: bool,
    pressed: bool,
    filled: bool,
    pending: bool,
    continuation: bool,
}

impl std::fmt::Debug for HeaderCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderCell")
            .field("len", &self.len)
            .field("width", &self.width)
            .field("selected", &self.selected)
            .field("continuation", &self.continuation)
            .finish()
    }
}

impl HeaderCell {
    /// Apply local pressed presentation without changing semantic strip geometry.
    pub fn with_pressed(mut self) -> Self {
        self.pressed = true;
        self
    }

    pub fn with_pending(mut self) -> Self {
        self.pending = true;
        self
    }

    /// Fill a semantic strip control, including its padding.
    pub fn with_filled(mut self) -> Self {
        self.filled = true;
        self
    }

    /// Construct a sanitized single- or double-width UTF-8 glyph.
    pub fn glyph(text: &str, selected: bool) -> Option<Self> {
        if text.is_empty() || text.len() > 32 || text.chars().any(char::is_control) {
            return None;
        }
        if text
            .chars()
            .next()
            .is_some_and(|character| UnicodeWidthChar::width(character).unwrap_or(0) == 0)
        {
            return None;
        }
        let width = UnicodeWidthStr::width(text);
        if !(1..=2).contains(&width) {
            return None;
        }
        let mut bytes = [0; 32];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self {
            text: bytes,
            len: text.len() as u8,
            width: width as u8,
            selected,
            pressed: false,
            filled: false,
            pending: false,
            continuation: false,
        })
    }

    /// Construct a visible blank cell.
    pub const fn blank(selected: bool) -> Self {
        Self {
            text: [0; 32],
            len: 0,
            width: 1,
            selected,
            pressed: false,
            filled: false,
            pending: false,
            continuation: false,
        }
    }

    /// Construct the second cell occupied by a wide glyph.
    pub const fn continuation(selected: bool) -> Self {
        Self {
            text: [0; 32],
            len: 0,
            width: 0,
            selected,
            pressed: false,
            filled: false,
            pending: false,
            continuation: true,
        }
    }

    pub fn text(&self) -> &str {
        // Construction is private and glyph() accepts valid UTF-8 only.
        std::str::from_utf8(&self.text[..usize::from(self.len)])
            .expect("validated UTF-8 header cell")
    }

    pub const fn is_wide(&self) -> bool {
        !self.continuation && self.width == 2
    }
    pub const fn is_continuation(&self) -> bool {
        self.continuation
    }
    pub const fn selected(&self) -> bool {
        self.selected
    }
}

/// Content shown inside one pane rectangle.
#[derive(Clone, Copy, Debug)]
pub enum PaneContent<'a> {
    /// A sanitized terminal snapshot whose dimensions equal the inset content rectangle.
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

/// History position used to draw a pane's right-margin scrollbar.
///
/// `total_rows` counts available history rows and excludes the live screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scrollbar {
    pub offset_rows: u16,
    pub total_rows: u16,
}

/// Per-pane chrome rendered outside PTY content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneChrome<'a> {
    /// Optional normal-history scrollbar for each stable pane slot.
    pub scrollbars: [Option<Scrollbar>; crate::pane_layout::MAX_PANES],
    /// Optional normal-history state and Back to live control for each slot.
    pub history: [Option<HistoryLabelKind>; crate::pane_layout::MAX_PANES],
    /// Captured header control: false for Menu, true for close.
    pub header_pressed: [Option<bool>; crate::pane_layout::MAX_PANES],
    pub history_pressed: [bool; crate::pane_layout::MAX_PANES],
    /// Optional concise pane labels. Missing labels use safe `Pane N` labels.
    pub labels: [Option<&'a str>; crate::pane_layout::MAX_PANES],
}

impl PaneChrome<'_> {
    /// No supplemental pane chrome.
    pub const fn empty() -> Self {
        Self {
            scrollbars: [None; crate::pane_layout::MAX_PANES],
            history: [None; crate::pane_layout::MAX_PANES],
            header_pressed: [None; crate::pane_layout::MAX_PANES],
            history_pressed: [false; crate::pane_layout::MAX_PANES],
            labels: [None; crate::pane_layout::MAX_PANES],
        }
    }
}

/// Hitboxes for the always-visible controls in a padded pane's top margin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneControls {
    /// Six-cell ` Menu ` hitbox.
    pub menu: Rect,
    /// Three-cell ` x ` hitbox.
    pub close: Rect,
    /// One-cell focus marker, immediately after the Menu control.
    pub focus_marker: Rect,
    /// Label area between the focus marker and close control.
    pub label: Rect,
}

/// Returns the menu and close hitboxes for a valid padded pane rectangle.
///
/// The controls occupy only the top padding row and do not change content
/// geometry. Unpadded recovery panes should not expose controls.
pub fn pane_controls(rect: Rect) -> Option<PaneControls> {
    content_rect(rect)?;
    let menu = Rect {
        x: rect.x.checked_add(1)?,
        y: rect.y,
        rows: 1,
        columns: 6,
    };
    let close = Rect {
        x: rect.x.checked_add(rect.columns.checked_sub(4)?)?,
        y: rect.y,
        rows: 1,
        columns: 3,
    };
    // One blank cell separates [Menu] from the focus marker, and one more
    // separates the marker from the label.
    let marker_x = menu.x.checked_add(menu.columns)?.checked_add(1)?;
    let label_x = marker_x.checked_add(2)?;
    Some(PaneControls {
        menu,
        close,
        focus_marker: Rect {
            x: marker_x,
            y: rect.y,
            rows: 1,
            columns: 1,
        },
        label: Rect {
            x: label_x,
            y: rect.y,
            rows: 1,
            // Keep one blank cell between the label and the close control.
            columns: close.x.checked_sub(label_x)?.saturating_sub(1),
        },
    })
}

/// Returns the index and exact rectangle of the divider containing a pointer.
/// Divider rectangles are the same half-open rectangles rendered by the compositor.
pub fn divider_at(
    dividers: [Option<Rect>; crate::pane_layout::MAX_PANES - 1],
    x: u16,
    y: u16,
) -> Option<(usize, Rect)> {
    dividers.into_iter().enumerate().find_map(|(index, rect)| {
        rect.filter(|rect| rect.contains(x, y))
            .map(|rect| (index, rect))
    })
}

/// Map a host pointer position to a clamped cell in one pane's content.
///
/// The content rectangle must already include any host-row offset, such as the
/// optional workspace tab strip. Pointer hits in pane padding clamp to the
/// nearest content edge; callers decide whether padding is a valid gesture area.
pub fn pointer_content_point(content: Rect, x: u16, y: u16) -> Point {
    Point {
        row: y
            .saturating_sub(content.y)
            .min(content.rows.saturating_sub(1)),
        column: x
            .saturating_sub(content.x)
            .min(content.columns.saturating_sub(1)),
    }
}

/// Short history state shown in a pane's bottom padding row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryLabelKind {
    History,
    NewOutput,
    Truncated,
}

impl HistoryLabelKind {
    const fn text(self) -> &'static [u8] {
        match self {
            Self::History => b"History",
            Self::NewOutput => b"New output",
            Self::Truncated => b"History truncated",
        }
    }
}

/// Shared hit/render geometry for the history label and return-to-live button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryControls {
    /// Clipped history-state label area; zero width means the label is omitted.
    pub label: Rect,
    /// Clickable return-to-live button.
    pub back: Rect,
    /// Button wording selected for the available width.
    pub back_text: &'static [u8],
}

/// The client version badge's cells, when this pane owns the bottom-right
/// viewport corner. Keeping this geometry shared prevents history controls
/// from painting or accepting clicks over the badge.
pub fn pane_version_badge(rect: Rect, host_rows: u16, host_columns: u16) -> Option<Rect> {
    if rect.x.checked_add(rect.columns)? != host_columns
        || rect.y.checked_add(rect.rows)? != host_rows
        || VERSION_LABEL.len() >= usize::from(rect.columns)
    {
        return None;
    }
    Some(Rect {
        x: rect
            .x
            .checked_add(rect.columns.checked_sub(1 + VERSION_LABEL.len() as u16)?)?,
        y: rect.y.checked_add(rect.rows.checked_sub(1)?)?,
        rows: 1,
        columns: VERSION_LABEL.len() as u16,
    })
}

/// Compute one padded pane's bottom-row history-control geometry. The button
/// remains available at the minimum pane width; the state label is clipped or
/// omitted before the button is reduced to its compact `[Live]` wording.
pub fn pane_history_controls(
    rect: Rect,
    kind: HistoryLabelKind,
    version_badge: Option<Rect>,
) -> Option<HistoryControls> {
    content_rect(rect)?;
    let row = rect.y.checked_add(rect.rows.checked_sub(1)?)?;
    let start = rect.x.checked_add(1)?;
    let right = rect.x.checked_add(rect.columns.checked_sub(1)?)?;
    let end = version_badge
        .filter(|badge| badge.y == row)
        .map_or(right, |badge| badge.x.saturating_sub(1).min(right));
    let compact = b"[Live]".as_slice();
    let expanded = b"[Back to live]".as_slice();
    let expanded_width = u16::try_from(expanded.len()).ok()?;
    let compact_width = u16::try_from(compact.len()).ok()?;
    let width = end.saturating_sub(start);
    let (back_text, back_width) = if width >= expanded_width {
        (expanded, expanded_width)
    } else if width >= compact_width {
        (compact, compact_width)
    } else {
        return None;
    };
    let back = Rect {
        x: end.checked_sub(back_width)?,
        y: row,
        rows: 1,
        columns: back_width,
    };
    Some(HistoryControls {
        label: Rect {
            x: start,
            y: row,
            rows: 1,
            columns: back
                .x
                .saturating_sub(start)
                .min(u16::try_from(kind.text().len()).ok()?),
        },
        back,
        back_text,
    })
}

/// Shared track and thumb geometry for rendering and pointer hit testing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollbarGeometry {
    /// One-cell-wide track in a pane's right padding.
    pub track: Rect,
    /// Current thumb within `track`.
    pub thumb: Rect,
}

/// Returns validated scrollbar geometry for a padded pane rectangle.
pub fn scrollbar_geometry(rect: Rect, scrollbar: Scrollbar) -> Option<ScrollbarGeometry> {
    content_rect(rect)?;
    if scrollbar.offset_rows > scrollbar.total_rows {
        return None;
    }
    let track = Rect {
        x: rect.x.checked_add(rect.columns.checked_sub(1)?)?,
        y: rect.y.checked_add(1)?,
        rows: rect.rows.checked_sub(2)?,
        columns: 1,
    };
    let thumb_rows = scrollbar_thumb(track.rows, scrollbar);
    let thumb_top = scrollbar_thumb_top(track.rows, thumb_rows, scrollbar);
    Some(ScrollbarGeometry {
        track,
        thumb: Rect {
            x: track.x,
            y: track.y.checked_add(thumb_top)?,
            rows: thumb_rows,
            columns: 1,
        },
    })
}

/// Frozen selected cells to paint over one live pane snapshot.
///
/// The source is borrowed and never modified. The compositor copies selected
/// cell presentation into its own pending frame, so live updates outside the
/// range continue to render and a later render without this overlay restores
/// the current live view.
#[derive(Clone, Copy, Debug)]
pub struct SelectionOverlay<'a> {
    /// Stable slot in [`ComposeFrame::panes`].
    pub slot: usize,
    /// Immutable snapshot captured when selection began.
    pub frozen: &'a ScreenSnapshot,
    /// First endpoint in zero-based pane-content coordinates.
    pub anchor: Point,
    /// Current endpoint in zero-based pane-content coordinates.
    pub end: Point,
}

/// One local keyboard caret painted over a frozen pane snapshot.
#[derive(Clone, Copy, Debug)]
pub struct CopyCaretOverlay<'a> {
    /// Stable slot in [`ComposeFrame::panes`].
    pub slot: usize,
    /// Immutable snapshot captured when copy mode began.
    pub frozen: &'a ScreenSnapshot,
    /// Caret in zero-based pane-content coordinates.
    pub point: Point,
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

/// Presentation of one actionable overlay rectangle, independent of row text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayControl {
    pub bounds: Rect,
    pub role: ControlRole,
    pub state: ControlState,
}
const MAX_OVERLAY_CONTROLS: usize = MAX_ROWS * 2;

/// An opaque overlay painted above every pane and the divider.
#[derive(Clone, Copy, Debug)]
pub struct OverlayView<'a> {
    /// Absolute screen-space actionable control paint spans.
    pub controls: &'a [OverlayControl],
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
    scrollbar: Option<Scrollbar>,
    history: Option<HistoryLabelKind>,
    placeholder: [u8; MAX_COLUMNS],
    placeholder_len: u16,
    label: [u8; MAX_PANE_LABEL_BYTES],
    label_len: u16,
    label_truncated: bool,
    header_pressed: Option<bool>,
    history_pressed: bool,
}

impl SlotMeta {
    const EMPTY: Self = Self {
        rect: None,
        kind: SlotKind::Empty,
        scrollbar: None,
        history: None,
        placeholder: [0; MAX_COLUMNS],
        placeholder_len: 0,
        label: [0; MAX_PANE_LABEL_BYTES],
        label_len: 0,
        label_truncated: false,
        header_pressed: None,
        history_pressed: false,
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
    header_present: bool,
    focused: Option<usize>,
    overlay_styles: [OverlayStyle; MAX_ROWS],
    overlay_controls: [Option<OverlayControl>; MAX_OVERLAY_CONTROLS],
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
        header_present: false,
        focused: None,
        overlay_styles: [OverlayStyle::Normal; MAX_ROWS],
        overlay_controls: [None; MAX_OVERLAY_CONTROLS],
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
    displayed_header: [HeaderCell; MAX_COLUMNS],
    target_header: [HeaderCell; MAX_COLUMNS],
    displayed_meta: FrameMeta,
    pending_meta: FrameMeta,
    valid: bool,
    pending_ready: bool,
    padded: bool,
    reduced_control_color: bool,
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
        Self::with_padding(true)
    }

    /// Preserve saved checkpoint geometry without adding live-pane margins.
    /// Recovery snapshots retain the dimensions recorded by their source.
    pub fn try_new_unpadded() -> Result<Self, TerminalError> {
        Self::with_padding(false)
    }

    fn with_padding(padded: bool) -> Result<Self, TerminalError> {
        Ok(Self {
            displayed: std::array::from_fn(|_| None),
            target: std::array::from_fn(|_| None),
            displayed_overlay: overlay_buffer()?,
            target_overlay: overlay_buffer()?,
            displayed_header: [HeaderCell::blank(false); MAX_COLUMNS],
            target_header: [HeaderCell::blank(false); MAX_COLUMNS],
            displayed_meta: FrameMeta::EMPTY,
            pending_meta: FrameMeta::EMPTY,
            valid: false,
            pending_ready: false,
            padded,
            reduced_control_color: false,
        })
    }

    /// Select reduced-color chrome without consulting the process environment.
    /// A change discards staged output and repaints against a fresh baseline.
    pub fn set_reduced_control_color(&mut self, reduced: bool) {
        if self.reduced_control_color != reduced {
            self.reduced_control_color = reduced;
            self.invalidate();
        }
    }

    pub fn reduced_control_color(&self) -> bool {
        self.reduced_control_color
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
        std::mem::swap(&mut self.displayed_header, &mut self.target_header);
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
        self.render_with_selection(frame, None, output)
    }

    /// Render a frame with an optional persistent header occupying host row 0.
    /// The header must contain exactly one cell per host column. Existing
    /// overlays remain independent and are validated to stay below the row.
    pub fn render_with_header<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        header: Option<&[HeaderCell]>,
        output: &mut W,
    ) -> io::Result<()> {
        self.render_inner(frame, selection, None, header, PaneChrome::empty(), output)
    }

    /// Validate and render a frame with an optional frozen-cell selection.
    ///
    /// The selected pane must still contain a snapshot with the frozen
    /// snapshot's dimensions and buffer. Invalid selection geometry is
    /// rejected before staging or writing any bytes. A menu overlay in the
    /// frame keeps priority and suppresses the selection presentation.
    pub fn render_with_selection<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        output: &mut W,
    ) -> io::Result<()> {
        self.render_inner(frame, selection, None, None, PaneChrome::empty(), output)
    }

    /// Render frozen selection and a local copy caret, optionally below a
    /// persistent header. Existing rendering entry points remain unchanged.
    pub fn render_with_copy_mode<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: CopyCaretOverlay<'_>,
        header: Option<&[HeaderCell]>,
        output: &mut W,
    ) -> io::Result<()> {
        self.render_inner(
            frame,
            selection,
            Some(caret),
            header,
            PaneChrome::empty(),
            output,
        )
    }

    /// Render overlays, a persistent header, and per-pane vertical scrollbars.
    /// Scrollbars occupy only the right cell of padded snapshot panes; they
    /// never change snapshot or selection geometry.
    pub fn render_with_chrome<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: Option<CopyCaretOverlay<'_>>,
        header: Option<&[HeaderCell]>,
        scrollbars: [Option<Scrollbar>; crate::pane_layout::MAX_PANES],
        output: &mut W,
    ) -> io::Result<()> {
        self.render_with_history_controls(
            frame,
            selection,
            caret,
            header,
            PaneChrome {
                scrollbars,
                history: [None; crate::pane_layout::MAX_PANES],
                header_pressed: [None; crate::pane_layout::MAX_PANES],
                history_pressed: [false; crate::pane_layout::MAX_PANES],
                labels: [None; crate::pane_layout::MAX_PANES],
            },
            output,
        )
    }

    /// Render pane chrome including history labels and clickable Back to live
    /// controls. Geometry for both presentation and hit-testing comes from
    /// [`pane_history_controls`].
    pub fn render_with_history_controls<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: Option<CopyCaretOverlay<'_>>,
        header: Option<&[HeaderCell]>,
        chrome: PaneChrome<'_>,
        output: &mut W,
    ) -> io::Result<()> {
        self.render_inner(frame, selection, caret, header, chrome, output)
    }

    fn render_inner<W: Write>(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: Option<CopyCaretOverlay<'_>>,
        header: Option<&[HeaderCell]>,
        chrome: PaneChrome<'_>,
        output: &mut W,
    ) -> io::Result<()> {
        self.pending_ready = false;
        validate_frame(frame, self.padded, header.is_some())?;
        if let Some(header) = header {
            validate_header(header, frame.host_columns)?;
        }
        validate_scrollbars(frame, self.padded, chrome.scrollbars)?;
        let selection = if frame.overlay.is_some() {
            None
        } else {
            selection
        };
        let caret = if frame.overlay.is_some() { None } else { caret };
        if let Some(selection) = selection {
            validate_selection(frame, selection)?;
        }
        if let Some(caret) = caret {
            validate_copy_caret(frame, caret)?;
        }
        let selection = selection.filter(|selection| selection.anchor != selection.end);
        self.stage(frame, selection, caret, header, chrome);
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

    fn stage(
        &mut self,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: Option<CopyCaretOverlay<'_>>,
        header: Option<&[HeaderCell]>,
        chrome: PaneChrome<'_>,
    ) {
        let mut meta = FrameMeta::EMPTY;
        meta.host_rows = frame.host_rows;
        meta.host_columns = frame.host_columns;
        meta.focused = frame.focused;
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
            slot_meta.scrollbar = chrome.scrollbars[slot];
            slot_meta.history = chrome.history[slot];
            slot_meta.header_pressed = chrome.header_pressed[slot];
            slot_meta.history_pressed = chrome.history_pressed[slot];
            let label = chrome.labels[slot].unwrap_or(PANE_LABELS[slot]);
            store_pane_label(slot_meta, label);
            match pane.content {
                PaneContent::Snapshot(snapshot) => {
                    slot_meta.kind = SlotKind::Snapshot;
                    let content = pane_content_rect(pane.rect, self.padded)
                        .expect("validated pane dimensions");
                    match self.target[slot].as_mut() {
                        Some(target) => target.copy_from(snapshot),
                        None => {
                            let size = TerminalSize::new(content.rows, content.columns)
                                .expect("validated pane dimensions");
                            let mut target = ScreenSnapshot::try_new(size)
                                .expect("bounded compositor pane allocation");
                            target.copy_from(snapshot);
                            self.target[slot] = Some(target);
                        }
                    }
                    if let Some(selection) = selection.filter(|selection| selection.slot == slot) {
                        let target = self.target[slot]
                            .as_mut()
                            .expect("snapshot target was allocated or reused");
                        paint_selection(target, selection);
                    }
                    if let Some(caret) = caret.filter(|caret| caret.slot == slot) {
                        let target = self.target[slot]
                            .as_mut()
                            .expect("snapshot target was allocated or reused");
                        paint_copy_caret(target, caret.point);
                    }
                }
                PaneContent::Placeholder(text) => {
                    slot_meta.kind = SlotKind::Placeholder;
                    let content = pane_content_rect(pane.rect, self.padded)
                        .expect("validated pane dimensions");
                    let limit = usize::from(content.columns).min(MAX_COLUMNS);
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
            for (slot, control) in meta.overlay_controls.iter_mut().zip(overlay.controls) {
                *slot = Some(*control);
            }
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
        if let Some(header) = header {
            meta.header_present = true;
            self.target_header[..header.len()].copy_from_slice(header);
        } else {
            self.target_header[..usize::from(frame.host_columns)].fill(HeaderCell::blank(false));
        }

        meta.cursor = match (frame.overlay, selection, caret, frame.focused) {
            (None, None, None, Some(slot)) => match frame.panes.get(slot).copied().flatten() {
                Some(PaneView {
                    rect,
                    content: PaneContent::Snapshot(snapshot),
                }) if !snapshot.cursor_hidden() => {
                    let content =
                        pane_content_rect(rect, self.padded).expect("validated pane dimensions");
                    CursorState::Visible {
                        row: content.y + snapshot.cursor_row().min(snapshot.rows() - 1),
                        column: content.x + snapshot.cursor_column().min(snapshot.columns() - 1),
                    }
                }
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
            || pending.header_present != displayed.header_present
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
        pending.overlay_controls == displayed.overlay_controls
            && pending.overlay_styles[..rows] == displayed.overlay_styles[..rows]
            && self.target_overlay[..cells] == self.displayed_overlay[..cells]
    }

    fn header_unchanged(&self) -> bool {
        let columns = usize::from(self.pending_meta.host_columns);
        self.pending_meta.header_present == self.displayed_meta.header_present
            && self.target_header[..columns] == self.displayed_header[..columns]
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
        let scrollbar_changed: [bool; crate::pane_layout::MAX_PANES] =
            std::array::from_fn(|slot| {
                pending.slots[slot].scrollbar != displayed.slots[slot].scrollbar
            });
        let history_changed: [bool; crate::pane_layout::MAX_PANES] = std::array::from_fn(|slot| {
            pending.slots[slot].history != displayed.slots[slot].history
                || pending.slots[slot].history_pressed != displayed.slots[slot].history_pressed
        });
        let label_changed: [bool; crate::pane_layout::MAX_PANES] = std::array::from_fn(|slot| {
            pending.slots[slot].header_pressed != displayed.slots[slot].header_pressed
                || !same_pane_label(&pending.slots[slot], &displayed.slots[slot])
        });
        let focus_changed = pending.focused != displayed.focused;
        let controls_repaint: [bool; crate::pane_layout::MAX_PANES] = std::array::from_fn(|slot| {
            work[slot] == PaneWork::Full
                || label_changed[slot]
                || (focus_changed
                    && (pending.focused == Some(slot) || displayed.focused == Some(slot)))
        });

        let header_changed = !self.header_unchanged();
        if !overlay_changed
            && !header_changed
            && !focus_changed
            && work.iter().all(|work| *work == PaneWork::None)
            && !scrollbar_changed.iter().any(|changed| *changed)
            && !history_changed.iter().any(|changed| *changed)
            && !label_changed.iter().any(|changed| *changed)
        {
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
        if focus_changed || overlay_moved {
            for slot in 0..crate::pane_layout::MAX_PANES {
                let focused = pending.focused == Some(slot);
                if focused || (focus_changed && displayed.focused == Some(slot)) {
                    self.paint_focus_border(output, slot, focused)?;
                }
            }
        }
        for (slot, changed) in scrollbar_changed.into_iter().enumerate() {
            if changed || overlay_moved {
                self.paint_scrollbar(
                    output,
                    slot,
                    changed,
                    if overlay_moved { damage } else { [None, None] },
                )?;
            }
        }
        for (slot, repaint_all) in controls_repaint.into_iter().enumerate() {
            if repaint_all || overlay_moved || history_changed[slot] {
                self.paint_pane_controls(
                    output,
                    slot,
                    repaint_all,
                    history_changed[slot],
                    if overlay_moved { damage } else { [None, None] },
                )?;
            }
        }
        if overlay_changed {
            self.paint_overlay(output)?;
        }
        if header_changed {
            self.paint_header(output, false)?;
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
        for slot in 0..crate::pane_layout::MAX_PANES {
            if self.pending_meta.focused == Some(slot) {
                self.paint_focus_border(output, slot, true)?;
            }
        }
        for slot in 0..crate::pane_layout::MAX_PANES {
            self.paint_scrollbar(output, slot, true, [None, None])?;
        }
        for slot in 0..crate::pane_layout::MAX_PANES {
            self.paint_pane_controls(output, slot, true, true, [None, None])?;
        }
        self.paint_overlay(output)?;
        if self.pending_meta.header_present {
            self.paint_header(output, true)?;
        }
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
                let content =
                    pane_content_rect(rect, self.padded).expect("validated pane dimensions");
                if self.padded && damage.iter().any(Option::is_some) {
                    self.paint_padding(output, rect, exclude, damage)?;
                }
                let placement = Placement {
                    row: content.y,
                    column: content.x,
                    exclude,
                };
                for row in 0..snapshot.rows() {
                    let host_row = content.y + row;
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
                let content =
                    pane_content_rect(rect, self.padded).expect("validated pane dimensions");
                let text = meta.placeholder_text();
                let text_len = u16::try_from(text.len()).unwrap_or(0);
                let text_row = content.y + (content.rows - 1) / 2;
                let text_column = content.x - rect.x + (content.columns - text_len) / 2;
                for row in 0..rect.rows {
                    let host_row = rect.y + row;
                    write_ascii_run(
                        output,
                        host_row,
                        rect.x,
                        rect.columns,
                        |column| excluded(exclude, host_row, column),
                        |offset| {
                            if host_row == text_row && offset >= text_column {
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

    /// Clear pane margins when an overlay moves or disappears over them.
    fn paint_padding<W: Write>(
        &self,
        output: &mut W,
        rect: Rect,
        exclude: Option<Rect>,
        damage: [Option<Rect>; 2],
    ) -> io::Result<()> {
        let damaged = |row: u16, column: u16| {
            damage
                .iter()
                .flatten()
                .any(|rect| rect.contains(column, row))
        };
        output.write_all(b"\x1b[0m")?;
        for row in [rect.y, rect.y + rect.rows - 1] {
            write_ascii_run(
                output,
                row,
                rect.x,
                rect.columns,
                |column| !damaged(row, column) || excluded(exclude, row, column),
                |_| b' ',
            )?;
        }
        let right = rect.x + rect.columns - 1;
        for row in rect.y + 1..rect.y + rect.rows - 1 {
            for column in [rect.x, right] {
                if damaged(row, column) {
                    write_ascii_run(
                        output,
                        row,
                        column,
                        1,
                        |candidate| excluded(exclude, row, candidate),
                        |_| b' ',
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Paint pane controls and the bottom-right version in existing padding, restoring only cells
    /// damaged by a moved modal overlay during incremental rendering.
    fn paint_pane_controls<W: Write>(
        &self,
        output: &mut W,
        slot: usize,
        repaint_all: bool,
        history_changed: bool,
        damage: [Option<Rect>; 2],
    ) -> io::Result<()> {
        if !self.padded {
            return Ok(());
        }
        let meta = &self.pending_meta.slots[slot];
        let Some(rect) = meta.rect.filter(|_| meta.kind != SlotKind::Empty) else {
            return Ok(());
        };
        let Some(controls) = pane_controls(rect) else {
            return Ok(());
        };
        let focused = self.pending_meta.focused == Some(slot);
        let damaged = |row: u16, column: u16| {
            damage
                .iter()
                .flatten()
                .any(|rect| rect.contains(column, row))
        };
        for (hitbox, label, role) in [
            (controls.menu, b"Menu".as_slice(), ControlRole::Secondary),
            (controls.close, b"x".as_slice(), ControlRole::Destructive),
        ] {
            let semantic = if meta.header_pressed == Some(hitbox == controls.close) {
                ControlState::Pressed
            } else {
                ControlState::Normal
            };
            let Some(layout) = layout_filled(hitbox, label.len() as u16) else {
                continue;
            };
            let colors = style(role, semantic, self.reduced_control_color);
            colors.write(output)?;
            write_ascii_run(
                output,
                layout.bounds.y,
                layout.bounds.x,
                layout.bounds.columns,
                |column| {
                    (!repaint_all && !damaged(hitbox.y, column))
                        || excluded(self.pending_meta.overlay, hitbox.y, column)
                },
                |offset| {
                    if offset >= layout.label.x - layout.bounds.x
                        && offset < layout.label.x - layout.bounds.x + layout.label.columns
                    {
                        label[usize::from(offset - (layout.label.x - layout.bounds.x))]
                    } else {
                        b' '
                    }
                },
            )?;
        }
        output.write_all(if focused { b"\x1b[0;1;36m" } else { b"\x1b[0m" })?;
        write_ascii_run(
            output,
            controls.focus_marker.y,
            controls.focus_marker.x,
            controls.focus_marker.columns,
            |column| {
                (!repaint_all && !damaged(controls.focus_marker.y, column))
                    || excluded(self.pending_meta.overlay, controls.focus_marker.y, column)
            },
            |_| if focused { b'>' } else { b' ' },
        )?;
        paint_pane_label(
            output,
            controls.label,
            meta,
            focused,
            repaint_all,
            damage,
            self.pending_meta.overlay,
        )?;
        output.write_all(b"\x1b[0m")?;
        // Only the pane touching the viewport's bottom-right corner owns the
        // badge. Its bottom margin is outside the child PTY and all hitboxes.
        if let Some(badge) = pane_version_badge(
            rect,
            self.pending_meta.host_rows,
            self.pending_meta.host_columns,
        ) {
            output.write_all(b"\x1b[0;2m")?;
            write_ascii_run(
                output,
                badge.y,
                badge.x,
                badge.columns,
                |column| {
                    (!repaint_all && !damaged(badge.y, column))
                        || excluded(self.pending_meta.overlay, badge.y, column)
                },
                |offset| VERSION_LABEL.as_bytes()[usize::from(offset)],
            )?;
        }
        self.paint_history_controls(output, slot, rect, repaint_all, history_changed, damage)?;
        output.write_all(b"\x1b[0m")
    }

    /// Emphasize the focused pane's left padding rail without repainting or
    /// changing any terminal content cells.
    fn paint_focus_border<W: Write>(
        &self,
        output: &mut W,
        slot: usize,
        focused: bool,
    ) -> io::Result<()> {
        if !self.padded {
            return Ok(());
        }
        let meta = &self.pending_meta.slots[slot];
        let Some(rect) = meta.rect.filter(|_| meta.kind != SlotKind::Empty) else {
            return Ok(());
        };
        let Some(content) = content_rect(rect) else {
            return Ok(());
        };
        let exclude = self.pending_meta.overlay;
        for row in content.y..content.y + content.rows {
            if excluded(exclude, row, rect.x) {
                continue;
            }
            output.write_all(if focused { b"\x1b[1;46m" } else { b"\x1b[0m" })?;
            write_ascii_run(output, row, rect.x, 1, |_| false, |_| b' ')?;
        }
        output.write_all(b"\x1b[0m")
    }

    fn paint_history_controls<W: Write>(
        &self,
        output: &mut W,
        slot: usize,
        rect: Rect,
        repaint_all: bool,
        history_changed: bool,
        damage: [Option<Rect>; 2],
    ) -> io::Result<()> {
        if !repaint_all && !history_changed && damage.iter().all(Option::is_none) {
            return Ok(());
        }
        let row = rect.y + rect.rows - 1;
        let start = rect.x + 1;
        let badge = pane_version_badge(
            rect,
            self.pending_meta.host_rows,
            self.pending_meta.host_columns,
        );
        let end = badge.map_or(rect.x + rect.columns - 1, |badge| badge.x.saturating_sub(1));
        let width = end.saturating_sub(start);
        if self.pending_meta.slots[slot].history.is_none()
            && self.displayed_meta.slots[slot].history.is_none()
        {
            return Ok(());
        }
        if width == 0 {
            return Ok(());
        }
        let controls = self.pending_meta.slots[slot]
            .history
            .and_then(|kind| pane_history_controls(rect, kind, badge));
        let changed_cells = |column: u16| {
            damage
                .iter()
                .flatten()
                .any(|area| area.contains(column, row))
        };
        write_ascii_run(
            output,
            row,
            start,
            width,
            |column| {
                (!repaint_all && !history_changed && !changed_cells(column))
                    || excluded(self.pending_meta.overlay, row, column)
            },
            |offset| {
                let column = start + offset;
                let Some(controls) = controls else {
                    return b' ';
                };
                if controls.label.contains(column, row) {
                    let offset = usize::from(column - controls.label.x);
                    self.pending_meta.slots[slot]
                        .history
                        .map(HistoryLabelKind::text)
                        .and_then(|label| label.get(offset).copied())
                        .unwrap_or(b' ')
                } else if controls.back.contains(column, row) {
                    controls.back_text[usize::from(column - controls.back.x)]
                } else {
                    b' '
                }
            },
        )?;
        if let Some(controls) = controls {
            let pressed = self.pending_meta.slots[slot].history_pressed;
            style(
                ControlRole::Secondary,
                if pressed {
                    ControlState::Pressed
                } else {
                    ControlState::Normal
                },
                self.reduced_control_color,
            )
            .write(output)?;
            write_ascii_run(
                output,
                row,
                controls.back.x,
                controls.back.columns,
                |column| excluded(self.pending_meta.overlay, row, column),
                |offset| {
                    if pressed && offset == 0 {
                        b'*'
                    } else {
                        controls.back_text[usize::from(offset)]
                    }
                },
            )?;
            output.write_all(b"\x1b[0m")?;
        }
        Ok(())
    }

    /// Paint a scrollbar in the right interior padding cell of one snapshot
    /// pane. When a menu moves, `damage` limits restoration to rows the menu
    /// covered; a changed scrollbar repaints its complete track.
    fn paint_scrollbar<W: Write>(
        &self,
        output: &mut W,
        slot: usize,
        repaint_all: bool,
        damage: [Option<Rect>; 2],
    ) -> io::Result<()> {
        if !self.padded {
            return Ok(());
        }
        let meta = &self.pending_meta.slots[slot];
        if meta.scrollbar.is_none() && self.displayed_meta.slots[slot].scrollbar.is_none() {
            return Ok(());
        }
        let Some(rect) = meta.rect.filter(|_| meta.kind == SlotKind::Snapshot) else {
            return Ok(());
        };
        let scrollbar = meta.scrollbar.or(self.displayed_meta.slots[slot].scrollbar);
        let Some(scrollbar) = scrollbar else {
            return Ok(());
        };
        let Some(geometry) = scrollbar_geometry(rect, scrollbar) else {
            return Ok(());
        };
        output.write_all(b"\x1b[0m")?;
        for offset in 0..geometry.track.rows {
            let row = geometry.track.y + offset;
            let damaged = damage
                .iter()
                .flatten()
                .any(|rect| rect.contains(geometry.track.x, row));
            if !repaint_all && !damaged {
                continue;
            }
            if excluded(self.pending_meta.overlay, row, geometry.track.x) {
                continue;
            }
            if meta.scrollbar.is_some_and(|bar| bar.total_rows > 0) {
                let thumb = geometry.thumb.contains(geometry.track.x, row);
                output.write_all(if thumb { b"\x1b[0;1m" } else { b"\x1b[0;2m" })?;
                write_ascii_run(
                    output,
                    row,
                    geometry.track.x,
                    1,
                    |_| false,
                    |_| {
                        if thumb {
                            b'#'
                        } else {
                            b'|'
                        }
                    },
                )?;
            } else {
                output.write_all(b"\x1b[0m")?;
                write_ascii_run(output, row, geometry.track.x, 1, |_| false, |_| b' ')?;
            }
        }
        output.write_all(b"\x1b[0m")
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
        for control in self.pending_meta.overlay_controls.iter().flatten() {
            let bounds = control.bounds;
            let left = bounds.x.max(rect.x);
            let right = bounds
                .x
                .saturating_add(bounds.columns)
                .min(rect.x + rect.columns);
            if bounds.y < rect.y || bounds.y >= rect.y + rect.rows || left >= right {
                continue;
            }
            let paint = style(control.role, control.state, self.reduced_control_color);
            paint.write(output)?;
            let start = usize::from(bounds.y - rect.y) * columns;
            write_ascii_run(
                output,
                bounds.y,
                left,
                right - left,
                |_| false,
                |offset| self.target_overlay[start + usize::from(left - rect.x + offset)],
            )?;
            output.write_all(b"\x1b[0m")?;
        }
        output.write_all(b"\x1b[0m")?;
        Ok(())
    }

    fn paint_header<W: Write>(&self, output: &mut W, repaint_all: bool) -> io::Result<()> {
        let columns = usize::from(self.pending_meta.host_columns);
        let active = self.pending_meta.header_present;
        let mut column = 0;
        while column < columns {
            let changed = repaint_all
                || !self.valid
                || self.target_header[column] != self.displayed_header[column]
                || self.pending_meta.header_present != self.displayed_meta.header_present;
            if !changed {
                column += 1;
                continue;
            }
            let selected = active && self.target_header[column].selected();
            write_position(output, 0, column as u16)?;
            if active && self.target_header[column].filled {
                let state = if self.target_header[column].pending {
                    ControlState::Pending
                } else if self.target_header[column].pressed {
                    ControlState::Pressed
                } else {
                    ControlState::Normal
                };
                let role = if selected {
                    ControlRole::Primary
                } else {
                    ControlRole::Secondary
                };
                let style =
                    crate::terminal::controls::style(role, state, self.reduced_control_color);
                style.write(output)?;
            } else {
                output.write_all(if selected { b"\x1b[0;7m" } else { b"\x1b[0m" })?;
            }
            let cell = self.target_header[column];
            if active && !cell.is_continuation() && !cell.text().is_empty() {
                output.write_all(cell.text().as_bytes())?;
            } else {
                output.write_all(b" ")?;
            }
            column += if active && cell.is_wide() { 2 } else { 1 };
        }
        output.write_all(b"\x1b[0m")
    }
}

fn validate_selection(frame: &ComposeFrame<'_>, selection: SelectionOverlay<'_>) -> io::Result<()> {
    let invalid_selection = || invalid("selection overlay does not match its pane snapshot");
    let Some(Some(PaneView {
        content: PaneContent::Snapshot(live),
        ..
    })) = frame.panes.get(selection.slot).copied()
    else {
        return Err(invalid_selection());
    };
    validate_snapshot(selection.frozen, live.rows(), live.columns())?;
    if selection.frozen.rows() != live.rows()
        || selection.frozen.columns() != live.columns()
        || selection.frozen.alternate_screen() != live.alternate_screen()
        || selection.anchor.row >= live.rows()
        || selection.anchor.column >= live.columns()
        || selection.end.row >= live.rows()
        || selection.end.column >= live.columns()
    {
        return Err(invalid_selection());
    }
    Ok(())
}

fn validate_copy_caret(frame: &ComposeFrame<'_>, caret: CopyCaretOverlay<'_>) -> io::Result<()> {
    let invalid_caret = || invalid("copy caret does not match its pane snapshot");
    let Some(Some(PaneView {
        content: PaneContent::Snapshot(live),
        ..
    })) = frame.panes.get(caret.slot).copied()
    else {
        return Err(invalid_caret());
    };
    validate_snapshot(caret.frozen, live.rows(), live.columns())?;
    if caret.frozen.rows() != live.rows()
        || caret.frozen.columns() != live.columns()
        || caret.frozen.alternate_screen() != live.alternate_screen()
        || caret.point.row >= live.rows()
        || caret.point.column >= live.columns()
    {
        return Err(invalid_caret());
    }
    Ok(())
}

fn validate_scrollbars(
    frame: &ComposeFrame<'_>,
    padded: bool,
    scrollbars: [Option<Scrollbar>; crate::pane_layout::MAX_PANES],
) -> io::Result<()> {
    for (slot, scrollbar) in scrollbars.into_iter().enumerate() {
        let Some(scrollbar) = scrollbar else {
            continue;
        };
        if !padded {
            return Err(invalid("scrollbar requires padded compositor"));
        }
        if scrollbar.offset_rows > scrollbar.total_rows {
            return Err(invalid("scrollbar offset exceeds total history"));
        }
        if !matches!(
            frame.panes.get(slot).copied().flatten(),
            Some(PaneView {
                content: PaneContent::Snapshot(_),
                ..
            })
        ) {
            return Err(invalid("scrollbar slot is not a snapshot pane"));
        }
    }
    Ok(())
}

fn scrollbar_thumb(track_rows: u16, scrollbar: Scrollbar) -> u16 {
    if scrollbar.total_rows == 0 {
        return track_rows;
    }
    let track = u64::from(track_rows);
    let document = u64::from(scrollbar.total_rows) + track;
    ((track * track / document).max(1).min(track)) as u16
}

fn scrollbar_thumb_top(track_rows: u16, thumb_rows: u16, scrollbar: Scrollbar) -> u16 {
    if scrollbar.total_rows == 0 {
        return 0;
    }
    let travel = u64::from(track_rows - thumb_rows);
    let remaining = u64::from(scrollbar.total_rows - scrollbar.offset_rows);
    (travel * remaining / u64::from(scrollbar.total_rows)) as u16
}

fn paint_copy_caret(target: &mut ScreenSnapshot, point: Point) {
    let column = if target
        .cell(point.row, point.column)
        .is_some_and(SnapshotCell::is_wide_continuation)
        && point.column > 0
    {
        point.column - 1
    } else {
        point.column
    };
    let index = usize::from(point.row) * usize::from(target.columns()) + usize::from(column);
    target.cells[index].inverse = true;
    if target.cells[index].wide && column + 1 < target.columns() {
        let continuation = index + 1;
        target.cells[continuation].inverse = true;
    }
}

fn paint_selection(target: &mut ScreenSnapshot, selection: SelectionOverlay<'_>) {
    let frozen = selection.frozen;
    let (start, end) = ordered_points(selection.anchor, selection.end);
    for row in start.row..=end.row {
        let mut first = if row == start.row { start.column } else { 0 };
        let mut last = if row == end.row {
            end.column
        } else {
            target.columns() - 1
        };
        if frozen
            .cell(row, first)
            .is_some_and(SnapshotCell::is_wide_continuation)
            && first > 0
        {
            first -= 1;
        }
        if frozen.cell(row, last).is_some_and(SnapshotCell::is_wide) && last + 1 < frozen.columns()
        {
            last += 1;
        }
        for column in first..=last {
            let index = usize::from(row) * usize::from(target.columns()) + usize::from(column);
            target.cells[index] = *frozen.cell(row, column).expect("validated cell geometry");
            target.cells[index].inverse = true;
        }
        // A live wide glyph can straddle a frozen/live boundary. Clear an
        // orphan half only when it touches this selected range; unrelated live
        // cells retain their normal renderer fallback behavior.
        let base = usize::from(row) * usize::from(target.columns());
        for column in 0..target.columns() {
            let index = base + usize::from(column);
            let selected = (first..=last).contains(&column);
            let left_selected = column > 0 && (first..=last).contains(&(column - 1));
            let right_selected =
                column + 1 < target.columns() && (first..=last).contains(&(column + 1));
            if target.cells[index].wide {
                let paired =
                    column + 1 < target.columns() && target.cells[index + 1].wide_continuation;
                if !paired && (selected || right_selected) {
                    target.cells[index] = SnapshotCell::default();
                }
            } else if target.cells[index].wide_continuation
                && (column == 0 || !target.cells[index - 1].wide)
                && (selected || left_selected)
            {
                target.cells[index] = SnapshotCell::default();
            }
        }
    }
}

fn ordered_points(a: Point, b: Point) -> (Point, Point) {
    if (a.row, a.column) <= (b.row, b.column) {
        (a, b)
    } else {
        (b, a)
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

fn safe_pane_label_character(character: char) -> char {
    let width = UnicodeWidthChar::width(character).unwrap_or(0);
    if character.is_control()
        || width == 0
        || matches!(
            character,
            '\u{2028}'
                | '\u{2029}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{206f}'
        )
    {
        '?'
    } else {
        character
    }
}

fn pane_label_character_width(character: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(character).unwrap_or(1).max(1)).unwrap_or(2)
}

fn store_pane_label(slot: &mut SlotMeta, label: &str) {
    slot.label.fill(0);
    let mut len = 0_usize;
    slot.label_truncated = false;
    for character in label.chars().map(safe_pane_label_character) {
        let mut encoded = [0; 4];
        let bytes = character.encode_utf8(&mut encoded).as_bytes();
        let Some(end) = len.checked_add(bytes.len()) else {
            slot.label_truncated = true;
            break;
        };
        if end > slot.label.len() {
            slot.label_truncated = true;
            break;
        }
        slot.label[len..end].copy_from_slice(bytes);
        len = end;
    }
    slot.label_len = u16::try_from(len).unwrap_or(0);
}

fn same_pane_label(left: &SlotMeta, right: &SlotMeta) -> bool {
    left.label_len == right.label_len
        && left.label_truncated == right.label_truncated
        && left.label[..usize::from(left.label_len)] == right.label[..usize::from(right.label_len)]
}

fn paint_pane_label<W: Write>(
    output: &mut W,
    rect: Rect,
    meta: &SlotMeta,
    focused: bool,
    repaint_all: bool,
    damage: [Option<Rect>; 2],
    overlay: Option<Rect>,
) -> io::Result<()> {
    let label = std::str::from_utf8(&meta.label[..usize::from(meta.label_len)])
        .expect("stored pane labels are valid UTF-8");
    let damaged = |column| {
        damage
            .iter()
            .flatten()
            .any(|area| area.contains(column, rect.y))
    };
    output.write_all(b"\x1b[0m")?;
    write_ascii_run(
        output,
        rect.y,
        rect.x,
        rect.columns,
        |column| (!repaint_all && !damaged(column)) || excluded(overlay, rect.y, column),
        |_| b' ',
    )?;

    let total_width = label
        .chars()
        .map(pane_label_character_width)
        .fold(0_u16, u16::saturating_add);
    let truncated = meta.label_truncated || total_width > rect.columns;
    let text_limit = if truncated {
        rect.columns.saturating_sub(1)
    } else {
        rect.columns
    };
    let mut used = 0_u16;
    output.write_all(if focused { b"\x1b[1;36m" } else { b"\x1b[0m" })?;
    for character in label.chars() {
        let width = pane_label_character_width(character);
        if used.saturating_add(width) > text_limit {
            break;
        }
        let needs_paint =
            repaint_all || (used..used + width).any(|offset| damaged(rect.x + offset));
        let covered = (used..used + width).any(|offset| excluded(overlay, rect.y, rect.x + offset));
        if needs_paint && !covered {
            write_position(output, rect.y, rect.x + used)?;
            let mut encoded = [0; 4];
            output.write_all(character.encode_utf8(&mut encoded).as_bytes())?;
        }
        used += width;
    }
    if truncated && rect.columns > 0 {
        let offset = rect.columns - 1;
        if (repaint_all || damaged(rect.x + offset)) && !excluded(overlay, rect.y, rect.x + offset)
        {
            write_position(output, rect.y, rect.x + offset)?;
            output.write_all(b"~")?;
        }
    }
    output.write_all(b"\x1b[0m")
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

fn validate_header(header: &[HeaderCell], host_columns: u16) -> io::Result<()> {
    if header.len() != usize::from(host_columns) || header.len() > MAX_COLUMNS {
        return Err(invalid("header length does not match host columns"));
    }
    for (index, cell) in header.iter().enumerate() {
        if cell.is_continuation() {
            if index == 0
                || !header[index - 1].is_wide()
                || header[index - 1].selected() != cell.selected()
            {
                return Err(invalid("header continuation has no matching wide glyph"));
            }
        } else if cell.is_wide()
            && (index + 1 == header.len()
                || !header[index + 1].is_continuation()
                || header[index + 1].selected() != cell.selected())
        {
            return Err(invalid("wide header glyph has no matching continuation"));
        }
    }
    Ok(())
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

fn pane_content_rect(rect: Rect, padded: bool) -> Option<Rect> {
    if padded {
        content_rect(rect)
    } else {
        Some(rect)
    }
}

fn validate_frame(frame: &ComposeFrame<'_>, padded: bool, header: bool) -> io::Result<()> {
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
        if header && pane.rect.y == 0 {
            return Err(invalid("pane overlaps persistent header row"));
        }
        let content = pane_content_rect(pane.rect, padded)
            .ok_or_else(|| invalid("pane rectangle is smaller than the minimum content size"))?;
        if let PaneContent::Snapshot(snapshot) = pane.content {
            if snapshot.rows() != content.rows || snapshot.columns() != content.columns {
                return Err(invalid(
                    "pane snapshot does not match its content rectangle",
                ));
            }
            validate_snapshot(snapshot, content.rows, content.columns)?;
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
        if header && divider.y == 0 {
            return Err(invalid("divider overlaps persistent header row"));
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
    if let Some(overlay) = frame.overlay {
        if overlay.controls.len() > MAX_OVERLAY_CONTROLS
            || overlay.controls.iter().any(|control| {
                control.bounds.rows != 1
                    || control
                        .bounds
                        .x
                        .checked_add(control.bounds.columns)
                        .is_none()
                    || control.bounds.y.checked_add(1).is_none()
            })
        {
            return Err(invalid("overlay control geometry exceeds bounds"));
        }
        let clipped = clip_to_host(overlay.rect, frame);
        if overlay.rect.x >= host_columns
            || overlay.rect.y >= host_rows
            || overlay.rect.rows == 0
            || overlay.rect.columns == 0
            || !within_limits(clipped)
        {
            return Err(invalid("overlay rectangle is invalid"));
        }
        if header && overlay.rect.y == 0 {
            return Err(invalid("overlay overlaps persistent header row"));
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

    /// Two panes with one-cell padding and a one-column divider.
    fn side_by_side<'a>(left: &'a ScreenSnapshot, right: &'a ScreenSnapshot) -> ComposeFrame<'a> {
        let rows = left.rows() + 2;
        let columns = left.columns() + 2;
        let right_x = columns + 1;
        let host_rows = rows.max(right.rows() + 2);
        let host_columns = right_x + right.columns() + 2;
        ComposeFrame {
            host_rows,
            host_columns,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, rows, columns),
                    content: PaneContent::Snapshot(left),
                }),
                Some(PaneView {
                    rect: rect(0, right_x, right.rows() + 2, right.columns() + 2),
                    content: PaneContent::Snapshot(right),
                }),
                None,
                None,
            ],
            divider: Some(rect(0, columns, rows, 1)),
            dividers: [Some(rect(0, columns, rows, 1)), None, None],
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

    fn label_at(parser: &Parser, row: u16, column: u16, width: u16) -> String {
        (0..width)
            .map(|offset| cell_text(parser, row, column + offset))
            .collect()
    }

    fn chrome_rendered_committed(
        compositor: &mut Compositor,
        frame: &ComposeFrame<'_>,
        header: Option<&[HeaderCell]>,
        scrollbars: [Option<Scrollbar>; crate::pane_layout::MAX_PANES],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        compositor
            .render_with_chrome(frame, None, None, header, scrollbars, &mut bytes)
            .unwrap();
        assert!(compositor.commit());
        bytes
    }

    fn history_rendered_committed(
        compositor: &mut Compositor,
        frame: &ComposeFrame<'_>,
        history: [Option<HistoryLabelKind>; crate::pane_layout::MAX_PANES],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        compositor
            .render_with_history_controls(
                frame,
                None,
                None,
                None,
                PaneChrome {
                    scrollbars: [None; crate::pane_layout::MAX_PANES],
                    history,
                    header_pressed: [None; crate::pane_layout::MAX_PANES],
                    history_pressed: [false; crate::pane_layout::MAX_PANES],
                    labels: [None; crate::pane_layout::MAX_PANES],
                },
                &mut bytes,
            )
            .unwrap();
        assert!(compositor.commit());
        bytes
    }

    fn selection_rendered_committed(
        compositor: &mut Compositor,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        compositor
            .render_with_selection(frame, selection, &mut bytes)
            .unwrap();
        assert!(compositor.commit());
        bytes
    }

    fn copy_mode_rendered_committed(
        compositor: &mut Compositor,
        frame: &ComposeFrame<'_>,
        selection: Option<SelectionOverlay<'_>>,
        caret: CopyCaretOverlay<'_>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        compositor
            .render_with_copy_mode(frame, selection, caret, None, &mut bytes)
            .unwrap();
        assert!(compositor.commit());
        bytes
    }

    fn single_pane(snapshot: &ScreenSnapshot) -> ComposeFrame<'_> {
        ComposeFrame {
            host_rows: snapshot.rows() + 2,
            host_columns: snapshot.columns() + 2,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, snapshot.rows() + 2, snapshot.columns() + 2),
                    content: PaneContent::Snapshot(snapshot),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; crate::pane_layout::MAX_PANES - 1],
            focused: None,
            overlay: None,
        }
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

    fn assert_blank(parser: &Parser, row: u16, column: u16) {
        let text = cell_text(parser, row, column);
        assert!(
            text.is_empty() || text == " ",
            "cell {row},{column} was {text:?}"
        );
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
                let left_text = left.contents();
                let right_text = right.contents();
                assert!(
                    left_text == right_text
                        || (left_text.is_empty() && right_text == " ")
                        || (left_text == " " && right_text.is_empty()),
                    "cell {row},{column}: {left_text:?} != {right_text:?}"
                );
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
        assert!(contains(&output, b"\x1b[2;25H\x1b[0;39;49mR"));
        assert!(contains(&output, b"\x1b[3;25H\x1b[0;39;49mr"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "L");
        assert_eq!(cell_text(&parser, 1, 4), "T");
        assert_eq!(cell_text(&parser, 1, 24), "R");
        assert_eq!(cell_text(&parser, 2, 24), "r");
        for (row, column) in [(0, 0), (1, 0), (11, 21), (1, 21)] {
            assert_blank(&parser, row, column);
        }
        assert_eq!(label_at(&parser, 0, 1, 6), " Menu ");
        assert_eq!(label_at(&parser, 0, 18, 3), " x ");
        for row in 0..12 {
            assert_eq!(cell_text(&parser, row, 22), "|");
        }
        assert!(positions(&output)
            .iter()
            .all(|&(row, column)| row <= 12 && column <= 45));
    }

    #[test]
    fn top_bottom_panes_render_at_translated_rows_with_horizontal_divider() {
        let top = snapshot_from("TOP", 5, 20);
        let bottom = snapshot_from("BOTTOM", 5, 20);
        let frame = ComposeFrame {
            host_rows: 15,
            host_columns: 22,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 7, 22),
                    content: PaneContent::Snapshot(&top),
                }),
                Some(PaneView {
                    rect: rect(8, 0, 7, 22),
                    content: PaneContent::Snapshot(&bottom),
                }),
                None,
                None,
            ],
            divider: Some(rect(7, 0, 1, 22)),
            dividers: [Some(rect(7, 0, 1, 22)), None, None],
            focused: None,
            overlay: None,
        };
        let output = fresh_render(&frame);

        assert!(contains(&output, b"\x1b[10;2H\x1b[0;39;49mB"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "T");
        assert_eq!(cell_text(&parser, 9, 1), "B");
        assert_eq!(cell_text(&parser, 9, 6), "M");
        for column in 0..22 {
            assert_eq!(cell_text(&parser, 7, column), "-");
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
            host_rows: 25,
            host_columns: 45,
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 12, 22),
                    content: PaneContent::Snapshot(&panes[0]),
                }),
                Some(PaneView {
                    rect: rect(0, 23, 12, 22),
                    content: PaneContent::Snapshot(&panes[1]),
                }),
                Some(PaneView {
                    rect: rect(13, 0, 12, 22),
                    content: PaneContent::Snapshot(&panes[2]),
                }),
                Some(PaneView {
                    rect: rect(13, 23, 12, 22),
                    content: PaneContent::Snapshot(&panes[3]),
                }),
            ],
            divider: Some(rect(0, 22, 25, 1)),
            dividers: [
                Some(rect(0, 22, 25, 1)),
                Some(rect(12, 0, 1, 22)),
                Some(rect(12, 23, 1, 22)),
            ],
            focused: None,
            overlay: None,
        };
        let output = fresh_render(&frame);
        let rendered = screen(&output, &frame);
        assert_eq!(cell_text(&rendered, 1, 1), "P");
        assert_eq!(cell_text(&rendered, 1, 24), "P");
        assert_eq!(cell_text(&rendered, 14, 1), "P");
        assert_eq!(cell_text(&rendered, 14, 24), "P");
        for row in 0..25 {
            assert_eq!(cell_text(&rendered, row, 22), "|");
        }
        for column in 0..22 {
            assert_eq!(cell_text(&rendered, 12, column), "-");
            assert_eq!(cell_text(&rendered, 12, column + 23), "-");
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
        assert_eq!(positions(&output), vec![(2, 26)]);
        assert!(contains(&output, b"\x1b[2;26H\x1b[0;39;49mX"));
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
        assert_eq!(output, b"\x1b[4;28H");
    }

    #[test]
    fn scrollbar_thumb_tracks_live_top_and_zero_history_positions() {
        let pane = snapshot_from("CONTENT", 5, 20);
        let frame = single_pane(&pane);
        let mut compositor = Compositor::try_new().unwrap();
        let top = chrome_rendered_committed(
            &mut compositor,
            &frame,
            None,
            [
                Some(Scrollbar {
                    offset_rows: 4,
                    total_rows: 4,
                }),
                None,
                None,
                None,
            ],
        );
        let parser = screen(&top, &frame);
        for row in 1..=2 {
            assert_eq!(cell_text(&parser, row, 21), "#");
            assert!(parser.screen().cell(row, 21).unwrap().bold());
        }
        for row in 3..=5 {
            assert_eq!(cell_text(&parser, row, 21), "|");
            assert!(parser.screen().cell(row, 21).unwrap().dim());
        }
        assert_eq!(cell_text(&parser, 1, 1), "C");
        assert_eq!(cell_text(&parser, 1, 6), "N");
        for column in [0, 21] {
            assert_blank(&parser, 0, column);
            assert_blank(&parser, 6, column);
        }

        let live = chrome_rendered_committed(
            &mut compositor,
            &frame,
            None,
            [
                Some(Scrollbar {
                    offset_rows: 0,
                    total_rows: 4,
                }),
                None,
                None,
                None,
            ],
        );
        let parser = screen(&live, &frame);
        for row in 1..=3 {
            assert_eq!(cell_text(&parser, row, 21), "|");
        }
        for row in 4..=5 {
            assert_eq!(cell_text(&parser, row, 21), "#");
        }

        let no_history = chrome_rendered_committed(
            &mut compositor,
            &frame,
            None,
            [
                Some(Scrollbar {
                    offset_rows: 0,
                    total_rows: 0,
                }),
                None,
                None,
                None,
            ],
        );
        let parser = screen(&no_history, &frame);
        // Without retained history the scrollbar column stays blank.
        for row in 1..=5 {
            assert_blank(&parser, row, 21);
        }
    }

    #[test]
    fn pane_controls_validate_geometry_and_render_only_in_top_padding() {
        let pane_rect = rect(5, 3, 7, 22);
        let controls = pane_controls(pane_rect).unwrap();
        assert_eq!(controls.menu, rect(5, 4, 1, 6));
        assert_eq!(controls.close, rect(5, 21, 1, 3));
        assert!(pane_controls(rect(0, 0, 6, 22)).is_none());
        assert!(pane_controls(rect(0, u16::MAX - 20, 7, 22)).is_none());

        let pane = snapshot_from("CONTENT", 5, 20);
        let frame = single_pane(&pane);
        let mut compositor = Compositor::try_new().unwrap();
        let output = rendered_committed(&mut compositor, &frame);
        let parser = screen(&output, &frame);
        assert_eq!(label_at(&parser, 0, 1, 6), " Menu ");
        assert_eq!(label_at(&parser, 0, 18, 3), " x ");
        assert_eq!(cell_text(&parser, 1, 1), "C");
        assert_eq!(cell_text(&parser, 1, 7), "T");
        assert_eq!(pane.rows(), 5);
        assert_eq!(pane.columns(), 20);
        assert!(rendered(&mut compositor, &frame).is_empty());
        assert!(compositor.commit());

        let mut placeholder = frame;
        placeholder.panes[0] = Some(PaneView {
            rect: rect(0, 0, 7, 22),
            content: PaneContent::Placeholder("pending"),
        });
        let placeholder_output = fresh_render(&placeholder);
        let parser = screen(&placeholder_output, &placeholder);
        assert_eq!(label_at(&parser, 0, 1, 6), " Menu ");
        assert_eq!(label_at(&parser, 0, 18, 3), " x ");

        let mut unpadded = Compositor::try_new_unpadded().unwrap();
        let mut recovery = frame;
        recovery.host_rows = 5;
        recovery.host_columns = 20;
        recovery.panes[0].as_mut().unwrap().rect = rect(0, 0, 5, 20);
        let recovery_output = rendered(&mut unpadded, &recovery);
        let parser = screen(&recovery_output, &recovery);
        assert_eq!(cell_text(&parser, 0, 0), "C");
        assert!(!contains(&recovery_output, b" Menu "));
        assert!(!contains(&recovery_output, b" x "));
    }

    #[test]
    fn pane_controls_respect_modal_overlay_and_restore_after_it_moves() {
        let pane = snapshot_from("content", 5, 20);
        let base = single_pane(&pane);
        let lines = [OverlayLine {
            text: "MODAL",
            style: OverlayStyle::Selected,
        }];
        let mut covered = base;
        covered.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(0, 2, 1, 5),
            lines: &lines,
        });
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = rendered_committed(&mut compositor, &base);
        stream.extend(rendered_committed(&mut compositor, &covered));
        let covered_parser = screen(&stream, &base);
        assert_eq!(label_at(&covered_parser, 0, 2, 5), "MODAL");
        stream.extend(rendered_committed(&mut compositor, &base));
        let restored = screen(&stream, &base);
        assert_eq!(label_at(&restored, 0, 1, 6), " Menu ");
        assert_eq!(label_at(&restored, 0, 18, 3), " x ");
        assert_eq!(cell_text(&restored, 1, 1), "c");
    }

    #[test]
    fn version_badge_uses_bottom_margin_and_survives_modal_damage() {
        let pane = snapshot_from("CONTENT", 5, 20);
        let base = single_pane(&pane);
        let row = base.host_rows - 1;
        let column = base.host_columns - 1 - VERSION_LABEL.len() as u16;
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = rendered_committed(&mut compositor, &base);
        let parser = screen(&stream, &base);
        assert_eq!(
            label_at(&parser, row, column, VERSION_LABEL.len() as u16),
            VERSION_LABEL
        );
        assert_eq!(label_at(&parser, 1, 1, 7), "CONTENT");
        assert!(rendered_committed(&mut compositor, &base).is_empty());

        let lines = [OverlayLine {
            text: "NOTICE",
            style: OverlayStyle::Selected,
        }];
        let mut covered = base;
        covered.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(row, column, 1, VERSION_LABEL.len() as u16),
            lines: &lines,
        });
        stream.extend(rendered_committed(&mut compositor, &covered));
        assert_eq!(label_at(&screen(&stream, &base), row, column, 6), "NOTICE");
        stream.extend(rendered_committed(&mut compositor, &base));
        assert_eq!(
            label_at(
                &screen(&stream, &base),
                row,
                column,
                VERSION_LABEL.len() as u16
            ),
            VERSION_LABEL
        );

        let mut placeholder = base;
        placeholder.panes[0].as_mut().unwrap().content = PaneContent::Placeholder("pending");
        stream.extend(rendered_committed(&mut compositor, &placeholder));
        assert_eq!(
            label_at(
                &screen(&stream, &base),
                row,
                column,
                VERSION_LABEL.len() as u16
            ),
            VERSION_LABEL
        );

        let other = snapshot_from("RIGHT", 5, 20);
        let split = side_by_side(&pane, &other);
        let split_output = fresh_render(&split);
        let split_screen = screen(&split_output, &split);
        assert_eq!(
            label_at(
                &split_screen,
                split.host_rows - 1,
                split.host_columns - 1 - VERSION_LABEL.len() as u16,
                VERSION_LABEL.len() as u16
            ),
            VERSION_LABEL
        );
        assert_eq!(
            label_at(
                &split_screen,
                split.host_rows - 1,
                column,
                VERSION_LABEL.len() as u16
            ),
            ""
        );
    }

    #[test]
    fn history_controls_share_render_and_hit_geometry_and_avoid_version_badge() {
        let narrow = rect(0, 0, 24, 22);
        let badge = pane_version_badge(narrow, 24, 22).unwrap();
        let controls =
            pane_history_controls(narrow, HistoryLabelKind::Truncated, Some(badge)).unwrap();
        assert_eq!(controls.back_text, b"[Live]");
        assert!(controls.label.x + controls.label.columns <= controls.back.x);
        assert!(controls.back.x + controls.back.columns < badge.x);

        let wide = rect(0, 0, 24, 82);
        let wide_badge = pane_version_badge(wide, 24, 82).unwrap();
        let wide_controls =
            pane_history_controls(wide, HistoryLabelKind::NewOutput, Some(wide_badge)).unwrap();
        assert_eq!(wide_controls.back_text, b"[Back to live]");
        assert!(wide_controls.back.x + wide_controls.back.columns < wide_badge.x);

        let pane = snapshot_from("HISTORY", 22, 80);
        let frame = single_pane(&pane);
        let bytes = history_rendered_committed(
            &mut Compositor::try_new().unwrap(),
            &frame,
            [Some(HistoryLabelKind::History), None, None, None],
        );
        let rendered = screen(&bytes, &frame);
        let rendered_controls = pane_history_controls(
            frame.panes[0].unwrap().rect,
            HistoryLabelKind::History,
            pane_version_badge(
                frame.panes[0].unwrap().rect,
                frame.host_rows,
                frame.host_columns,
            ),
        )
        .unwrap();
        assert_eq!(
            label_at(
                &rendered,
                rendered_controls.back.y,
                rendered_controls.back.x,
                rendered_controls.back.columns,
            ),
            "[Back to live]"
        );
        assert_eq!(
            label_at(
                &rendered,
                rendered_controls.label.y,
                rendered_controls.label.x,
                7
            ),
            "History"
        );
    }

    #[test]
    fn history_label_damage_repaints_and_restores_after_overlay() {
        let pane = snapshot_from("HISTORY", 22, 80);
        let base = single_pane(&pane);
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = history_rendered_committed(
            &mut compositor,
            &base,
            [Some(HistoryLabelKind::History), None, None, None],
        );
        let controls = pane_history_controls(
            base.panes[0].unwrap().rect,
            HistoryLabelKind::History,
            pane_version_badge(
                base.panes[0].unwrap().rect,
                base.host_rows,
                base.host_columns,
            ),
        )
        .unwrap();
        let repaint = history_rendered_committed(
            &mut compositor,
            &base,
            [Some(HistoryLabelKind::NewOutput), None, None, None],
        );
        assert!(!repaint.windows(7).any(|window| window == b"History"));
        stream.extend(repaint);
        let changed = screen(&stream, &base);
        assert_eq!(
            label_at(&changed, controls.label.y, controls.label.x, 10),
            "New output"
        );
        let modal = [OverlayLine {
            text: "MODAL",
            style: OverlayStyle::Selected,
        }];
        let mut covered = base;
        covered.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(23, 0, 1, base.host_columns),
            lines: &modal,
        });
        let mut overlay_bytes = Vec::new();
        compositor
            .render_with_history_controls(
                &covered,
                None,
                None,
                None,
                PaneChrome {
                    scrollbars: [None; crate::pane_layout::MAX_PANES],
                    history: [Some(HistoryLabelKind::NewOutput), None, None, None],
                    header_pressed: [None; crate::pane_layout::MAX_PANES],
                    history_pressed: [false; crate::pane_layout::MAX_PANES],
                    labels: [None; crate::pane_layout::MAX_PANES],
                },
                &mut overlay_bytes,
            )
            .unwrap();
        assert!(compositor.commit());
        stream.extend(overlay_bytes);
        assert_eq!(label_at(&screen(&stream, &base), 23, 0, 5), "MODAL");

        stream.extend(history_rendered_committed(
            &mut compositor,
            &base,
            [Some(HistoryLabelKind::NewOutput), None, None, None],
        ));
        let restored = screen(&stream, &base);
        assert_eq!(
            label_at(&restored, controls.label.y, controls.label.x, 10),
            "New output"
        );
        assert_eq!(
            label_at(
                &restored,
                controls.back.y,
                controls.back.x,
                controls.back.columns
            ),
            "[Back to live]"
        );
    }

    #[test]
    fn pane_controls_partial_write_failure_invalidates_for_full_repaint() {
        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                }
                let written = self.0.min(bytes.len());
                self.0 -= written;
                Ok(written)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let pane = snapshot_from("body", 5, 20);
        let frame = single_pane(&pane);
        let expected = fresh_render(&frame);
        for label in [b"Menu".as_slice(), VERSION_LABEL.as_bytes()] {
            let offset = expected
                .windows(label.len())
                .position(|window| window == label)
                .unwrap();
            let mut compositor = Compositor::try_new().unwrap();
            let error = compositor
                .render(&frame, &mut FailAfter(offset + 2))
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert!(!compositor.commit());

            let retry = rendered_committed(&mut compositor, &frame);
            assert!(contains(&retry, b"\x1b[2J"));
            let parser = screen(&retry, &frame);
            assert_eq!(label_at(&parser, 0, 1, 6), " Menu ");
            assert_eq!(label_at(&parser, 0, 18, 3), " x ");
            assert_eq!(cell_text(&parser, 1, 1), "b");
            assert_eq!(
                label_at(
                    &parser,
                    frame.host_rows - 1,
                    frame.host_columns - 1 - VERSION_LABEL.len() as u16,
                    VERSION_LABEL.len() as u16
                ),
                VERSION_LABEL
            );
        }
    }

    #[test]
    fn scrollbar_geometry_has_expected_track_and_thumb_positions() {
        let pane_rect = rect(4, 2, 7, 22);
        let at_top = scrollbar_geometry(
            pane_rect,
            Scrollbar {
                offset_rows: 4,
                total_rows: 4,
            },
        )
        .unwrap();
        assert_eq!(at_top.track, rect(5, 23, 5, 1));
        assert_eq!(at_top.thumb, rect(5, 23, 2, 1));
        let at_middle = scrollbar_geometry(
            pane_rect,
            Scrollbar {
                offset_rows: 2,
                total_rows: 4,
            },
        )
        .unwrap();
        assert_eq!(at_middle.thumb, rect(6, 23, 2, 1));
        let at_live = scrollbar_geometry(
            pane_rect,
            Scrollbar {
                offset_rows: 0,
                total_rows: 4,
            },
        )
        .unwrap();
        assert_eq!(at_live.thumb, rect(8, 23, 2, 1));
        assert!(scrollbar_geometry(
            pane_rect,
            Scrollbar {
                offset_rows: 5,
                total_rows: 4,
            }
        )
        .is_none());
        assert!(scrollbar_geometry(
            rect(0, 0, 6, 22),
            Scrollbar {
                offset_rows: 0,
                total_rows: 0,
            }
        )
        .is_none());
    }

    #[test]
    fn scrollbar_updates_removals_and_unchanged_frames_only_touch_margin() {
        let pane = snapshot_from("unchanged", 5, 20);
        let frame = single_pane(&pane);
        let bar = [
            Some(Scrollbar {
                offset_rows: 2,
                total_rows: 12,
            }),
            None,
            None,
            None,
        ];
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = chrome_rendered_committed(&mut compositor, &frame, None, bar);
        assert!(chrome_rendered_committed(&mut compositor, &frame, None, bar).is_empty());

        let moved = [
            Some(Scrollbar {
                offset_rows: 0,
                total_rows: 12,
            }),
            None,
            None,
            None,
        ];
        let output = chrome_rendered_committed(&mut compositor, &frame, None, moved);
        assert!(!contains(&output, b"\x1b[2J"));
        assert!(!contains(&output, b"unchanged"));
        assert!(positions(&output).iter().all(|&(_, column)| column == 22));
        stream.extend(output);
        let parser = screen(&stream, &frame);
        assert_eq!(cell_text(&parser, 5, 21), "#");
        assert_eq!(cell_text(&parser, 1, 1), "u");

        let removed = chrome_rendered_committed(
            &mut compositor,
            &frame,
            None,
            [None; crate::pane_layout::MAX_PANES],
        );
        assert!(!contains(&removed, b"\x1b[2J"));
        assert!(positions(&removed).iter().all(|&(_, column)| column == 22));
        stream.extend(removed);
        let parser = screen(&stream, &frame);
        for row in 1..=5 {
            assert_blank(&parser, row, 21);
        }
        assert_eq!(cell_text(&parser, 1, 1), "u");
    }

    #[test]
    fn scrollbar_is_clipped_by_menu_and_restored_when_menu_leaves() {
        let pane = snapshot_from("content", 5, 20);
        let base = single_pane(&pane);
        let lines = [OverlayLine {
            text: "MENU",
            style: OverlayStyle::Selected,
        }];
        let mut covered = base;
        covered.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(2, 21, 1, 1),
            lines: &lines,
        });
        let bar = [
            Some(Scrollbar {
                offset_rows: 4,
                total_rows: 4,
            }),
            None,
            None,
            None,
        ];
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = chrome_rendered_committed(&mut compositor, &base, None, bar);
        stream.extend(chrome_rendered_committed(
            &mut compositor,
            &covered,
            None,
            bar,
        ));
        stream.extend(chrome_rendered_committed(&mut compositor, &base, None, bar));
        let parser = screen(&stream, &base);
        assert_eq!(cell_text(&parser, 2, 21), "#");
        assert!(parser.screen().cell(2, 21).unwrap().bold());
        assert_eq!(cell_text(&parser, 1, 21), "#");
        assert_eq!(cell_text(&parser, 4, 21), "|");
    }

    #[test]
    fn scrollbar_partial_write_retries_and_header_keeps_content_offset() {
        let pane = snapshot_from("body", 5, 20);
        let mut frame = single_pane(&pane);
        frame.host_rows += 1;
        frame.panes[0].as_mut().unwrap().rect.y = 1;
        let header = [HeaderCell::blank(false); 22];
        let bars = [
            Some(Scrollbar {
                offset_rows: 0,
                total_rows: 4,
            }),
            None,
            None,
            None,
        ];
        let mut compositor = Compositor::try_new().unwrap();
        chrome_rendered_committed(&mut compositor, &frame, Some(&header), bars);

        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                }
                let written = self.0.min(bytes.len());
                self.0 -= written;
                Ok(written)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let shifted = [
            Some(Scrollbar {
                offset_rows: 4,
                total_rows: 4,
            }),
            None,
            None,
            None,
        ];
        assert_eq!(
            compositor
                .render_with_chrome(
                    &frame,
                    None,
                    None,
                    Some(&header),
                    shifted,
                    &mut FailAfter(12)
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(!compositor.commit());
        let retry = chrome_rendered_committed(&mut compositor, &frame, Some(&header), shifted);
        assert!(contains(&retry, b"\x1b[2J"));
        let parser = screen(&retry, &frame);
        assert_eq!(cell_text(&parser, 2, 21), "#");
        assert_eq!(cell_text(&parser, 3, 21), "#");
        assert_eq!(cell_text(&parser, 2, 1), "b");
    }

    #[test]
    fn scrollbar_rejects_invalid_slot_geometry_before_output() {
        let snapshot = snapshot_from("x", 5, 20);
        let frame = single_pane(&snapshot);
        let bar = Some(Scrollbar {
            offset_rows: 1,
            total_rows: 1,
        });
        let invalid_frames = [
            (frame, [None, bar, None, None]),
            (
                {
                    let mut placeholder = frame;
                    placeholder.panes[0] = Some(PaneView {
                        rect: rect(0, 0, 7, 22),
                        content: PaneContent::Placeholder("pending"),
                    });
                    placeholder
                },
                [bar, None, None, None],
            ),
        ];
        let mut compositor = Compositor::try_new().unwrap();
        for (invalid_frame, bars) in invalid_frames {
            let mut output = Vec::new();
            assert_eq!(
                compositor
                    .render_with_chrome(&invalid_frame, None, None, None, bars, &mut output)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(output.is_empty());
        }
        let mut output = Vec::new();
        assert_eq!(
            compositor
                .render_with_chrome(
                    &frame,
                    None,
                    None,
                    None,
                    [
                        Some(Scrollbar {
                            offset_rows: 2,
                            total_rows: 1
                        }),
                        None,
                        None,
                        None
                    ],
                    &mut output,
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(output.is_empty());
        let mut unpadded = Compositor::try_new_unpadded().unwrap();
        let mut unpadded_frame = frame;
        unpadded_frame.host_rows = 5;
        unpadded_frame.host_columns = 20;
        unpadded_frame.panes[0].as_mut().unwrap().rect = rect(0, 0, 5, 20);
        assert_eq!(
            unpadded
                .render_with_chrome(
                    &unpadded_frame,
                    None,
                    None,
                    None,
                    [bar, None, None, None],
                    &mut output,
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(output.is_empty());
    }

    #[test]
    fn one_cell_scrollbar_track_keeps_a_visible_thumb() {
        let bar = Scrollbar {
            offset_rows: 1,
            total_rows: 50,
        };
        let thumb_rows = scrollbar_thumb(1, bar);
        let thumb_top = scrollbar_thumb_top(1, thumb_rows, bar);
        assert_eq!(thumb_rows, 1);
        assert_eq!(thumb_top, 0);

        let snapshot = snapshot_from("x", 5, 20);
        let frame = single_pane(&snapshot);
        let mut compositor = Compositor::try_new().unwrap();
        let output =
            chrome_rendered_committed(&mut compositor, &frame, None, [Some(bar), None, None, None]);
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 4, 21), "#");
        assert_eq!(cell_text(&parser, 1, 21), "|");
        assert_blank(&parser, 0, 21);
        assert_blank(&parser, 6, 21);
    }

    #[test]
    fn unpadded_render_preserves_minimum_snapshot_without_margin_writes() {
        let snapshot = snapshot_from("preserved", 5, 20);
        let mut frame = single_pane(&snapshot);
        frame.host_rows = 5;
        frame.host_columns = 20;
        frame.panes[0].as_mut().unwrap().rect = rect(0, 0, 5, 20);
        let mut compositor = Compositor::try_new_unpadded().unwrap();
        let mut output = Vec::new();
        compositor.render(&frame, &mut output).unwrap();
        assert!(compositor.commit());
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 0, 0), "p");
        assert_eq!(cell_text(&parser, 0, 8), "d");

        let mut no_bar_output = Vec::new();
        compositor
            .render_with_chrome(
                &frame,
                None,
                None,
                None,
                [None; crate::pane_layout::MAX_PANES],
                &mut no_bar_output,
            )
            .unwrap();
        assert!(no_bar_output.is_empty());
        assert!(compositor.commit());
    }

    #[test]
    fn rect_or_host_change_and_invalidate_force_full_repaint() {
        let left = snapshot_from("L", 10, 20);
        let right = snapshot_from("R", 10, 20);
        let narrow = snapshot_from("R", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &side_by_side(&left, &right));

        let mut moved = side_by_side(&left, &narrow);
        moved.host_columns = 46;
        let output = rendered_committed(&mut compositor, &moved);
        assert!(output.starts_with(b"\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J"));

        let mut grown = moved;
        grown.host_columns = 47;
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
            controls: &[],
            rect: rect(1, 30, 4, 20),
            lines: &lines,
        });
        let output = fresh_render(&frame);

        assert!(positions(&output)
            .iter()
            .all(|&(row, column)| row <= 12 && column <= 45));
        assert!(contains(&output, b"\x1b[0;7m\x1b[2;31HSplit Left "));
        assert!(contains(&output, b"\x1b[0;2m\x1b[3;31HSplit Right"));
        assert!(!contains(&output, b"\x1b]52"));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 1, 29), "R");
        assert_eq!(cell_text(&parser, 1, 30), "S");
        assert_blank(&parser, 1, 40);
        assert!(parser.screen().cell(1, 30).unwrap().inverse());
        assert!(parser.screen().cell(2, 30).unwrap().dim());
        assert!(parser.screen().cell(3, 30).unwrap().bold());
        assert_eq!(cell_text(&parser, 3, 37), "?");
        assert_blank(&parser, 4, 30);

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
            controls: &[],
            rect: rect(0, 15, 3, 12),
            lines: &lines,
        });
        let mut moved = base;
        moved.overlay = Some(OverlayView {
            controls: &[],
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
        assert!(output.ends_with(b"\x1b[4;28H\x1b[?25h"));
        let parser = screen(&output, &frame);
        assert_eq!(parser.screen().cursor_position(), (3, 27));
        assert!(!parser.screen().hide_cursor());

        let mut overlaid = frame;
        let lines: [OverlayLine<'_>; 0] = [];
        overlaid.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(0, 0, 2, 5),
            lines: &lines,
        });
        let output = fresh_render(&overlaid);
        assert!(!contains(&output, b"\x1b[?25h"));
        assert!(screen(&output, &overlaid).screen().hide_cursor());

        let mut placeholder = frame;
        placeholder.panes[1] = Some(PaneView {
            rect: rect(0, 23, 12, 22),
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
        assert!(output.ends_with(b"\x1b[4;28H\x1b[?25h"));
    }

    #[test]
    fn placeholder_is_centered_sanitized_and_clipped() {
        let left = snapshot_from("L", 10, 20);
        let mut frame = side_by_side(&left, &left);
        frame.panes[1] = Some(PaneView {
            rect: rect(0, 23, 12, 22),
            content: PaneContent::Placeholder("starting"),
        });
        let parser = screen(&fresh_render(&frame), &frame);
        // Content is 10x20 at row 1, column 24; the eight-cell label centers at column 30.
        assert_eq!(cell_text(&parser, 5, 30), "s");
        assert_eq!(cell_text(&parser, 5, 37), "g");
        assert_eq!(cell_text(&parser, 5, 29), " ");

        frame.panes[1] = Some(PaneView {
            rect: rect(0, 23, 12, 22),
            content: PaneContent::Placeholder("launch_failed: \x1b]52;c;Q\x07 é and much more"),
        });
        let output = fresh_render(&frame);
        assert!(!contains(&output, b"\x1b]52"));
        assert!(!contains(&output, "é".as_bytes()));
        assert!(positions(&output).iter().all(|&(_, column)| column <= 45));
        let parser = screen(&output, &frame);
        assert_eq!(cell_text(&parser, 5, 24), "l");
        assert_eq!(cell_text(&parser, 5, 39), "?");
        assert_eq!(cell_text(&parser, 5, 43), ";");
        assert_eq!(cell_text(&parser, 5, 22), "|");

        // Changing the placeholder text repaints only that pane.
        let mut compositor = Compositor::try_new().unwrap();
        rendered_committed(&mut compositor, &frame);
        frame.panes[1] = Some(PaneView {
            rect: rect(0, 23, 12, 22),
            content: PaneContent::Placeholder("resize_pending"),
        });
        let output = rendered_committed(&mut compositor, &frame);
        assert!(!contains(&output, b"\x1b[2J"));
        assert!(positions(&output).iter().all(|&(_, column)| column >= 24));
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
        frame.panes[1] = pane(rect(0, 23, 12, 22), &small);
        cases.push(frame);
        let mut frame = valid;
        frame.panes[1] = pane(rect(0, 15, 12, 22), &left);
        frame.divider = None;
        cases.push(frame);
        let mut frame = valid;
        frame.divider = Some(rect(0, 21, 12, 1));
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
        frame.divider = Some(rect(0, 22, 12, 2));
        frame.dividers = [Some(rect(0, 22, 12, 2)), None, None];
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
        assert_eq!(cell_text(&parser, 1, 20), "?");
        assert_blank(&parser, 1, 21);
        assert_eq!(cell_text(&parser, 1, 22), "|");
        assert_eq!(cell_text(&parser, 1, 24), "界");
        assert_eq!(cell_text(&parser, 1, 26), "X");

        // A wide glyph straddling the overlay's left edge keeps a fallback.
        let lines = [OverlayLine {
            text: "MENU",
            style: OverlayStyle::Normal,
        }];
        let mut overlaid = frame;
        overlaid.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(1, 25, 1, 4),
            lines: &lines,
        });
        let output = fresh_render(&overlaid);
        assert!(!contains(&output, "界X".as_bytes()));
        let parser = screen(&output, &overlaid);
        assert_eq!(cell_text(&parser, 1, 24), "?");
        assert_eq!(cell_text(&parser, 1, 25), "M");
        assert_eq!(cell_text(&parser, 1, 28), "U");
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
            controls: &[],
            rect: rect(3, 18, 3, 10),
            lines: &lines,
        });
        let mut placeholder = side_by_side(&c, &a);
        placeholder.panes[0] = Some(PaneView {
            rect: rect(0, 0, 12, 22),
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
        let mut first = snapshot_from("", 78, 117);
        let maximum_cell_text = format!("A{}", "\u{20d0}".repeat(7));
        for cell in &mut first.cells {
            set_text(cell, &maximum_cell_text);
            cell.bold = true;
            cell.inverse = true;
            cell.foreground = crate::terminal::Color::Rgb(255, 255, 255);
            cell.background = crate::terminal::Color::Rgb(255, 255, 255);
        }
        let mut second = snapshot_from("", 78, 118);
        second.copy_from(&first);
        second.columns = 118;
        second.cells.resize(78 * 118, first.cells[0]);
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

    #[test]
    fn frozen_selection_drag_updates_clear_incrementally_and_tracks_live_cells() {
        let frozen = snapshot_from("abcdefgh", 10, 20);
        let live_before = snapshot_from("abcdefgh", 10, 20);
        let live_during = snapshot_from("aXcdefgh", 10, 20);
        let mut compositor = Compositor::try_new().unwrap();
        let initial = single_pane(&live_before);
        let mut parser = Parser::new(initial.host_rows, initial.host_columns, 0);
        parser.process(&rendered_committed(&mut compositor, &initial));

        let forward = SelectionOverlay {
            slot: 0,
            frozen: &frozen,
            anchor: Point { row: 0, column: 1 },
            end: Point { row: 0, column: 3 },
        };
        let frame = single_pane(&live_during);
        let start = selection_rendered_committed(&mut compositor, &frame, Some(forward));
        assert!(!contains(&start, b"\x1b[2J"));
        parser.process(&start);
        assert_eq!(cell_text(&parser, 1, 2), "b");
        assert!(parser.screen().cell(1, 2).unwrap().inverse());
        assert_eq!(cell_text(&parser, 1, 5), "e");
        assert!(!parser.screen().cell(1, 5).unwrap().inverse());
        assert_eq!(cell_text(&parser, 1, 3), "c");

        let extended = SelectionOverlay {
            end: Point { row: 0, column: 5 },
            ..forward
        };
        parser.process(&selection_rendered_committed(
            &mut compositor,
            &frame,
            Some(extended),
        ));
        let reverse = SelectionOverlay {
            anchor: Point { row: 0, column: 5 },
            end: Point { row: 0, column: 2 },
            ..forward
        };
        parser.process(&selection_rendered_committed(
            &mut compositor,
            &frame,
            Some(reverse),
        ));
        let cleared = selection_rendered_committed(&mut compositor, &frame, None);
        assert!(!contains(&cleared, b"\x1b[2J"));
        parser.process(&cleared);
        assert_same_screen(&parser, &screen(&fresh_render(&frame), &frame), &frame);
        assert_eq!(cell_text(&parser, 1, 2), "X");
        assert!(!parser.screen().cell(1, 2).unwrap().inverse());
        assert_eq!(cell_text(&parser, 1, 4), "d");
    }

    #[test]
    fn copy_caret_moves_clears_and_recovers_after_writer_failure() {
        let frozen = snapshot_from("abc", 10, 20);
        let frame = single_pane(&frozen);
        let mut compositor = Compositor::try_new().unwrap();
        let caret = |column| CopyCaretOverlay {
            slot: 0,
            frozen: &frozen,
            point: Point { row: 0, column },
        };
        let first = copy_mode_rendered_committed(&mut compositor, &frame, None, caret(0));
        let mut parser = screen(&first, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "a");
        assert!(parser.screen().cell(1, 1).unwrap().inverse());

        let moved = copy_mode_rendered_committed(&mut compositor, &frame, None, caret(1));
        parser.process(&moved);
        assert_eq!(cell_text(&parser, 1, 1), "a");
        assert!(!parser.screen().cell(1, 1).unwrap().inverse());
        assert_eq!(cell_text(&parser, 1, 2), "b");
        assert!(parser.screen().cell(1, 2).unwrap().inverse());

        let cleared = rendered_committed(&mut compositor, &frame);
        parser.process(&cleared);
        assert!(!parser.screen().cell(1, 2).unwrap().inverse());
        assert_eq!(cell_text(&parser, 1, 2), "b");

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
            .render_with_copy_mode(&frame, None, caret(2), None, &mut FailAfter(5))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(!compositor.commit());
        let recovery = copy_mode_rendered_committed(&mut compositor, &frame, None, caret(2));
        assert!(contains(&recovery, b"\x1b[2J"));
        let parser = screen(&recovery, &frame);
        assert!(parser.screen().cell(1, 3).unwrap().inverse());
        assert!(!parser.screen().cell(1, 2).unwrap().inverse());

        let wide = snapshot_from("界x", 10, 20);
        let wide_frame = single_pane(&wide);
        let mut wide_compositor = Compositor::try_new().unwrap();
        let wide_caret = CopyCaretOverlay {
            slot: 0,
            frozen: &wide,
            point: Point { row: 0, column: 1 },
        };
        let output =
            copy_mode_rendered_committed(&mut wide_compositor, &wide_frame, None, wide_caret);
        let parser = screen(&output, &wide_frame);
        assert!(parser.screen().cell(1, 1).unwrap().is_wide());
        assert!(parser.screen().cell(1, 1).unwrap().inverse());
        assert!(parser.screen().cell(1, 2).unwrap().is_wide_continuation());
        // The VT parser models the continuation topology but does not expose
        // its rendition separately. Prove the emitted wide glyph itself is
        // preceded by inverse SGR, which styles the glyph's full host width.
        assert!(contains(&output, b"\x1b[0;7;39;49m\xE7\x95\x8C"));
    }

    #[test]
    fn selection_preserves_unicode_combining_and_wide_topology_at_edges() {
        let frozen = snapshot_from("界e\u{301}z", 10, 20);
        let live = snapshot_from("界e\u{301}z", 10, 20);
        let frame = single_pane(&live);
        let mut compositor = Compositor::try_new().unwrap();
        let range = SelectionOverlay {
            slot: 0,
            frozen: &frozen,
            anchor: Point { row: 0, column: 1 }, // continuation normalizes to the lead
            end: Point { row: 0, column: 2 },
        };
        let bytes = selection_rendered_committed(&mut compositor, &frame, Some(range));
        let parser = screen(&bytes, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "界");
        assert!(parser.screen().cell(1, 1).unwrap().is_wide());
        assert!(parser.screen().cell(1, 2).unwrap().is_wide_continuation());
        assert!(parser.screen().cell(1, 1).unwrap().inverse());

        let combining_range = SelectionOverlay {
            anchor: Point { row: 0, column: 2 },
            end: Point { row: 0, column: 3 },
            ..range
        };
        let mut combining_compositor = Compositor::try_new().unwrap();
        let bytes =
            selection_rendered_committed(&mut combining_compositor, &frame, Some(combining_range));
        let parser = screen(&bytes, &frame);
        assert_eq!(cell_text(&parser, 1, 3), "e\u{301}");
        assert!(parser.screen().cell(1, 3).unwrap().inverse());

        // Replacing only a live wide lead with a single frozen cell clears
        // the now-orphaned live continuation in the composed presentation.
        let frozen_narrow = snapshot_from("ab", 10, 20);
        let live_wide = snapshot_from("\x1b[1;2H界x", 10, 20);
        let frame = single_pane(&live_wide);
        let range = SelectionOverlay {
            slot: 0,
            frozen: &frozen_narrow,
            anchor: Point { row: 0, column: 0 },
            end: Point { row: 0, column: 1 },
        };
        let mut compositor = Compositor::try_new().unwrap();
        let bytes = selection_rendered_committed(&mut compositor, &frame, Some(range));
        let parser = screen(&bytes, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "a");
        assert_eq!(cell_text(&parser, 1, 2), "b");
        assert_blank(&parser, 1, 3);
        assert_eq!(cell_text(&parser, 1, 4), "x");
        assert_eq!(live_wide.cell(0, 1).unwrap().text(), "界");
        assert!(live_wide.cell(0, 2).unwrap().is_wide_continuation());
    }

    #[test]
    fn selection_rejects_invalid_geometry_before_output_and_menu_keeps_priority() {
        let frozen = snapshot_from("frozen", 10, 20);
        let live = snapshot_from("live", 10, 20);
        let frame = single_pane(&live);
        let mut compositor = Compositor::try_new().unwrap();
        let invalid_range = SelectionOverlay {
            slot: 0,
            frozen: &frozen,
            anchor: Point { row: 0, column: 0 },
            end: Point { row: 10, column: 0 },
        };
        let mut output = Vec::new();
        let error = compositor
            .render_with_selection(&frame, Some(invalid_range), &mut output)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(output.is_empty());

        let collapsed = SelectionOverlay {
            slot: 0,
            frozen: &frozen,
            anchor: Point { row: 0, column: 0 },
            end: Point { row: 0, column: 0 },
        };
        let bytes = selection_rendered_committed(&mut compositor, &frame, Some(collapsed));
        let parser = screen(&bytes, &frame);
        assert_eq!(cell_text(&parser, 1, 1), "l");
        assert!(!parser.screen().cell(1, 1).unwrap().inverse());

        let lines = [OverlayLine {
            text: "MENU",
            style: OverlayStyle::Selected,
        }];
        let mut menu = frame;
        menu.overlay = Some(OverlayView {
            controls: &[],
            rect: rect(1, 1, 1, 4),
            lines: &lines,
        });
        let selection = SelectionOverlay {
            slot: 0,
            frozen: &frozen,
            anchor: Point { row: 0, column: 0 },
            end: Point { row: 0, column: 5 },
        };
        let bytes = selection_rendered_committed(&mut compositor, &menu, Some(selection));
        let parser = screen(&bytes, &menu);
        assert_eq!(cell_text(&parser, 1, 1), "M");
        assert_eq!(cell_text(&parser, 1, 5), " ");
    }

    fn header_frame<'a>(overlay: Option<OverlayView<'a>>) -> ComposeFrame<'a> {
        ComposeFrame {
            host_rows: 6,
            host_columns: 20,
            panes: [None; crate::pane_layout::MAX_PANES],
            divider: None,
            dividers: [None; crate::pane_layout::MAX_PANES - 1],
            focused: None,
            overlay,
        }
    }

    fn header_with_label(label: &str, selected: bool) -> [HeaderCell; 20] {
        let mut cells = [HeaderCell::blank(false); 20];
        for (index, byte) in label.bytes().enumerate() {
            cells[index] =
                HeaderCell::glyph(std::str::from_utf8(&[byte]).unwrap(), selected).unwrap();
        }
        cells
    }

    #[test]
    fn header_changes_are_incremental_committed_and_removable() {
        let frame = header_frame(None);
        let first = header_with_label("Project", true);
        let second = header_with_label("Workspace", false);
        let mut compositor = Compositor::try_new().unwrap();
        let mut bytes = Vec::new();
        compositor
            .render_with_header(&frame, None, Some(&first), &mut bytes)
            .unwrap();
        assert!(contains(&bytes, b"\x1b[2J"));
        let mut complete_output = bytes.clone();
        assert!(compositor.commit());

        bytes.clear();
        compositor
            .render_with_header(&frame, None, Some(&second), &mut bytes)
            .unwrap();
        assert!(!contains(&bytes, b"\x1b[2J"));
        complete_output.extend_from_slice(&bytes);
        assert!(compositor.commit());

        bytes.clear();
        compositor
            .render_with_header(&frame, None, Some(&second), &mut bytes)
            .unwrap();
        assert!(bytes.is_empty());
        assert!(compositor.commit());

        bytes.clear();
        let lines = [OverlayLine {
            text: "modal content",
            style: OverlayStyle::Selected,
        }];
        let modal_at_header = header_frame(Some(OverlayView {
            controls: &[],
            rect: rect(0, 1, 1, 14),
            lines: &lines,
        }));
        compositor
            .render_with_header(&modal_at_header, None, None, &mut bytes)
            .unwrap();
        assert!(!bytes.is_empty());
        assert!(contains(&bytes, b"\x1b[2J"));
        complete_output.extend_from_slice(&bytes);
        let parser = screen(&complete_output, &modal_at_header);
        assert_blank(&parser, 0, 0);
        assert_eq!(cell_text(&parser, 0, 1), "m");
        assert_eq!(cell_text(&parser, 0, 2), "o");
        assert_blank(&parser, 0, 14);
    }

    #[test]
    fn full_repaint_restores_an_unchanged_header_after_geometry_change() {
        let frame = header_frame(None);
        let resized = ComposeFrame {
            host_rows: frame.host_rows - 1,
            ..frame
        };
        let cells = header_with_label("Project", true);
        let mut compositor = Compositor::try_new().unwrap();
        let mut complete_output = Vec::new();
        compositor
            .render_with_header(&frame, None, Some(&cells), &mut complete_output)
            .unwrap();
        assert!(compositor.commit());

        let mut resized_output = Vec::new();
        compositor
            .render_with_header(&resized, None, Some(&cells), &mut resized_output)
            .unwrap();
        assert!(contains(&resized_output, b"\x1b[2J"));
        complete_output.extend_from_slice(&resized_output);

        let parser = screen(&complete_output, &resized);
        assert_eq!(cell_text(&parser, 0, 0), "P");
        assert_eq!(cell_text(&parser, 0, 6), "t");
    }

    #[test]
    fn modal_overlay_composes_below_the_persistent_header() {
        let lines = [OverlayLine {
            text: "modal content",
            style: OverlayStyle::Selected,
        }];
        let frame = header_frame(Some(OverlayView {
            controls: &[],
            rect: rect(2, 1, 1, 14),
            lines: &lines,
        }));
        let cells = header_with_label("Project", true);
        let mut compositor = Compositor::try_new().unwrap();
        let mut bytes = Vec::new();
        compositor
            .render_with_header(&frame, None, Some(&cells), &mut bytes)
            .unwrap();
        let parser = screen(&bytes, &frame);
        assert_eq!(cell_text(&parser, 0, 0), "P");
        assert_eq!(cell_text(&parser, 0, 1), "r");
        assert_eq!(cell_text(&parser, 0, 6), "t");
        assert_eq!(cell_text(&parser, 2, 1), "m");
        assert_eq!(cell_text(&parser, 2, 2), "o");
        assert_eq!(cell_text(&parser, 2, 13), "t");
    }

    #[test]
    fn header_validates_wide_topology_and_rejects_row_overlap_before_output() {
        let frame = header_frame(None);
        let mut cells = [HeaderCell::blank(false); 20];
        cells[0] = HeaderCell::glyph("界", false).unwrap();
        let mut compositor = Compositor::try_new().unwrap();
        let mut output = Vec::new();
        assert_eq!(
            compositor
                .render_with_header(&frame, None, Some(&cells), &mut output)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(output.is_empty());

        cells[1] = HeaderCell::continuation(false);
        compositor
            .render_with_header(&frame, None, Some(&cells), &mut output)
            .unwrap();
        assert!(compositor.commit());

        let overlapping = ComposeFrame {
            panes: [
                Some(PaneView {
                    rect: rect(0, 0, 3, 20),
                    content: PaneContent::Placeholder("x"),
                }),
                None,
                None,
                None,
            ],
            ..frame
        };
        output.clear();
        assert_eq!(
            compositor
                .render_with_header(&overlapping, None, Some(&cells), &mut output)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(output.is_empty());

        let lines = [OverlayLine {
            text: "modal",
            style: OverlayStyle::Normal,
        }];
        let overlapping_modal = ComposeFrame {
            overlay: Some(OverlayView {
                controls: &[],
                rect: rect(0, 0, 1, 5),
                lines: &lines,
            }),
            ..frame
        };
        assert_eq!(
            compositor
                .render_with_header(&overlapping_modal, None, Some(&cells), &mut output)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(output.is_empty());

        assert!(HeaderCell::glyph("\u{1b}", false).is_none());
        assert!(HeaderCell::glyph("\u{301}", false).is_none());
        assert!(HeaderCell::glyph("\u{301}a", false).is_none());
        assert_eq!(
            HeaderCell::glyph("a\u{301}", false).unwrap().text(),
            "a\u{301}"
        );
        assert!(HeaderCell::glyph("abcdefghijklmnopqrstuvwxyzABCDEFG", false).is_none());
    }

    #[test]
    fn header_baseline_advances_only_after_commit_or_complete_write() {
        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::other("injected writer failure"));
                }
                let count = bytes.len().min(self.0);
                self.0 -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let frame = header_frame(None);
        let cells = header_with_label("A", true);
        let mut compositor = Compositor::try_new().unwrap();
        let mut first = Vec::new();
        compositor
            .render_with_header(&frame, None, Some(&cells), &mut first)
            .unwrap();
        let mut replay = Vec::new();
        compositor
            .render_with_header(&frame, None, Some(&cells), &mut replay)
            .unwrap();
        assert_eq!(first, replay, "uncommitted header changes must replay");
        assert!(compositor.commit());

        assert!(compositor
            .render_with_header(&frame, None, None, &mut FailAfter(5))
            .is_err());
        let mut retry = Vec::new();
        compositor
            .render_with_header(&frame, None, None, &mut retry)
            .unwrap();
        assert!(
            contains(&retry, b"\x1b[2J"),
            "a partial write invalidates the baseline"
        );
        assert!(compositor.commit());
    }

    #[test]
    fn shared_pointer_geometry_uses_exact_half_open_control_thumb_and_divider_edges() {
        let pane = rect(4, 7, 24, 40);
        let controls = pane_controls(pane).unwrap();
        for hitbox in [controls.menu, controls.close] {
            assert!(hitbox.contains(hitbox.x, hitbox.y));
            assert!(hitbox.contains(hitbox.x + hitbox.columns - 1, hitbox.y));
            assert!(!hitbox.contains(hitbox.x - 1, hitbox.y));
            assert!(!hitbox.contains(hitbox.x + hitbox.columns, hitbox.y));
            assert!(!hitbox.contains(hitbox.x, hitbox.y - 1));
            assert!(!hitbox.contains(hitbox.x, hitbox.y + hitbox.rows));
        }

        let scrollbar = scrollbar_geometry(
            pane,
            Scrollbar {
                offset_rows: 50,
                total_rows: 100,
            },
        )
        .unwrap();
        for hitbox in [scrollbar.track, scrollbar.thumb] {
            assert!(hitbox.contains(hitbox.x, hitbox.y));
            assert!(hitbox.contains(hitbox.x + hitbox.columns - 1, hitbox.y + hitbox.rows - 1));
            assert!(!hitbox.contains(hitbox.x, hitbox.y - 1));
            assert!(!hitbox.contains(hitbox.x, hitbox.y + hitbox.rows));
            assert!(!hitbox.contains(hitbox.x - 1, hitbox.y));
            assert!(!hitbox.contains(hitbox.x + hitbox.columns, hitbox.y));
        }

        let divider = rect(2, 30, 8, 1);
        let dividers = [Some(divider), None, None];
        assert_eq!(
            divider_at(dividers, divider.x, divider.y),
            Some((0, divider))
        );
        assert_eq!(
            divider_at(dividers, divider.x, divider.y + divider.rows - 1),
            Some((0, divider))
        );
        assert!(divider_at(dividers, divider.x - 1, divider.y).is_none());
        assert!(divider_at(dividers, divider.x + 1, divider.y).is_none());
        assert!(divider_at(dividers, divider.x, divider.y - 1).is_none());
        assert!(divider_at(dividers, divider.x, divider.y + divider.rows).is_none());

        let horizontal = rect(12, 5, 1, 8);
        assert_eq!(
            divider_at(
                [Some(horizontal), None, None],
                horizontal.x + horizontal.columns - 1,
                horizontal.y
            ),
            Some((0, horizontal))
        );
        assert!(divider_at(
            [Some(horizontal), None, None],
            horizontal.x + horizontal.columns,
            horizontal.y
        )
        .is_none());
    }

    #[test]
    fn pointer_content_mapping_uses_inset_geometry_and_clamps_padding() {
        let pane = rect(9, 4, 24, 40);
        let content = content_rect(pane).unwrap();
        assert_eq!(content, rect(10, 5, 22, 38));
        assert_eq!(
            pointer_content_point(content, 10, 13),
            Point { row: 3, column: 5 }
        );
        assert_eq!(
            pointer_content_point(content, pane.x, pane.y),
            Point { row: 0, column: 0 }
        );
        assert_eq!(
            pointer_content_point(content, pane.x + pane.columns - 1, pane.y + pane.rows - 1),
            Point {
                row: content.rows - 1,
                column: content.columns - 1,
            }
        );
    }

    #[test]
    fn filled_header_padding_has_background_without_leaking_into_content() {
        for width in [22, 60] {
            let pane = snapshot_from("child", 5, width - 2);
            let mut frame = single_pane(&pane);
            frame.host_columns = width;
            frame.panes[0].as_mut().unwrap().rect.columns = width;
            for focused in [None, Some(0)] {
                frame.focused = focused;
                let bytes = fresh_render(&frame);
                let parser = screen(&bytes, &frame);
                let controls = pane_controls(frame.panes[0].unwrap().rect).unwrap();
                for (bounds, expected) in [
                    (controls.menu, vt100::Color::Idx(8)),
                    (controls.close, vt100::Color::Idx(1)),
                ] {
                    for x in bounds.x..bounds.x + bounds.columns {
                        let cell = parser.screen().cell(bounds.y, x).unwrap();
                        assert_eq!(cell.bgcolor(), expected);
                        assert!(!cell.bold());
                    }
                    println!("width={width} focused={focused:?} bounds={bounds:?} label={:?} background={expected:?}", label_at(&parser, bounds.y, bounds.x, bounds.columns));
                }
                assert_eq!(
                    parser.screen().cell(1, 1).unwrap().bgcolor(),
                    vt100::Color::Default
                );
                assert_eq!(
                    parser
                        .screen()
                        .cell(0, controls.focus_marker.x)
                        .unwrap()
                        .bgcolor(),
                    vt100::Color::Default
                );
            }
        }
    }

    #[test]
    fn focused_chrome_uses_ascii_marker_and_truncates_safe_wide_labels_before_controls() {
        let pane = snapshot_from("content", 5, 20);
        let mut frame = single_pane(&pane);
        frame.focused = Some(0);
        let chrome = PaneChrome {
            scrollbars: [None; crate::pane_layout::MAX_PANES],
            history: [None; crate::pane_layout::MAX_PANES],
            header_pressed: [None; crate::pane_layout::MAX_PANES],
            history_pressed: [false; crate::pane_layout::MAX_PANES],
            labels: [Some("界界界long"), None, None, None],
        };
        let mut compositor = Compositor::try_new().unwrap();
        let mut bytes = Vec::new();
        compositor
            .render_with_history_controls(&frame, None, None, None, chrome, &mut bytes)
            .unwrap();
        assert!(compositor.commit());

        let parser = screen(&bytes, &frame);
        let controls = pane_controls(frame.panes[0].unwrap().rect).unwrap();
        assert_eq!(
            cell_text(&parser, controls.focus_marker.y, controls.focus_marker.x),
            ">"
        );
        assert_eq!(
            label_at(
                &parser,
                controls.label.y,
                controls.label.x,
                controls.label.columns
            ),
            "界界界~"
        );
        assert_eq!(
            label_at(&parser, controls.menu.y, controls.menu.x, 6),
            " Menu "
        );
        assert_eq!(
            label_at(&parser, controls.close.y, controls.close.x, 3),
            " x "
        );
        assert_eq!(
            controls.label.x + controls.label.columns + 1,
            controls.close.x
        );
        let marker = parser
            .screen()
            .cell(controls.focus_marker.y, controls.focus_marker.x)
            .unwrap();
        assert!(marker.bold());
        assert_ne!(marker.fgcolor(), vt100::Color::Default);
        assert!(!marker.inverse());
        let rail = parser
            .screen()
            .cell(1, frame.panes[0].unwrap().rect.x)
            .unwrap();
        assert!(rail.bold());
        assert_ne!(rail.bgcolor(), vt100::Color::Default);
        assert!(!rail.inverse());
    }

    #[test]
    fn focus_changes_only_repaint_chrome_and_preserve_child_cells() {
        let pane = snapshot_from("child text", 5, 20);
        let unfocused = single_pane(&pane);
        let focused = ComposeFrame {
            focused: Some(0),
            ..unfocused
        };
        let mut compositor = Compositor::try_new().unwrap();
        let initial = rendered_committed(&mut compositor, &unfocused);
        let focused_frame = rendered_committed(&mut compositor, &focused);
        assert!(!contains(&focused_frame, b"\x1b[2J"));
        assert!(!focused_frame.is_empty());
        let mut stream = initial.clone();
        stream.extend_from_slice(&focused_frame);

        let before = screen(&initial, &unfocused);
        let after = screen(&stream, &focused);
        for row in 1..=5 {
            for column in 1..=20 {
                let old = before.screen().cell(row, column).unwrap();
                let new = after.screen().cell(row, column).unwrap();
                assert_eq!(old.contents(), new.contents(), "cell {row},{column}");
                assert_eq!(old.fgcolor(), new.fgcolor(), "cell {row},{column}");
                assert_eq!(old.bgcolor(), new.bgcolor(), "cell {row},{column}");
                assert_eq!(old.bold(), new.bold(), "cell {row},{column}");
                assert_eq!(old.dim(), new.dim(), "cell {row},{column}");
                assert_eq!(old.italic(), new.italic(), "cell {row},{column}");
                assert_eq!(old.underline(), new.underline(), "cell {row},{column}");
                assert_eq!(old.inverse(), new.inverse(), "cell {row},{column}");
            }
        }
        let controls = pane_controls(focused.panes[0].unwrap().rect).unwrap();
        assert_eq!(
            cell_text(&after, controls.focus_marker.y, controls.focus_marker.x),
            ">"
        );
        assert_blank(&after, 1, 0);
        let rail = after.screen().cell(1, 0).unwrap();
        assert!(rail.bold());
        assert_ne!(rail.bgcolor(), vt100::Color::Default);
        assert!(!rail.inverse());
        let menu = after
            .screen()
            .cell(controls.menu.y, controls.menu.x)
            .unwrap();
        assert!(!menu.bold());
        assert_ne!(menu.fgcolor(), vt100::Color::Default);
        assert!(!menu.inverse());
        let inactive_menu = before
            .screen()
            .cell(controls.menu.y, controls.menu.x)
            .unwrap();
        assert!(!inactive_menu.bold());
        assert_eq!(inactive_menu.fgcolor(), menu.fgcolor());
        assert_eq!(inactive_menu.bgcolor(), menu.bgcolor());
        assert_eq!(menu.bgcolor(), vt100::Color::Idx(8));
        assert!(!inactive_menu.inverse());
    }
    #[test]
    fn captured_chrome_damage_overlay_and_partial_write_commit() {
        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::other("injected"));
                }
                let count = bytes.len().min(self.0);
                self.0 -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let pane = snapshot_from("child text", 5, 20);
        let frame = single_pane(&pane);
        let controls = pane_controls(frame.panes[0].unwrap().rect).unwrap();
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = rendered_committed(&mut compositor, &frame);
        let mut chrome = PaneChrome::empty();
        chrome.header_pressed[0] = Some(false);
        for limit in [0, 12] {
            assert!(compositor
                .render_with_history_controls(
                    &frame,
                    None,
                    None,
                    None,
                    chrome,
                    &mut FailAfter(limit)
                )
                .is_err());
            assert!(!compositor.commit());
        }
        let mut bytes = Vec::new();
        compositor
            .render_with_history_controls(&frame, None, None, None, chrome, &mut bytes)
            .unwrap();
        assert!(!contains(&bytes, b"child text"));
        assert!(compositor.commit());
        stream.extend(bytes);
        let parser = screen(&stream, &frame);
        let mut evidence = String::new();
        for col in controls.menu.x..controls.menu.x + controls.menu.columns {
            let cell = parser.screen().cell(controls.menu.y, col).unwrap();
            assert_eq!(cell.bgcolor(), vt100::Color::Idx(4));
            evidence.push_str(&format!(
                "{},{} {:?} fg={:?} bg={:?}\n",
                controls.menu.y,
                col,
                cell.contents(),
                cell.fgcolor(),
                cell.bgcolor()
            ));
        }
        println!("{evidence}");
        let mut noop = Vec::new();
        compositor
            .render_with_history_controls(&frame, None, None, None, chrome, &mut noop)
            .unwrap();
        assert!(noop.is_empty());
        assert!(compositor.commit());
        let lines = [OverlayLine {
            text: "MODAL",
            style: OverlayStyle::Selected,
        }];
        let covered = ComposeFrame {
            overlay: Some(OverlayView {
                controls: &[],
                rect: controls.menu,
                lines: &lines,
            }),
            ..frame
        };
        let mut bytes = Vec::new();
        compositor
            .render_with_history_controls(
                &covered,
                None,
                None,
                None,
                PaneChrome::empty(),
                &mut bytes,
            )
            .unwrap();
        assert!(compositor.commit());
        stream.extend(bytes);
        assert_eq!(
            label_at(
                &screen(&stream, &covered),
                controls.menu.y,
                controls.menu.x,
                5
            ),
            "MODAL"
        );
        stream.extend(rendered_committed(&mut compositor, &frame));
        let parser = screen(&stream, &frame);
        for col in controls.menu.x..controls.menu.x + controls.menu.columns {
            assert_eq!(
                parser
                    .screen()
                    .cell(controls.menu.y, col)
                    .unwrap()
                    .bgcolor(),
                vt100::Color::Idx(8)
            );
        }
    }
    #[test]
    fn overlay_controls_fill_exact_spans_and_style_only_changes_repaint() {
        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::other("injected"));
                }
                let count = bytes.len().min(self.0);
                self.0 -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for reduced in [false, true] {
            let pane = snapshot_from("child text", 5, 20);
            let base = single_pane(&pane);
            let lines = [OverlayLine {
                text: "[Yes] gap[Cancel] status",
                style: OverlayStyle::Normal,
            }];
            let mut compositor = Compositor::try_new().unwrap();
            compositor.set_reduced_control_color(reduced);
            let mut stream = Vec::new();
            for state in [
                ControlState::Normal,
                ControlState::Focus,
                ControlState::Pressed,
                ControlState::Disabled,
                ControlState::Pending,
            ] {
                let controls = [
                    OverlayControl {
                        bounds: Rect {
                            x: 1,
                            y: 2,
                            rows: 1,
                            columns: 5,
                        },
                        role: ControlRole::Destructive,
                        state,
                    },
                    OverlayControl {
                        bounds: Rect {
                            x: 10,
                            y: 2,
                            rows: 1,
                            columns: 8,
                        },
                        role: ControlRole::Secondary,
                        state: ControlState::Normal,
                    },
                ];
                let mut frame = base;
                frame.overlay = Some(OverlayView {
                    rect: Rect {
                        x: 1,
                        y: 2,
                        rows: 1,
                        columns: 20,
                    },
                    lines: &lines,
                    controls: &controls,
                });
                let bytes = rendered_committed(&mut compositor, &frame);
                assert!(!bytes.is_empty());
                stream.extend(bytes);
                let parser = screen(&stream, &frame);
                let paint = style(ControlRole::Destructive, state, reduced);
                for x in 1..6 {
                    assert_eq!(
                        parser.screen().cell(2, x).unwrap().bgcolor(),
                        if reduced {
                            vt100::Color::Default
                        } else {
                            vt100::Color::Idx(if paint.background >= 100 {
                                paint.background - 100 + 8
                            } else {
                                paint.background - 40
                            })
                        }
                    );
                }
                for x in 6..10 {
                    assert_eq!(
                        parser.screen().cell(2, x).unwrap().bgcolor(),
                        vt100::Color::Default
                    );
                }
                for x in 10..18 {
                    assert_eq!(
                        parser.screen().cell(2, x).unwrap().bgcolor(),
                        if reduced {
                            vt100::Color::Default
                        } else {
                            vt100::Color::Idx(8)
                        }
                    );
                }
                for x in 18..21 {
                    assert_eq!(
                        parser.screen().cell(2, x).unwrap().bgcolor(),
                        vt100::Color::Default
                    );
                }
                println!("BTN03 {state:?}: {}", parser.screen().contents());
                println!(
                    "cells: {:?}",
                    (0..21)
                        .map(|x| (
                            x,
                            parser.screen().cell(2, x).unwrap().contents(),
                            parser.screen().cell(2, x).unwrap().bgcolor()
                        ))
                        .collect::<Vec<_>>()
                );
                assert!(rendered_committed(&mut compositor, &frame).is_empty());
                let mut moved = frame;
                moved.overlay.as_mut().unwrap().rect.y += 1;
                for limit in [0, 12] {
                    assert!(compositor.render(&moved, &mut FailAfter(limit)).is_err());
                    assert!(!compositor.commit(), "partial writes cannot commit spans");
                }
                // Retry the original frame after invalidation, then remove the overlay.
                stream.extend(rendered_committed(&mut compositor, &frame));
            }
            stream.extend(rendered_committed(&mut compositor, &base));
            let restored = screen(&stream, &base);
            let mut fresh_compositor = Compositor::try_new().unwrap();
            fresh_compositor.set_reduced_control_color(reduced);
            let fresh = screen(&rendered_committed(&mut fresh_compositor, &base), &base);
            assert_eq!(restored.screen().contents(), fresh.screen().contents());
            for y in 0..base.host_rows {
                for x in 0..base.host_columns {
                    assert_eq!(
                        restored.screen().cell(y, x).unwrap().bgcolor(),
                        fresh.screen().cell(y, x).unwrap().bgcolor()
                    );
                    assert_eq!(restored.screen().cell(y, x), fresh.screen().cell(y, x));
                }
            }
        }
    }
    #[test]
    fn overlay_control_admission_rejects_truncation_and_invalid_rows_before_output() {
        let pane = snapshot_from("child", 5, 20);
        let base = single_pane(&pane);
        let control = OverlayControl {
            bounds: rect(0, 1, 1, 3),
            role: ControlRole::Secondary,
            state: ControlState::Normal,
        };
        for controls in [
            vec![control; MAX_OVERLAY_CONTROLS + 1],
            vec![OverlayControl {
                bounds: rect(0, 1, 2, 3),
                ..control
            }],
            vec![OverlayControl {
                bounds: rect(0, u16::MAX, 1, 3),
                ..control
            }],
        ] {
            let frame = ComposeFrame {
                overlay: Some(OverlayView {
                    rect: rect(0, 0, 3, 20),
                    lines: &[],
                    controls: &controls,
                }),
                ..base
            };
            let mut compositor = Compositor::try_new().unwrap();
            let mut bytes = Vec::new();
            assert!(compositor.render(&frame, &mut bytes).is_err());
            assert!(bytes.is_empty());
        }
    }
    #[test]
    fn btn05_modes_cells_and_successful_baseline() {
        let pane = snapshot_from("child", 5, 20);
        let base = single_pane(&pane);
        let lines = [OverlayLine {
            text: "[Run] gap status",
            style: OverlayStyle::Normal,
        }];
        for initial in [b"\x1b[30;47m".as_slice(), b"\x1b[37;40m".as_slice()] {
            let mut compositor = Compositor::try_new().unwrap();
            let mut stream = initial.to_vec();
            for reduced in [false, true, false] {
                compositor.set_reduced_control_color(reduced);
                assert!(!compositor.commit());
                for role in [
                    ControlRole::Secondary,
                    ControlRole::Primary,
                    ControlRole::Destructive,
                ] {
                    for state in [
                        ControlState::Normal,
                        ControlState::Focus,
                        ControlState::Pressed,
                        ControlState::Disabled,
                        ControlState::Pending,
                    ] {
                        let controls = [OverlayControl {
                            bounds: rect(2, 1, 1, 5),
                            role,
                            state,
                        }];
                        let frame = ComposeFrame {
                            overlay: Some(OverlayView {
                                rect: rect(2, 1, 1, 18),
                                lines: &lines,
                                controls: &controls,
                            }),
                            ..base
                        };
                        stream.extend(rendered_committed(&mut compositor, &frame));
                        let parser = screen(&stream, &frame);
                        let expected = style(role, state, reduced);
                        for x in 1..6 {
                            let cell = parser.screen().cell(2, x).unwrap();
                            assert_eq!(cell.inverse(), reduced);
                            assert_eq!(
                                cell.bold(),
                                expected.attributes.split(';').any(|a| a == "1")
                            );
                            assert_eq!(
                                cell.italic(),
                                expected.attributes.split(';').any(|a| a == "3")
                            );
                            assert_eq!(
                                cell.underline(),
                                expected.attributes.split(';').any(|a| a == "4")
                            );
                            assert_eq!(
                                cell.bgcolor(),
                                if reduced {
                                    vt100::Color::Default
                                } else {
                                    vt100::Color::Idx(if expected.background >= 100 {
                                        expected.background - 100 + 8
                                    } else {
                                        expected.background - 40
                                    })
                                }
                            );
                        }
                        for x in 6..19 {
                            let cell = parser.screen().cell(2, x).unwrap();
                            assert!(!cell.inverse());
                            assert!(!cell.bold());
                            assert!(!cell.underline());
                            assert_eq!(cell.bgcolor(), vt100::Color::Default);
                        }
                        println!("BTN05 initial={initial:?} reduced={reduced} {role:?}/{state:?} attrs={} cells={}", expected.attributes, parser.screen().contents());
                        assert!(rendered_committed(&mut compositor, &frame).is_empty());
                        compositor.set_reduced_control_color(reduced);
                        assert!(rendered_committed(&mut compositor, &frame).is_empty());
                    }
                }
            }
        }
    }
    #[test]
    fn btn05_header_history_and_mode_failure_repaint() {
        struct Fail;
        impl Write for Fail {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let pane = snapshot_from("child", 5, 40);
        let mut frame = single_pane(&pane);
        frame.host_rows += 1;
        frame.panes[0].as_mut().unwrap().rect.y += 1;
        let bounds = frame.panes[0].unwrap().rect;
        let mut header = vec![HeaderCell::blank(false); usize::from(frame.host_columns)];
        header[0] = HeaderCell::glyph("T", true).unwrap().with_filled();
        let mut chrome = PaneChrome::empty();
        chrome.history[0] = Some(HistoryLabelKind::History);
        let controls = pane_controls(bounds).unwrap();
        let history = pane_history_controls(
            bounds,
            HistoryLabelKind::History,
            pane_version_badge(bounds, frame.host_rows, frame.host_columns),
        )
        .unwrap();
        let mut compositor = Compositor::try_new().unwrap();
        let mut stream = Vec::new();
        for reduced in [false, true, false] {
            compositor.set_reduced_control_color(reduced);
            assert!(compositor
                .render_with_history_controls(&frame, None, None, Some(&header), chrome, &mut Fail)
                .is_err());
            assert!(!compositor.commit());
            for pressed in [false, true] {
                chrome.header_pressed[0] = pressed.then_some(false);
                chrome.history_pressed[0] = pressed;
                compositor
                    .render_with_history_controls(
                        &frame,
                        None,
                        None,
                        Some(&header),
                        chrome,
                        &mut stream,
                    )
                    .unwrap();
                assert!(compositor.commit());
                let parser = screen(&stream, &frame);
                for (y, x) in [
                    (0, 0),
                    (controls.menu.y, controls.menu.x),
                    (history.back.y, history.back.x),
                ] {
                    let cell = parser.screen().cell(y, x).unwrap();
                    assert_eq!(cell.inverse(), reduced);
                    assert_eq!(cell.bold(), reduced && (y == 0 || pressed));
                    println!("BTN05 chrome reduced={reduced} pressed={pressed} ({y},{x}) text={} fg={:?} bg={:?} inverse={} bold={} underline={}",cell.contents(),cell.fgcolor(),cell.bgcolor(),cell.inverse(),cell.bold(),cell.underline());
                }
                let mut noop = Vec::new();
                compositor
                    .render_with_history_controls(
                        &frame,
                        None,
                        None,
                        Some(&header),
                        chrome,
                        &mut noop,
                    )
                    .unwrap();
                assert!(noop.is_empty());
                assert!(compositor.commit());
            }
        }
    }
}
