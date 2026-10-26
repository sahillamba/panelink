//! Captured, data-redacted requests for server-owned pane operations.

use panelink::control_protocol::TargetRef;
use panelink::pane_operations_protocol::PaneOperationsView;
use panelink::pane_operations_protocol::{PaneDividerSet, PaneSwap, PaneZoomSet, Request};
use panelink::protocol::{OperationKey, Outcome};
use std::fmt;

/// One absolute pane operation captured from the current view.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum PaneAction {
    Swap {
        source: TargetRef,
        destination: TargetRef,
        expected_layout_revision: u64,
    },
    DividerSet {
        target: TargetRef,
        expected_layout_revision: u64,
        divider_index: u8,
        first_cells: u16,
    },
    ZoomSet {
        target: TargetRef,
        expected_layout_revision: u64,
        zoomed: bool,
    },
}

/// Validated authoritative view paired with a terminal applied/partial result.
pub(crate) struct PaneOperationUpdate {
    pub(crate) view: PaneOperationsView,
    pub(crate) outcome: Outcome,
}

impl PaneAction {
    /// Target whose live connection carries this operation.
    pub(crate) const fn captured_target(self) -> TargetRef {
        match self {
            Self::Swap { source, .. } => source,
            Self::DividerSet { target, .. } | Self::ZoomSet { target, .. } => target,
        }
    }

    /// Build the exact schema-13 mutation using the key reserved by `PaneConn`.
    pub(crate) const fn request(self, key: OperationKey) -> Request {
        match self {
            Self::Swap {
                source,
                destination,
                expected_layout_revision,
            } => Request::Swap(PaneSwap {
                key,
                expected_layout_revision,
                source,
                destination,
            }),
            Self::DividerSet {
                target,
                expected_layout_revision,
                divider_index,
                first_cells,
            } => Request::DividerSet(PaneDividerSet {
                key,
                expected_layout_revision,
                target,
                divider_index,
                first_cells,
            }),
            Self::ZoomSet {
                target,
                expected_layout_revision,
                zoomed,
            } => Request::ZoomSet(PaneZoomSet {
                key,
                expected_layout_revision,
                target,
                zoomed,
            }),
        }
    }
}

impl fmt::Debug for PaneAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Swap {
                expected_layout_revision,
                ..
            } => f
                .debug_struct("Swap")
                .field("targets", &"<redacted>")
                .field("expected_layout_revision", expected_layout_revision)
                .finish(),
            Self::DividerSet {
                expected_layout_revision,
                divider_index,
                first_cells,
                ..
            } => f
                .debug_struct("DividerSet")
                .field("target", &"<redacted>")
                .field("expected_layout_revision", expected_layout_revision)
                .field("divider_index", divider_index)
                .field("first_cells", first_cells)
                .finish(),
            Self::ZoomSet {
                expected_layout_revision,
                zoomed,
                ..
            } => f
                .debug_struct("ZoomSet")
                .field("target", &"<redacted>")
                .field("expected_layout_revision", expected_layout_revision)
                .field("zoomed", zoomed)
                .finish(),
        }
    }
}
