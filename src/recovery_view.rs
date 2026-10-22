//! Pure historical recovery-view model and bounded host geometry.
//!
//! Historical targets are display labels only; this module has no access to
//! storage, a PTY, a server, commands, or provider state.

use crate::control_protocol::TargetRef;
use crate::history_rows;
use crate::pane_layout::{Rect, MAX_PANE_COLUMNS, MAX_PANE_ROWS, MIN_PANE_COLUMNS, MIN_PANE_ROWS};
use crate::saved_layout::{LayoutAxis, LayoutMetadata, SavedLayout};
use crate::screen_checkpoint::ScreenBuffer;
use std::fmt;

/// Requested immutable layout slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoverySlot {
    /// Most recently saved layout.
    Current,
    /// Immediately preceding saved generation.
    Previous,
}

/// Availability of a source belonging to one exact historical target.
#[derive(Clone, Eq, PartialEq)]
pub enum HistoryAvailability {
    /// The saved bounded scrollback rows and continuity metadata.
    Available {
        rows: Vec<HistoryRow>,
        epoch: u64,
        gap: bool,
        reset: ResetStatus,
        capture_disabled: bool,
        ended_at_unix_ms: u64,
    },
    /// No committed rows exist for this exact target.
    Missing,
    /// The committed row file could not be validated as a whole.
    Invalid,
}

impl fmt::Debug for HistoryAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available {
                rows,
                epoch,
                gap,
                reset,
                capture_disabled,
                ended_at_unix_ms,
            } => f
                .debug_struct("HistoryAvailability::Available")
                .field("row_count", &rows.len())
                .field("epoch", epoch)
                .field("gap", gap)
                .field("reset", reset)
                .field("capture_disabled", capture_disabled)
                .field("ended_at_unix_ms", ended_at_unix_ms)
                .field("text", &"<redacted>")
                .finish(),
            Self::Missing => f.write_str("HistoryAvailability::Missing"),
            Self::Invalid => f.write_str("HistoryAvailability::Invalid"),
        }
    }
}

/// What the persisted history discontinuity bit proves about terminal reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetStatus {
    /// The committed format does not indicate a reset or other discontinuity.
    NotIndicated,
    /// A gap is recorded; reset is one possible cause, but the format does not
    /// distinguish it from skipped or evicted rows.
    PossibleFromGap,
}

/// One sanitized retained history row.
#[derive(Clone, Eq, PartialEq)]
pub struct HistoryRow {
    /// Source capture time in Unix milliseconds.
    pub captured_at_unix_ms: u64,
    /// One-based ordinal within its engine epoch.
    pub ordinal: u64,
    /// Number of terminal cells represented by the row.
    pub width: u16,
    /// Whether the row continues a wrapped logical line.
    pub wrapped: bool,
    pub(crate) record: Vec<u8>,
}

impl HistoryRow {
    /// Borrow one sanitized cell string at its stored column.
    pub fn cell_text(&self, column: u16) -> Option<String> {
        let decoded = history_rows::decode(&self.record).ok()?;
        Some(decoded.cell_text(column)?.to_owned())
    }
}

impl fmt::Debug for HistoryRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryRow")
            .field("captured_at_unix_ms", &self.captured_at_unix_ms)
            .field("ordinal", &self.ordinal)
            .field("width", &self.width)
            .field("wrapped", &self.wrapped)
            .finish()
    }
}

/// A validated screen checkpoint's metadata (cell contents remain in storage
/// until a future presentation layer explicitly decodes it).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointInfo {
    /// Buffer represented by this checkpoint.
    pub buffer: ScreenBuffer,
    /// Captured child-output sequence.
    pub output_sequence: u64,
    /// Validated screen revision within this target.
    pub screen_revision: u64,
    /// Source capture time in Unix milliseconds.
    pub captured_at_unix_ms: u64,
    /// Captured cell dimensions.
    pub rows: u16,
    /// Captured cell dimensions.
    pub columns: u16,
}

/// Availability of one independent normal or alternate screen source.
#[derive(Clone, Eq, PartialEq)]
pub enum CheckpointAvailability {
    /// A validated checkpoint for the exact target and requested buffer.
    Available {
        /// Validated non-content checkpoint metadata.
        info: CheckpointInfo,
        /// Complete normalized checkpoint record. Use
        /// [`CheckpointAvailability::checkpoint`] for a validated borrowed view;
        /// never write these bytes to a terminal.
        record: Vec<u8>,
    },
    /// No committed checkpoint exists for this exact target and buffer.
    Missing,
    /// The committed checkpoint could not be validated as a whole.
    Invalid,
}

impl fmt::Debug for CheckpointAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available { info, record } => f
                .debug_struct("CheckpointAvailability::Available")
                .field("info", info)
                .field("record_bytes", &record.len())
                .field("record", &"<redacted>")
                .finish(),
            Self::Missing => f.write_str("CheckpointAvailability::Missing"),
            Self::Invalid => f.write_str("CheckpointAvailability::Invalid"),
        }
    }
}

impl CheckpointAvailability {
    /// Borrow the fully validated normalized checkpoint, if available.
    pub fn checkpoint(&self) -> Option<crate::screen_checkpoint::ScreenCheckpoint<'_>> {
        match self {
            Self::Available { record, .. } => crate::screen_checkpoint::decode(record).ok(),
            Self::Missing | Self::Invalid => None,
        }
    }

    /// Metadata for an available checkpoint, if present.
    pub const fn info(&self) -> Option<&CheckpointInfo> {
        match self {
            Self::Available { info, .. } => Some(info),
            Self::Missing | Self::Invalid => None,
        }
    }
}

/// One pane in the historical spatial order. Its target is for display only.
#[derive(Clone, Eq, PartialEq)]
pub struct RecoveryPane {
    /// Exact identity from the historical layout; never a live dispatch target.
    pub historical_target: TargetRef,
    /// Independently loaded bounded scrollback source.
    pub rows: HistoryAvailability,
    /// Independently loaded normal screen checkpoint.
    pub normal: CheckpointAvailability,
    /// Independently loaded alternate screen checkpoint.
    pub alternate: CheckpointAvailability,
}

impl fmt::Debug for RecoveryPane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryPane")
            .field("historical_target", &"<redacted>")
            .field("rows", &self.rows)
            .field("normal", &self.normal)
            .field("alternate", &self.alternate)
            .finish()
    }
}

/// Historical layout plus exact-target recovery data.
#[derive(Clone, Eq, PartialEq)]
pub struct RecoveryView {
    /// The explicitly selected current or previous slot.
    pub slot: RecoverySlot,
    /// Validated saved-layout metadata.
    pub metadata: LayoutMetadata,
    /// Historical root dimensions, not a live PTY resize request.
    pub saved_rows: u16,
    /// Historical root dimensions, not a live PTY resize request.
    pub saved_columns: u16,
    /// Historical spatial split orientation.
    pub axis: LayoutAxis,
    /// Panes in validated saved spatial order; empty for a tombstone.
    pub panes: Vec<RecoveryPane>,
}

impl fmt::Debug for RecoveryView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryView")
            .field("slot", &self.slot)
            .field("metadata", &self.metadata)
            .field("saved_rows", &self.saved_rows)
            .field("saved_columns", &self.saved_columns)
            .field("axis", &self.axis)
            .field("pane_count", &self.panes.len())
            .field("panes", &"<redacted>")
            .finish()
    }
}

impl RecoveryView {
    /// Build an empty-data model from a fully validated layout.
    pub fn from_layout(layout: SavedLayout) -> Self {
        let panes = layout
            .geometry()
            .into_iter()
            .flatten()
            .map(|geometry| RecoveryPane {
                historical_target: geometry.target,
                rows: HistoryAvailability::Missing,
                normal: CheckpointAvailability::Missing,
                alternate: CheckpointAvailability::Missing,
            })
            .collect();
        Self {
            slot: RecoverySlot::Current,
            metadata: layout.metadata(),
            saved_rows: layout.rows(),
            saved_columns: layout.columns(),
            axis: layout.axis(),
            panes,
        }
    }

    /// Associate this view with the explicitly requested historical slot.
    pub fn in_slot(mut self, slot: RecoverySlot) -> Self {
        self.slot = slot;
        self
    }
}

/// Safe label for a synthetic topology outline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverviewLabel {
    /// First historical spatial pane.
    PaneOne,
    /// Second historical spatial pane.
    PaneTwo,
    /// No pane in an empty tombstone.
    Empty,
}

/// One proportional box in a compact scaled topology outline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverviewBox {
    /// Bounded viewport-relative rectangle.
    pub rect: Rect,
    /// Fixed label only; no pane data or stored content is included.
    pub label: OverviewLabel,
}

/// Non-dispatching viewport plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisplayPlan {
    /// Regular display geometry fits every saved pane within supported limits.
    Panes {
        /// Content area after chrome.
        viewport: Rect,
        /// Zero, one, or two bounded pane rectangles in historical order.
        panes: Vec<Rect>,
        /// One-cell divider when two panes are displayed.
        divider: Option<Rect>,
    },
    /// The regular leaf sizes do not fit; draw only this scaled topology.
    Overview {
        /// Bounded host content viewport.
        viewport: Rect,
        /// Proportional boxes with fixed labels and no pane content.
        boxes: Vec<OverviewBox>,
        /// Divider coordinates in viewport-relative cells, when present.
        divider: Option<Rect>,
    },
    /// The host is below the minimum size for even a labeled outline (<3x10).
    TooSmall,
}

impl fmt::Display for RecoverySlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Current => "current",
            Self::Previous => "previous",
        })
    }
}

/// Plan bounded geometry for a terminal host. `rows` includes `chrome_rows`;
/// the resulting rectangles are always local to the post-chrome content area.
pub fn plan_viewport(
    view: &RecoveryView,
    rows: u16,
    columns: u16,
    chrome_rows: u16,
) -> DisplayPlan {
    let Some(content_rows) = rows.checked_sub(chrome_rows) else {
        return DisplayPlan::TooSmall;
    };
    let viewport = Rect {
        x: 0,
        y: 0,
        rows: content_rows.min(MAX_PANE_ROWS),
        columns: columns.min(MAX_PANE_COLUMNS),
    };

    if view.panes.is_empty() {
        return if regular_leaf_fits(viewport.rows, viewport.columns) {
            DisplayPlan::Panes {
                viewport,
                panes: Vec::new(),
                divider: None,
            }
        } else {
            overview(view.axis, 0, content_rows, columns)
        };
    }

    if view.panes.len() == 1 {
        let display_rows = viewport.rows;
        let display_columns = viewport.columns;
        if regular_leaf_fits(display_rows, display_columns) {
            return DisplayPlan::Panes {
                viewport,
                panes: vec![Rect {
                    x: 0,
                    y: 0,
                    rows: display_rows,
                    columns: display_columns,
                }],
                divider: None,
            };
        }
        return overview(LayoutAxis::Single, 1, content_rows, columns);
    }

    let pane_geometry = regular_split(viewport.rows, viewport.columns, view.axis);
    if let Some((panes, divider)) = pane_geometry {
        return DisplayPlan::Panes {
            viewport,
            panes,
            divider,
        };
    }
    overview(view.axis, view.panes.len(), viewport.rows, viewport.columns)
}

fn regular_leaf_fits(rows: u16, columns: u16) -> bool {
    rows >= MIN_PANE_ROWS && columns >= MIN_PANE_COLUMNS
}

fn regular_split(rows: u16, columns: u16, axis: LayoutAxis) -> Option<(Vec<Rect>, Option<Rect>)> {
    let (first, second, divider) = match axis {
        LayoutAxis::SideBySide => {
            let usable = columns.checked_sub(1)?;
            let first_columns = usable / 2;
            let second_columns = usable.checked_sub(first_columns)?;
            let divider_x = first_columns;
            (
                Rect {
                    x: 0,
                    y: 0,
                    rows,
                    columns: first_columns,
                },
                Rect {
                    x: divider_x.checked_add(1)?,
                    y: 0,
                    rows,
                    columns: second_columns,
                },
                Rect {
                    x: divider_x,
                    y: 0,
                    rows,
                    columns: 1,
                },
            )
        }
        LayoutAxis::Stacked => {
            let usable = rows.checked_sub(1)?;
            let first_rows = usable / 2;
            let second_rows = usable.checked_sub(first_rows)?;
            let divider_y = first_rows;
            (
                Rect {
                    x: 0,
                    y: 0,
                    rows: first_rows,
                    columns,
                },
                Rect {
                    x: 0,
                    y: divider_y.checked_add(1)?,
                    rows: second_rows,
                    columns,
                },
                Rect {
                    x: 0,
                    y: divider_y,
                    rows: 1,
                    columns,
                },
            )
        }
        LayoutAxis::Single => return None,
    };
    if [first, second].into_iter().all(|rect| {
        regular_leaf_fits(rect.rows, rect.columns)
            && rect.rows <= MAX_PANE_ROWS
            && rect.columns <= MAX_PANE_COLUMNS
    }) {
        Some((vec![first, second], Some(divider)))
    } else {
        None
    }
}

fn overview(axis: LayoutAxis, pane_count: usize, rows: u16, columns: u16) -> DisplayPlan {
    let rows = rows.min(MAX_PANE_ROWS);
    let columns = columns.min(MAX_PANE_COLUMNS);
    if rows < 3 || columns < 10 {
        return DisplayPlan::TooSmall;
    }
    let viewport = Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    };
    if pane_count == 0 {
        return DisplayPlan::Overview {
            viewport,
            boxes: vec![OverviewBox {
                rect: viewport,
                label: OverviewLabel::Empty,
            }],
            divider: None,
        };
    }

    let (first, second, divider) = match axis {
        LayoutAxis::SideBySide => {
            let Some(usable) = viewport.columns.checked_sub(1) else {
                return DisplayPlan::TooSmall;
            };
            let first_columns = proportional_half(usable);
            let divider_x = first_columns;
            let Some(second_columns) = usable.checked_sub(first_columns) else {
                return DisplayPlan::TooSmall;
            };
            let Some(second_x) = divider_x.checked_add(1) else {
                return DisplayPlan::TooSmall;
            };
            (
                Rect {
                    x: 0,
                    y: 0,
                    rows: viewport.rows,
                    columns: first_columns,
                },
                Rect {
                    x: second_x,
                    y: 0,
                    rows: viewport.rows,
                    columns: second_columns,
                },
                Some(Rect {
                    x: divider_x,
                    y: 0,
                    rows: viewport.rows,
                    columns: 1,
                }),
            )
        }
        LayoutAxis::Stacked => {
            let Some(usable) = viewport.rows.checked_sub(1) else {
                return DisplayPlan::TooSmall;
            };
            let first_rows = proportional_half(usable);
            let divider_y = first_rows;
            let Some(second_rows) = usable.checked_sub(first_rows) else {
                return DisplayPlan::TooSmall;
            };
            let Some(second_y) = divider_y.checked_add(1) else {
                return DisplayPlan::TooSmall;
            };
            (
                Rect {
                    x: 0,
                    y: 0,
                    rows: first_rows,
                    columns: viewport.columns,
                },
                Rect {
                    x: 0,
                    y: second_y,
                    rows: second_rows,
                    columns: viewport.columns,
                },
                Some(Rect {
                    x: 0,
                    y: divider_y,
                    rows: 1,
                    columns: viewport.columns,
                }),
            )
        }
        LayoutAxis::Single => (
            viewport,
            Rect {
                x: 0,
                y: 0,
                rows: 0,
                columns: 0,
            },
            None,
        ),
    };
    let mut boxes = vec![OverviewBox {
        rect: first,
        label: OverviewLabel::PaneOne,
    }];
    if pane_count > 1 {
        boxes.push(OverviewBox {
            rect: second,
            label: OverviewLabel::PaneTwo,
        });
    }
    DisplayPlan::Overview {
        viewport,
        boxes,
        divider,
    }
}

fn proportional_half(value: u16) -> u16 {
    value / 2
}
