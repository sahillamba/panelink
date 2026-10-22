//! Pure, fixed-capacity geometry for a bounded four-pane split tree.
//!
//! This module owns no terminal, process, persistence, or identifier-allocation
//! effects. Callers supply complete target references and decide when a
//! validated preview is committed.

use crate::control_protocol::TargetRef;
use std::error::Error;
use std::fmt;

/// Smallest supported terminal viewport height for one pane.
pub const MIN_PANE_ROWS: u16 = 5;
/// Largest supported terminal viewport height for one pane.
pub const MAX_PANE_ROWS: u16 = 80;
/// Smallest supported terminal viewport width for one pane.
pub const MIN_PANE_COLUMNS: u16 = 20;
/// Largest supported terminal viewport width for one pane.
pub const MAX_PANE_COLUMNS: u16 = 240;
/// Maximum number of pane leaves retained in one workspace layout.
pub const MAX_PANES: usize = 4;
const MAX_TREE_NODES: usize = MAX_PANES * 2 - 1;

/// Direction in which a new pane is placed relative to the selected pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitDirection {
    /// Place the new pane to the left of the selected pane.
    Left,
    /// Place the new pane to the right of the selected pane.
    Right,
    /// Place the new pane above the selected pane.
    Up,
    /// Place the new pane below the selected pane.
    Down,
}

/// A half-open rectangle measured in terminal cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    /// Leftmost cell coordinate.
    pub x: u16,
    /// Topmost cell coordinate.
    pub y: u16,
    /// Rectangle height in rows.
    pub rows: u16,
    /// Rectangle width in columns.
    pub columns: u16,
}

impl Rect {
    /// Returns whether the cell is inside this half-open rectangle.
    pub fn contains(self, x: u16, y: u16) -> bool {
        let Some(right) = self.x.checked_add(self.columns) else {
            return false;
        };
        let Some(bottom) = self.y.checked_add(self.rows) else {
            return false;
        };
        x >= self.x && x < right && y >= self.y && y < bottom
    }
}

/// Immutable association between a complete pane target and its cell rectangle.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneGeometry {
    /// Complete identity of the pane occupant.
    pub target: TargetRef,
    /// Pane viewport in the root content rectangle.
    pub rect: Rect,
}

impl fmt::Debug for PaneGeometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneGeometry")
            .field("target", &"[redacted]")
            .field("rect", &self.rect)
            .finish()
    }
}

/// Copyable read-only view of the current root, panes, and divider.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutSnapshot {
    /// Monotonically increasing layout revision.
    pub revision: u64,
    /// Accepted root height in rows.
    pub rows: u16,
    /// Accepted root width in columns.
    pub columns: u16,
    /// Spatially ordered panes; unused entries are `None`.
    pub panes: [Option<PaneGeometry>; MAX_PANES],
    /// First divider, retained for source compatibility with two-pane clients.
    pub divider: Option<Rect>,
    /// Spatially traversed one-cell dividers; unused entries are `None`.
    pub dividers: [Option<Rect>; MAX_PANES - 1],
}

impl LayoutSnapshot {
    /// Returns the number of occupied pane entries.
    pub fn pane_count(self) -> usize {
        self.panes.iter().filter(|pane| pane.is_some()).count()
    }
}

impl fmt::Debug for LayoutSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutSnapshot")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("pane_count", &self.pane_count())
            .field("divider", &self.divider)
            .finish_non_exhaustive()
    }
}

/// Fixed-capacity binary split-tree layout state with private mutation fields.
pub struct PaneLayout {
    state: LayoutState,
}

impl PaneLayout {
    /// Creates revision 1 with one pane filling a valid root rectangle.
    pub fn new(target: TargetRef, rows: u16, columns: u16) -> Result<Self, LayoutError> {
        validate_target(target)?;
        validate_root_dimensions(rows, columns)?;
        let root = root_rect(rows, columns);
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes: [Some(PaneGeometry { target, rect: root }), None, None, None],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        Ok(Self {
            state: LayoutState {
                snapshot,
                nodes: [
                    TreeNode::Leaf(target),
                    TreeNode::Empty,
                    TreeNode::Empty,
                    TreeNode::Empty,
                    TreeNode::Empty,
                    TreeNode::Empty,
                    TreeNode::Empty,
                ],
            },
        })
    }

    /// Rebuilds the deterministic split tree represented by one validated
    /// spatial snapshot. Dividers must form a complete recursive partition.
    pub fn from_snapshot(snapshot: LayoutSnapshot) -> Result<Self, LayoutError> {
        validate_root_dimensions(snapshot.rows, snapshot.columns)?;
        if snapshot.revision == 0 || snapshot.pane_count() == 0 {
            return Err(LayoutError::InvalidLayoutState);
        }
        let panes: Vec<_> = snapshot.panes.iter().flatten().copied().collect();
        let dividers = if snapshot.dividers.iter().any(Option::is_some) {
            snapshot.dividers
        } else {
            let mut dividers = [None; MAX_PANES - 1];
            dividers[0] = snapshot.divider;
            dividers
        };
        let mut nodes = [TreeNode::Empty; MAX_TREE_NODES];
        let mut next = 1;
        let mut visited = [false; MAX_PANES - 1];
        build_tree_from_rects(
            root_rect(snapshot.rows, snapshot.columns),
            &panes,
            &dividers,
            &mut nodes,
            0,
            &mut next,
            &mut visited,
        )?;
        if dividers
            .iter()
            .enumerate()
            .any(|(index, divider)| divider.is_some() != visited[index])
        {
            return Err(LayoutError::InvalidLayoutState);
        }
        let rebuilt = state_from_tree(nodes, snapshot.rows, snapshot.columns, snapshot.revision)?;
        if rebuilt.snapshot.panes != snapshot.panes
            || rebuilt.snapshot.dividers != dividers
            || rebuilt.snapshot.divider != snapshot.divider
        {
            return Err(LayoutError::InvalidLayoutState);
        }
        Ok(Self { state: rebuilt })
    }

    /// Returns a copy of the current immutable layout snapshot.
    pub fn snapshot(&self) -> LayoutSnapshot {
        self.state.snapshot
    }

    /// Checks whether this exact target can be split in the requested direction.
    pub fn can_split(
        &self,
        target: TargetRef,
        direction: SplitDirection,
    ) -> Result<(), LayoutError> {
        if !self.contains_target(target) {
            return Err(LayoutError::StaleTarget);
        }
        if self.state.snapshot.pane_count() >= MAX_PANES {
            return Err(LayoutError::PaneLimitExceeded);
        }
        let pane = self
            .state
            .snapshot
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == target)
            .ok_or(LayoutError::StaleTarget)?;
        split_rectangles(pane.rect, direction)?;
        self.state
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(LayoutError::RevisionExhausted)?;
        Ok(())
    }

    /// Builds an opaque split preview without changing this layout.
    pub fn preview_split(
        &self,
        target: TargetRef,
        expected_revision: u64,
        direction: SplitDirection,
        new_target: TargetRef,
    ) -> Result<SplitPreview, LayoutError> {
        if !self.contains_target(target) {
            return Err(LayoutError::StaleTarget);
        }
        if expected_revision != self.state.snapshot.revision {
            return Err(LayoutError::StaleRevision);
        }
        if self.state.snapshot.pane_count() >= MAX_PANES {
            return Err(LayoutError::PaneLimitExceeded);
        }
        validate_target(new_target)?;
        let existing_panes = self.state.snapshot.panes;
        if existing_panes.iter().flatten().any(|pane| {
            pane.target.pane == new_target.pane || pane.target.session == new_target.session
        }) {
            return Err(LayoutError::TargetIdentityConflict);
        }
        let existing = self
            .state
            .snapshot
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == target)
            .copied()
            .ok_or(LayoutError::StaleTarget)?;
        if new_target.generation != existing.target.generation
            || new_target.workspace != existing.target.workspace
        {
            return Err(LayoutError::TargetScopeMismatch);
        }
        let (first_rect, _, _) = split_rectangles_for_direction(existing.rect, direction)?;
        let nodes = self.state.nodes;
        let leaf_index = nodes
            .iter()
            .position(|node| matches!(node, TreeNode::Leaf(found) if *found == target))
            .ok_or(LayoutError::InvalidLayoutState)?;
        let mut free = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| matches!(node, TreeNode::Empty).then_some(index));
        let first_index = free.next().ok_or(LayoutError::InvalidLayoutState)?;
        let second_index = free.next().ok_or(LayoutError::InvalidLayoutState)?;
        let new_is_first = matches!(direction, SplitDirection::Left | SplitDirection::Up);
        let mut next_nodes = nodes;
        next_nodes[first_index] = TreeNode::Leaf(if new_is_first { new_target } else { target });
        next_nodes[second_index] = TreeNode::Leaf(if new_is_first { target } else { new_target });
        let horizontal = matches!(direction, SplitDirection::Left | SplitDirection::Right);
        let first_cells = if horizontal {
            first_rect.columns
        } else {
            first_rect.rows
        };
        let total_cells = if horizontal {
            existing.rect.columns - 1
        } else {
            existing.rect.rows - 1
        };
        next_nodes[leaf_index] = TreeNode::Split {
            first: first_index,
            second: second_index,
            horizontal,
            first_cells,
            total_cells,
        };
        let next_revision = self
            .state
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(LayoutError::RevisionExhausted)?;
        let after = state_from_tree(
            next_nodes,
            self.state.snapshot.rows,
            self.state.snapshot.columns,
            next_revision,
        )?;

        Ok(SplitPreview {
            before: self.state,
            after,
        })
    }

    /// Commits a preview only when its complete before-state still matches.
    pub fn commit_split(&mut self, preview: SplitPreview) -> Result<LayoutSnapshot, LayoutError> {
        if self.state != preview.before {
            return Err(LayoutError::PreviewStale);
        }
        self.state = preview.after;
        Ok(self.state.snapshot)
    }

    /// Resizes the whole root while preserving identities and split ordering.
    pub fn resize(
        &mut self,
        expected_revision: u64,
        rows: u16,
        columns: u16,
    ) -> Result<LayoutSnapshot, LayoutError> {
        if expected_revision != self.state.snapshot.revision {
            return Err(LayoutError::StaleRevision);
        }
        validate_root_dimensions(rows, columns)?;
        if rows == self.state.snapshot.rows && columns == self.state.snapshot.columns {
            return Ok(self.state.snapshot);
        }

        let candidate = resized_state(self.state, rows, columns)?;
        let next_revision = self
            .state
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(LayoutError::RevisionExhausted)?;
        let mut candidate = candidate;
        candidate.snapshot.revision = next_revision;
        self.state = candidate;
        Ok(self.state.snapshot)
    }

    /// Removes any pane; its sibling subtree absorbs the removed split region.
    ///
    /// The removed target must exactly identify one current pane. The sibling
    /// subtree keeps its identities and the revision advances by one. A
    /// single-pane layout cannot collapse and
    /// returns `InvalidLayoutState`. Every rejection leaves the layout
    /// unchanged. The survivor may then be split again with a fresh target.
    pub fn collapse(
        &mut self,
        removed: TargetRef,
        expected_revision: u64,
    ) -> Result<LayoutSnapshot, LayoutError> {
        if !self.contains_target(removed) {
            return Err(LayoutError::StaleTarget);
        }
        if expected_revision != self.state.snapshot.revision {
            return Err(LayoutError::StaleRevision);
        }
        if self.state.snapshot.pane_count() <= 1 {
            return Err(LayoutError::InvalidLayoutState);
        }
        let nodes = self.state.nodes;
        let removed_index = nodes
            .iter()
            .position(|node| matches!(node, TreeNode::Leaf(target) if *target == removed))
            .ok_or(LayoutError::InvalidLayoutState)?;
        let (parent_index, sibling_index) = nodes
            .iter()
            .enumerate()
            .find_map(|(index, node)| match node {
                TreeNode::Split { first, second, .. } if *first == removed_index => {
                    Some((index, *second))
                }
                TreeNode::Split { first, second, .. } if *second == removed_index => {
                    Some((index, *first))
                }
                _ => None,
            })
            .ok_or(LayoutError::InvalidLayoutState)?;
        let mut next_nodes = nodes;
        next_nodes[parent_index] = nodes[sibling_index];
        next_nodes[removed_index] = TreeNode::Empty;
        next_nodes[sibling_index] = TreeNode::Empty;
        let next_revision = self
            .state
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(LayoutError::RevisionExhausted)?;
        let candidate = state_from_tree(
            next_nodes,
            self.state.snapshot.rows,
            self.state.snapshot.columns,
            next_revision,
        )?;
        self.state = candidate;
        Ok(self.state.snapshot)
    }

    /// Returns the pane covering a cell, excluding the divider and root exterior.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<PaneGeometry> {
        self.state
            .snapshot
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.rect.contains(x, y))
    }

    fn contains_target(&self, target: TargetRef) -> bool {
        self.state
            .snapshot
            .panes
            .into_iter()
            .flatten()
            .any(|pane| pane.target == target)
    }
}

/// Return the geometry of the new pane when splitting one current pane.
pub fn split_geometry(rect: Rect, direction: SplitDirection) -> Result<(u16, u16), LayoutError> {
    let (first, second, _) = split_rectangles(rect, direction)?;
    let new_rect = match direction {
        SplitDirection::Left | SplitDirection::Up => first,
        SplitDirection::Right | SplitDirection::Down => second,
    };
    Ok((new_rect.rows, new_rect.columns))
}

fn split_rectangles_for_direction(
    root: Rect,
    direction: SplitDirection,
) -> Result<(Rect, Rect, Rect), LayoutError> {
    split_rectangles(root, direction)
}

fn build_tree_from_rects(
    region: Rect,
    panes: &[PaneGeometry],
    dividers: &[Option<Rect>; MAX_PANES - 1],
    nodes: &mut [TreeNode; MAX_TREE_NODES],
    node_index: usize,
    next: &mut usize,
    visited: &mut [bool; MAX_PANES - 1],
) -> Result<(), LayoutError> {
    let contained: Vec<_> = panes
        .iter()
        .copied()
        .filter(|pane| rect_contains(region, pane.rect))
        .collect();
    if contained.len() == 1 && contained[0].rect == region {
        nodes[node_index] = TreeNode::Leaf(contained[0].target);
        return Ok(());
    }
    for (divider_index, divider) in dividers.iter().enumerate() {
        if visited[divider_index] {
            continue;
        }
        let Some(divider) = *divider else { continue };
        let horizontal = divider.columns == 1
            && divider.y == region.y
            && divider.rows == region.rows
            && divider.x > region.x
            && u32::from(divider.x) + 1 < u32::from(region.x) + u32::from(region.columns);
        let vertical = divider.rows == 1
            && divider.x == region.x
            && divider.columns == region.columns
            && divider.y > region.y
            && u32::from(divider.y) + 1 < u32::from(region.y) + u32::from(region.rows);
        if !horizontal && !vertical {
            continue;
        }
        let (first_rect, second_rect, first_count, second_count, first_cells, total_cells) =
            if horizontal {
                let first_columns = divider.x - region.x;
                let second_x = divider.x + 1;
                let second_columns = region.x + region.columns - second_x;
                let first_rect = Rect {
                    columns: first_columns,
                    ..region
                };
                let second_rect = Rect {
                    x: second_x,
                    columns: second_columns,
                    ..region
                };
                let first_count = contained
                    .iter()
                    .filter(|pane| rect_contains(first_rect, pane.rect))
                    .count();
                let second_count = contained
                    .iter()
                    .filter(|pane| rect_contains(second_rect, pane.rect))
                    .count();
                (
                    first_rect,
                    second_rect,
                    first_count,
                    second_count,
                    first_columns,
                    region.columns - 1,
                )
            } else {
                let first_rows = divider.y - region.y;
                let second_y = divider.y + 1;
                let second_rows = region.y + region.rows - second_y;
                let first_rect = Rect {
                    rows: first_rows,
                    ..region
                };
                let second_rect = Rect {
                    y: second_y,
                    rows: second_rows,
                    ..region
                };
                let first_count = contained
                    .iter()
                    .filter(|pane| rect_contains(first_rect, pane.rect))
                    .count();
                let second_count = contained
                    .iter()
                    .filter(|pane| rect_contains(second_rect, pane.rect))
                    .count();
                (
                    first_rect,
                    second_rect,
                    first_count,
                    second_count,
                    first_rows,
                    region.rows - 1,
                )
            };
        if first_count == 0 || second_count == 0 || first_count + second_count != contained.len() {
            continue;
        }
        let first_index = *next;
        let second_index = first_index + 1;
        if second_index >= MAX_TREE_NODES {
            return Err(LayoutError::InvalidLayoutState);
        }
        *next += 2;
        visited[divider_index] = true;
        nodes[node_index] = TreeNode::Split {
            first: first_index,
            second: second_index,
            horizontal,
            first_cells,
            total_cells,
        };
        build_tree_from_rects(
            first_rect,
            panes,
            dividers,
            nodes,
            first_index,
            next,
            visited,
        )?;
        build_tree_from_rects(
            second_rect,
            panes,
            dividers,
            nodes,
            second_index,
            next,
            visited,
        )?;
        return Ok(());
    }
    Err(LayoutError::InvalidLayoutState)
}

fn rect_contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && u32::from(inner.x) + u32::from(inner.columns)
            <= u32::from(outer.x) + u32::from(outer.columns)
        && u32::from(inner.y) + u32::from(inner.rows) <= u32::from(outer.y) + u32::from(outer.rows)
}

impl fmt::Debug for PaneLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneLayout")
            .field("rows", &self.state.snapshot.rows)
            .field("columns", &self.state.snapshot.columns)
            .field("pane_count", &self.state.snapshot.pane_count())
            .finish_non_exhaustive()
    }
}

/// Opaque validated before/after state for one proposed split.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitPreview {
    before: LayoutState,
    after: LayoutState,
}

impl SplitPreview {
    /// Returns the planned post-commit snapshot without authorizing effects.
    pub fn planned_snapshot(self) -> LayoutSnapshot {
        self.after.snapshot
    }
}

impl fmt::Debug for SplitPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitPreview")
            .field("before", &"[redacted]")
            .field("planned", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// Fixed, sanitized failures from layout validation and mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// A target contains a zero identity field.
    InvalidTarget,
    /// The root is outside the supported terminal-core bounds.
    InvalidDimensions,
    /// The supplied target does not exactly identify a current pane.
    StaleTarget,
    /// The expected layout revision does not match the current revision.
    StaleRevision,
    /// The new target belongs to another generation or workspace.
    TargetScopeMismatch,
    /// The new pane or session identity is already used by the selected pane.
    TargetIdentityConflict,
    /// One or more panes would fall below the minimum supported size.
    LayoutTooSmall,
    /// One or more pane coordinates or dimensions exceed supported bounds.
    LayoutTooLarge,
    /// Both prototype pane slots are already occupied.
    PaneLimitExceeded,
    /// The layout revision cannot be incremented without wrapping.
    RevisionExhausted,
    /// The preview's complete before-state no longer matches this layout.
    PreviewStale,
    /// An internal fixed-state invariant was not satisfied.
    InvalidLayoutState,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTarget => "target identity is invalid",
            Self::InvalidDimensions => "root dimensions are outside supported bounds",
            Self::StaleTarget => "target no longer identifies a current pane",
            Self::StaleRevision => "layout revision is stale",
            Self::TargetScopeMismatch => "new target is outside the existing layout scope",
            Self::TargetIdentityConflict => {
                "new target reuses an existing pane or session identity"
            }
            Self::LayoutTooSmall => "one or more panes would be too small",
            Self::LayoutTooLarge => "one or more panes would exceed supported bounds",
            Self::PaneLimitExceeded => "the four-pane workspace capacity is full",
            Self::RevisionExhausted => "layout revision cannot be incremented",
            Self::PreviewStale => "split preview no longer matches the layout",
            Self::InvalidLayoutState => "layout state is invalid",
        };
        f.write_str(message)
    }
}

impl Error for LayoutError {}

#[derive(Clone, Copy, Eq, PartialEq)]
struct LayoutState {
    snapshot: LayoutSnapshot,
    nodes: [TreeNode; MAX_TREE_NODES],
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TreeNode {
    Empty,
    Leaf(TargetRef),
    Split {
        first: usize,
        second: usize,
        horizontal: bool,
        first_cells: u16,
        total_cells: u16,
    },
}

fn validate_target(target: TargetRef) -> Result<(), LayoutError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        return Err(LayoutError::InvalidTarget);
    }
    Ok(())
}

fn validate_root_dimensions(rows: u16, columns: u16) -> Result<(), LayoutError> {
    if !(MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rows)
        || !(MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
    {
        return Err(LayoutError::InvalidDimensions);
    }
    Ok(())
}

fn root_rect(rows: u16, columns: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    }
}

fn split_rectangles(
    root: Rect,
    direction: SplitDirection,
) -> Result<(Rect, Rect, Rect), LayoutError> {
    match direction {
        SplitDirection::Left | SplitDirection::Right => {
            let usable = root
                .columns
                .checked_sub(1)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let first_columns = usable / 2;
            let second_columns = usable
                .checked_sub(first_columns)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let divider_x = root
                .x
                .checked_add(first_columns)
                .ok_or(LayoutError::LayoutTooLarge)?;
            let second_x = divider_x
                .checked_add(1)
                .ok_or(LayoutError::LayoutTooLarge)?;
            let first = Rect {
                x: root.x,
                y: root.y,
                rows: root.rows,
                columns: first_columns,
            };
            let second = Rect {
                x: second_x,
                y: root.y,
                rows: root.rows,
                columns: second_columns,
            };
            let divider = Rect {
                x: divider_x,
                y: root.y,
                rows: root.rows,
                columns: 1,
            };
            validate_leaf(first)?;
            validate_leaf(second)?;
            validate_partition_bounds(root, first, second, divider)?;
            Ok((first, second, divider))
        }
        SplitDirection::Up | SplitDirection::Down => {
            let usable = root
                .rows
                .checked_sub(1)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let first_rows = usable / 2;
            let second_rows = usable
                .checked_sub(first_rows)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let divider_y = root
                .y
                .checked_add(first_rows)
                .ok_or(LayoutError::LayoutTooLarge)?;
            let second_y = divider_y
                .checked_add(1)
                .ok_or(LayoutError::LayoutTooLarge)?;
            let first = Rect {
                x: root.x,
                y: root.y,
                rows: first_rows,
                columns: root.columns,
            };
            let second = Rect {
                x: root.x,
                y: second_y,
                rows: second_rows,
                columns: root.columns,
            };
            let divider = Rect {
                x: root.x,
                y: divider_y,
                rows: 1,
                columns: root.columns,
            };
            validate_leaf(first)?;
            validate_leaf(second)?;
            validate_partition_bounds(root, first, second, divider)?;
            Ok((first, second, divider))
        }
    }
}

fn validate_leaf(rect: Rect) -> Result<(), LayoutError> {
    if rect.rows < MIN_PANE_ROWS || rect.columns < MIN_PANE_COLUMNS {
        return Err(LayoutError::LayoutTooSmall);
    }
    if rect.rows > MAX_PANE_ROWS || rect.columns > MAX_PANE_COLUMNS {
        return Err(LayoutError::LayoutTooLarge);
    }
    Ok(())
}

fn validate_partition_bounds(
    root: Rect,
    first: Rect,
    second: Rect,
    divider: Rect,
) -> Result<(), LayoutError> {
    let root_right = root
        .x
        .checked_add(root.columns)
        .ok_or(LayoutError::LayoutTooLarge)?;
    let root_bottom = root
        .y
        .checked_add(root.rows)
        .ok_or(LayoutError::LayoutTooLarge)?;
    for rect in [first, second, divider] {
        let right = rect
            .x
            .checked_add(rect.columns)
            .ok_or(LayoutError::LayoutTooLarge)?;
        let bottom = rect
            .y
            .checked_add(rect.rows)
            .ok_or(LayoutError::LayoutTooLarge)?;
        if rect.x < root.x || rect.y < root.y || right > root_right || bottom > root_bottom {
            return Err(LayoutError::LayoutTooLarge);
        }
    }
    Ok(())
}

fn state_from_tree(
    nodes: [TreeNode; MAX_TREE_NODES],
    rows: u16,
    columns: u16,
    revision: u64,
) -> Result<LayoutState, LayoutError> {
    validate_root_dimensions(rows, columns)?;
    let mut snapshot = LayoutSnapshot {
        revision,
        rows,
        columns,
        panes: [None; MAX_PANES],
        divider: None,
        dividers: [None; MAX_PANES - 1],
    };
    append_geometry(&nodes, 0, root_rect(rows, columns), &mut snapshot)?;
    if snapshot.pane_count() == 0 {
        return Err(LayoutError::InvalidLayoutState);
    }
    snapshot.divider = snapshot.dividers[0];
    Ok(LayoutState { snapshot, nodes })
}

fn append_geometry(
    nodes: &[TreeNode; MAX_TREE_NODES],
    index: usize,
    rect: Rect,
    snapshot: &mut LayoutSnapshot,
) -> Result<(), LayoutError> {
    match *nodes.get(index).ok_or(LayoutError::InvalidLayoutState)? {
        TreeNode::Empty => Err(LayoutError::InvalidLayoutState),
        TreeNode::Leaf(target) => {
            validate_leaf(rect)?;
            let slot = snapshot
                .panes
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(LayoutError::PaneLimitExceeded)?;
            *slot = Some(PaneGeometry { target, rect });
            Ok(())
        }
        TreeNode::Split {
            first,
            second,
            horizontal,
            first_cells,
            total_cells,
        } => {
            if total_cells == 0 || first_cells == 0 || first_cells >= total_cells {
                return Err(LayoutError::InvalidLayoutState);
            }
            let dimension = if horizontal { rect.columns } else { rect.rows };
            let usable = dimension
                .checked_sub(1)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let first_dimension = u32::from(first_cells)
                .checked_mul(u32::from(usable))
                .ok_or(LayoutError::LayoutTooLarge)?
                / u32::from(total_cells);
            let first_dimension =
                u16::try_from(first_dimension).map_err(|_| LayoutError::LayoutTooLarge)?;
            let second_dimension = usable
                .checked_sub(first_dimension)
                .ok_or(LayoutError::LayoutTooSmall)?;
            let (first_rect, second_rect, divider) = if horizontal {
                let divider_x = rect
                    .x
                    .checked_add(first_dimension)
                    .ok_or(LayoutError::LayoutTooLarge)?;
                let second_x = divider_x
                    .checked_add(1)
                    .ok_or(LayoutError::LayoutTooLarge)?;
                (
                    Rect {
                        columns: first_dimension,
                        ..rect
                    },
                    Rect {
                        x: second_x,
                        columns: second_dimension,
                        ..rect
                    },
                    Rect {
                        x: divider_x,
                        columns: 1,
                        ..rect
                    },
                )
            } else {
                let divider_y = rect
                    .y
                    .checked_add(first_dimension)
                    .ok_or(LayoutError::LayoutTooLarge)?;
                let second_y = divider_y
                    .checked_add(1)
                    .ok_or(LayoutError::LayoutTooLarge)?;
                (
                    Rect {
                        rows: first_dimension,
                        ..rect
                    },
                    Rect {
                        y: second_y,
                        rows: second_dimension,
                        ..rect
                    },
                    Rect {
                        y: divider_y,
                        rows: 1,
                        ..rect
                    },
                )
            };
            validate_leaf(first_rect)?;
            validate_leaf(second_rect)?;
            let divider_slot = snapshot
                .dividers
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(LayoutError::InvalidLayoutState)?;
            *divider_slot = Some(divider);
            append_geometry(nodes, first, first_rect, snapshot)?;
            append_geometry(nodes, second, second_rect, snapshot)
        }
    }
}

fn resized_state(
    current: LayoutState,
    rows: u16,
    columns: u16,
) -> Result<LayoutState, LayoutError> {
    state_from_tree(current.nodes, rows, columns, current.snapshot.revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn existing_target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 33,
            session: 44,
            occupant_revision: 55,
        }
    }

    fn new_target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 66,
            session: 77,
            occupant_revision: 88,
        }
    }

    fn geometry(
        target: TargetRef,
        x: u16,
        y: u16,
        rows: u16,
        columns: u16,
    ) -> Option<PaneGeometry> {
        Some(PaneGeometry {
            target,
            rect: Rect {
                x,
                y,
                rows,
                columns,
            },
        })
    }

    fn assert_split(
        direction: SplitDirection,
        rows: u16,
        columns: u16,
        first: PaneGeometry,
        second: PaneGeometry,
        divider: Rect,
    ) {
        let layout = PaneLayout::new(existing_target(), rows, columns).expect("valid root");
        let preview = layout
            .preview_split(existing_target(), 1, direction, new_target())
            .expect("split preview");
        assert_eq!(
            preview.planned_snapshot(),
            LayoutSnapshot {
                revision: 2,
                rows,
                columns,
                panes: [Some(first), Some(second), None, None],
                divider: Some(divider),
                dividers: [Some(divider), None, None],
            }
        );
    }

    #[test]
    fn four_directions_use_spatial_order_one_cell_divider_and_floor_rounding() {
        let old = existing_target();
        let new = new_target();
        assert_split(
            SplitDirection::Left,
            7,
            82,
            geometry(new, 0, 0, 7, 40).expect("new left"),
            geometry(old, 41, 0, 7, 41).expect("old right"),
            Rect {
                x: 40,
                y: 0,
                rows: 7,
                columns: 1,
            },
        );
        assert_split(
            SplitDirection::Right,
            7,
            83,
            geometry(old, 0, 0, 7, 41).expect("old left"),
            geometry(new, 42, 0, 7, 41).expect("new right"),
            Rect {
                x: 41,
                y: 0,
                rows: 7,
                columns: 1,
            },
        );
        assert_split(
            SplitDirection::Up,
            11,
            82,
            geometry(new, 0, 0, 5, 82).expect("new top"),
            geometry(old, 0, 6, 5, 82).expect("old bottom"),
            Rect {
                x: 0,
                y: 5,
                rows: 1,
                columns: 82,
            },
        );
        assert_split(
            SplitDirection::Down,
            12,
            82,
            geometry(old, 0, 0, 5, 82).expect("old top"),
            geometry(new, 0, 6, 6, 82).expect("new bottom"),
            Rect {
                x: 0,
                y: 5,
                rows: 1,
                columns: 82,
            },
        );
    }

    #[test]
    fn exact_minimum_splits_succeed_and_one_cell_less_fails() {
        let old = existing_target();
        for direction in [SplitDirection::Left, SplitDirection::Right] {
            let at_minimum = PaneLayout::new(old, 5, 41).expect("minimum root");
            assert_eq!(at_minimum.can_split(old, direction), Ok(()));
            let too_narrow = PaneLayout::new(old, 5, 40).expect("valid unsplittable root");
            assert_eq!(
                too_narrow.can_split(old, direction),
                Err(LayoutError::LayoutTooSmall)
            );
        }
        for direction in [SplitDirection::Up, SplitDirection::Down] {
            let at_minimum = PaneLayout::new(old, 11, 20).expect("minimum root");
            assert_eq!(at_minimum.can_split(old, direction), Ok(()));
            let too_short = PaneLayout::new(old, 10, 20).expect("valid unsplittable root");
            assert_eq!(
                too_short.can_split(old, direction),
                Err(LayoutError::LayoutTooSmall)
            );
        }
    }

    #[test]
    fn root_and_every_resulting_leaf_respect_core_minimum_and_maximum() {
        let old = existing_target();
        let layout = PaneLayout::new(old, MAX_PANE_ROWS, MAX_PANE_COLUMNS).expect("max root");
        assert_eq!(layout.snapshot().panes[0].expect("root pane").rect.rows, 80);
        for direction in [
            SplitDirection::Left,
            SplitDirection::Right,
            SplitDirection::Up,
            SplitDirection::Down,
        ] {
            let new = new_target();
            let preview = layout
                .preview_split(old, 1, direction, new)
                .expect("maximum root split");
            let planned = preview.planned_snapshot();
            for pane in planned.panes.into_iter().flatten() {
                assert!((MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&pane.rect.rows));
                assert!((MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&pane.rect.columns));
            }
        }
        assert_eq!(
            PaneLayout::new(old, 4, 20).map(|_| ()),
            Err(LayoutError::InvalidDimensions)
        );
        assert_eq!(
            PaneLayout::new(old, 5, 19).map(|_| ()),
            Err(LayoutError::InvalidDimensions)
        );
        assert_eq!(
            PaneLayout::new(old, 81, 240).map(|_| ()),
            Err(LayoutError::InvalidDimensions)
        );
        assert_eq!(
            PaneLayout::new(old, 80, 241).map(|_| ()),
            Err(LayoutError::InvalidDimensions)
        );
    }

    #[test]
    fn target_validation_checks_every_field_and_scope_identity_rules() {
        let original = existing_target();
        let fields: [fn(&mut TargetRef); 5] = [
            |target: &mut TargetRef| target.generation = 0,
            |target: &mut TargetRef| target.workspace = 0,
            |target: &mut TargetRef| target.pane = 0,
            |target: &mut TargetRef| target.session = 0,
            |target: &mut TargetRef| target.occupant_revision = 0,
        ];
        for invalidate in fields {
            let mut invalid = original;
            invalidate(&mut invalid);
            assert_eq!(
                PaneLayout::new(invalid, 5, 20).map(|_| ()),
                Err(LayoutError::InvalidTarget)
            );
            let layout = PaneLayout::new(original, 5, 41).expect("layout");
            assert_eq!(
                layout.preview_split(original, 1, SplitDirection::Right, invalid),
                Err(LayoutError::InvalidTarget)
            );
        }

        let layout = PaneLayout::new(original, 5, 41).expect("layout");
        let mut wrong_generation = new_target();
        wrong_generation.generation += 1;
        assert_eq!(
            layout.preview_split(original, 1, SplitDirection::Right, wrong_generation),
            Err(LayoutError::TargetScopeMismatch)
        );
        let mut wrong_workspace = new_target();
        wrong_workspace.workspace += 1;
        assert_eq!(
            layout.preview_split(original, 1, SplitDirection::Right, wrong_workspace),
            Err(LayoutError::TargetScopeMismatch)
        );
        let mut reused_pane = new_target();
        reused_pane.pane = original.pane;
        assert_eq!(
            layout.preview_split(original, 1, SplitDirection::Right, reused_pane),
            Err(LayoutError::TargetIdentityConflict)
        );
        let mut reused_session = new_target();
        reused_session.session = original.session;
        assert_eq!(
            layout.preview_split(original, 1, SplitDirection::Right, reused_session),
            Err(LayoutError::TargetIdentityConflict)
        );
    }

    #[test]
    fn preview_is_read_only_and_commit_changes_revision_once() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 7, 82).expect("layout");
        let before = layout.snapshot();
        let preview = layout
            .preview_split(
                original,
                before.revision,
                SplitDirection::Left,
                new_target(),
            )
            .expect("preview");
        let stale_preview = layout
            .preview_split(
                original,
                before.revision,
                SplitDirection::Left,
                new_target(),
            )
            .expect("second preview from the same state");
        assert_eq!(layout.snapshot(), before);
        assert_eq!(preview.planned_snapshot().revision, before.revision + 1);
        assert_eq!(preview.planned_snapshot().pane_count(), 2);

        let committed = layout.commit_split(preview).expect("commit");
        assert_eq!(committed, preview.planned_snapshot());
        assert_eq!(committed.revision, before.revision + 1);
        assert_eq!(committed.panes[0].expect("new pane").target, new_target());
        assert_eq!(committed.panes[1].expect("existing pane").target, original);
        assert_eq!(committed.panes[1].expect("existing pane").rect.x, 41);
        assert_eq!(
            layout.commit_split(stale_preview),
            Err(LayoutError::PreviewStale)
        );
        assert_eq!(layout.snapshot(), committed);
    }

    #[test]
    fn preview_and_rejection_failures_leave_the_layout_unchanged() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 5, 41).expect("layout");
        let before = layout.snapshot();
        assert_eq!(
            layout.preview_split(original, 2, SplitDirection::Right, new_target()),
            Err(LayoutError::StaleRevision)
        );
        let mut stale = original;
        stale.occupant_revision += 1;
        assert_eq!(
            layout.preview_split(stale, 1, SplitDirection::Right, new_target()),
            Err(LayoutError::StaleTarget)
        );
        let changed_fields: [fn(&mut TargetRef); 5] = [
            |target: &mut TargetRef| target.generation += 1,
            |target: &mut TargetRef| target.workspace += 1,
            |target: &mut TargetRef| target.pane += 1,
            |target: &mut TargetRef| target.session += 1,
            |target: &mut TargetRef| target.occupant_revision += 1,
        ];
        for change in changed_fields {
            let mut changed = original;
            change(&mut changed);
            assert_eq!(
                layout.can_split(changed, SplitDirection::Right),
                Err(LayoutError::StaleTarget)
            );
        }
        assert_eq!(
            layout.preview_split(original, 1, SplitDirection::Down, new_target()),
            Err(LayoutError::LayoutTooSmall)
        );
        assert_eq!(layout.snapshot(), before);

        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("valid preview");
        assert_eq!(
            layout
                .resize(1, 5, 42)
                .expect("resize one-pane layout")
                .revision,
            2
        );
        let resized = layout.snapshot();
        assert_eq!(layout.commit_split(preview), Err(LayoutError::PreviewStale));
        assert_eq!(layout.snapshot(), resized);
    }

    #[test]
    fn capacity_is_four_and_rejects_a_fifth_pane() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 11, 121).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("first split");
        layout.commit_split(preview).expect("commit");
        let third = TargetRef {
            pane: 90,
            session: 91,
            ..new_target()
        };
        let preview = layout
            .preview_split(new_target(), 2, SplitDirection::Up, third)
            .expect("third");
        layout.commit_split(preview).expect("third commit");
        let fourth = TargetRef {
            pane: 92,
            session: 93,
            ..new_target()
        };
        let preview = layout
            .preview_split(original, 3, SplitDirection::Down, fourth)
            .expect("fourth");
        layout.commit_split(preview).expect("fourth commit");
        let snapshot = layout.snapshot();
        assert_eq!(snapshot.pane_count(), MAX_PANES);
        assert_eq!(
            layout.can_split(fourth, SplitDirection::Left),
            Err(LayoutError::PaneLimitExceeded)
        );
        assert_eq!(
            layout.preview_split(
                fourth,
                4,
                SplitDirection::Left,
                TargetRef {
                    pane: 99,
                    session: 100,
                    ..new_target()
                }
            ),
            Err(LayoutError::PaneLimitExceeded)
        );
        assert_eq!(layout.snapshot(), snapshot);
    }

    #[test]
    fn every_leaf_can_be_split_in_each_direction_then_any_leaf_collapses() {
        let root_target = existing_target();
        let mut layout = PaneLayout::new(root_target, 21, 161).expect("large enough root");
        let mut targets = vec![root_target];
        for index in 0..3 {
            let split_target = targets[index];
            let direction = [
                SplitDirection::Right,
                SplitDirection::Down,
                SplitDirection::Left,
            ][index];
            let fresh = TargetRef {
                pane: 100 + index as u64,
                session: 200 + index as u64,
                ..root_target
            };
            let revision = layout.snapshot().revision;
            let preview = layout
                .preview_split(split_target, revision, direction, fresh)
                .expect("split below cap");
            let snapshot = layout.commit_split(preview).expect("commit split");
            assert_eq!(snapshot.pane_count(), index + 2);
            targets.push(fresh);
            for pane in snapshot.panes.iter().flatten() {
                assert!(layout
                    .hit_test(pane.rect.x, pane.rect.y)
                    .is_some_and(|hit| hit.target == pane.target));
            }
        }
        let before = layout.snapshot();
        assert_eq!(before.pane_count(), MAX_PANES);
        for direction in [
            SplitDirection::Left,
            SplitDirection::Right,
            SplitDirection::Up,
            SplitDirection::Down,
        ] {
            assert!(matches!(
                layout.can_split(targets[3], direction),
                Err(LayoutError::PaneLimitExceeded)
            ));
        }
        for removed in targets {
            let mut branch = PaneLayout::from_snapshot(before).expect("rebuild tree");
            let collapsed = branch
                .collapse(removed, before.revision)
                .expect("collapse any leaf");
            assert_eq!(collapsed.pane_count(), MAX_PANES - 1);
            assert_eq!(collapsed.revision, before.revision + 1);
        }
    }

    #[test]
    fn resize_recomputes_both_panes_and_no_op_preserves_revision() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 11, 82).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Down, new_target())
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let before = layout.snapshot();
        assert_eq!(layout.resize(before.revision, 11, 82), Ok(before));
        assert_eq!(layout.snapshot(), before);

        let resized = layout.resize(before.revision, 12, 83).expect("resize");
        assert_eq!(resized.revision, before.revision + 1);
        assert_eq!(resized.panes[0].expect("old top").target, original);
        assert_eq!(
            resized.panes[0].expect("old top").rect,
            Rect {
                x: 0,
                y: 0,
                rows: 5,
                columns: 83
            }
        );
        assert_eq!(resized.panes[1].expect("new bottom").target, new_target());
        assert_eq!(
            resized.panes[1].expect("new bottom").rect,
            Rect {
                x: 0,
                y: 6,
                rows: 6,
                columns: 83
            }
        );
        assert_eq!(
            resized.divider,
            Some(Rect {
                x: 0,
                y: 5,
                rows: 1,
                columns: 83
            })
        );
    }

    #[test]
    fn stale_invalid_oversize_and_too_small_resizes_preserve_two_panes() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 7, 82).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let before = layout.snapshot();
        assert_eq!(
            layout.resize(before.revision + 1, 7, 83),
            Err(LayoutError::StaleRevision)
        );
        assert_eq!(
            layout.resize(before.revision, 7, 40),
            Err(LayoutError::LayoutTooSmall)
        );
        assert_eq!(
            layout.resize(before.revision, 81, 83),
            Err(LayoutError::InvalidDimensions)
        );
        assert_eq!(
            layout.resize(before.revision, 7, 241),
            Err(LayoutError::InvalidDimensions)
        );
        assert_eq!(layout.snapshot(), before);
    }

    #[test]
    fn hit_testing_partitions_every_root_cell_and_excludes_divider_and_exterior() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 7, 82).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let snapshot = layout.snapshot();
        let mut pane_cells = 0_u32;
        let mut divider_cells = 0_u32;
        for y in 0..snapshot.rows {
            for x in 0..snapshot.columns {
                let expected = snapshot
                    .panes
                    .into_iter()
                    .flatten()
                    .find(|pane| pane.rect.contains(x, y));
                let actual = layout.hit_test(x, y);
                assert_eq!(actual, expected);
                if actual.is_some() {
                    pane_cells += 1;
                } else {
                    divider_cells += 1;
                }
            }
        }
        assert_eq!(
            pane_cells + divider_cells,
            u32::from(snapshot.rows) * u32::from(snapshot.columns)
        );
        assert_eq!(divider_cells, u32::from(snapshot.rows));
        assert_eq!(layout.hit_test(snapshot.columns, 0), None);
        assert_eq!(layout.hit_test(0, snapshot.rows), None);
        assert_eq!(layout.hit_test(u16::MAX, u16::MAX), None);
    }

    #[test]
    fn revision_overflow_rejects_changes_but_allows_no_op_resize() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 5, 41).expect("layout");
        layout.state.snapshot.revision = u64::MAX;
        let before = layout.snapshot();
        assert_eq!(layout.resize(u64::MAX, 5, 41), Ok(before));
        assert_eq!(
            layout.can_split(original, SplitDirection::Right),
            Err(LayoutError::RevisionExhausted)
        );
        assert_eq!(
            layout.preview_split(original, u64::MAX, SplitDirection::Right, new_target()),
            Err(LayoutError::RevisionExhausted)
        );
        assert_eq!(
            layout.resize(u64::MAX, 5, 42),
            Err(LayoutError::RevisionExhausted)
        );
        assert_eq!(layout.snapshot(), before);
    }

    fn third_target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 99,
            session: 100,
            occupant_revision: 1,
        }
    }

    #[test]
    fn collapse_gives_either_survivor_the_root_in_every_direction() {
        for direction in [
            SplitDirection::Left,
            SplitDirection::Right,
            SplitDirection::Up,
            SplitDirection::Down,
        ] {
            for (removed, survivor) in [
                (new_target(), existing_target()),
                (existing_target(), new_target()),
            ] {
                let mut layout = PaneLayout::new(existing_target(), 11, 82).expect("layout");
                let preview = layout
                    .preview_split(existing_target(), 1, direction, new_target())
                    .expect("split");
                layout.commit_split(preview).expect("commit");
                let collapsed = layout.collapse(removed, 2).expect("collapse");
                assert_eq!(
                    collapsed,
                    LayoutSnapshot {
                        revision: 3,
                        rows: 11,
                        columns: 82,
                        panes: [geometry(survivor, 0, 0, 11, 82), None, None, None],
                        divider: None,
                        dividers: [None; MAX_PANES - 1],
                    },
                    "{direction:?}"
                );
                assert_eq!(layout.snapshot(), collapsed);
                assert_eq!(
                    layout.hit_test(41, 5).map(|pane| pane.target),
                    Some(survivor)
                );
                // Resizing a collapsed layout keeps one root pane.
                let resized = layout.resize(3, 12, 83).expect("resize");
                assert_eq!(
                    resized.panes,
                    [geometry(survivor, 0, 0, 12, 83), None, None, None]
                );
            }
        }
    }

    #[test]
    fn collapse_rejections_leave_layout_unchanged() {
        let original = existing_target();
        let mut single = PaneLayout::new(original, 7, 82).expect("layout");
        let before = single.snapshot();
        assert_eq!(
            single.collapse(original, 1),
            Err(LayoutError::InvalidLayoutState)
        );
        assert_eq!(single.snapshot(), before);

        let mut layout = PaneLayout::new(original, 7, 82).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let split = layout.snapshot();
        assert_eq!(
            layout.collapse(original, 1),
            Err(LayoutError::StaleRevision)
        );
        let mut stale = original;
        stale.occupant_revision += 1;
        assert_eq!(layout.collapse(stale, 2), Err(LayoutError::StaleTarget));
        assert_eq!(
            layout.collapse(third_target(), 2),
            Err(LayoutError::StaleTarget)
        );
        assert_eq!(layout.snapshot(), split);

        layout.state.snapshot.revision = u64::MAX;
        assert_eq!(
            layout.collapse(original, u64::MAX),
            Err(LayoutError::RevisionExhausted)
        );
        assert_eq!(layout.snapshot().pane_count(), 2);
    }

    #[test]
    fn survivor_can_split_again_with_a_fresh_target_after_collapse() {
        let original = existing_target();
        let mut layout = PaneLayout::new(original, 7, 82).expect("layout");
        let preview = layout
            .preview_split(original, 1, SplitDirection::Right, new_target())
            .expect("split");
        let stale_preview = preview;
        layout.commit_split(preview).expect("commit");
        layout.collapse(original, 2).expect("collapse");
        assert_eq!(
            layout.commit_split(stale_preview),
            Err(LayoutError::PreviewStale)
        );
        assert_eq!(
            layout.can_split(new_target(), SplitDirection::Down),
            Err(LayoutError::LayoutTooSmall)
        );
        let again = layout
            .preview_split(new_target(), 3, SplitDirection::Left, third_target())
            .expect("re-split preview");
        let committed = layout.commit_split(again).expect("re-split");
        assert_eq!(committed.revision, 4);
        assert_eq!(committed.panes[0].expect("new left").target, third_target());
        assert_eq!(
            committed.panes[1].expect("survivor right").target,
            new_target()
        );
        assert!(committed.divider.is_some());
    }

    #[test]
    fn layout_debug_redacts_targets_and_revisions() {
        let mut original = existing_target();
        original.generation = 9_876_543_210_123;
        original.workspace = 9_876_543_210_124;
        original.pane = 9_876_543_210_125;
        original.session = 9_876_543_210_126;
        original.occupant_revision = 9_876_543_210_127;
        let layout = PaneLayout::new(original, 5, 41).expect("layout");
        let preview = layout
            .preview_split(
                original,
                1,
                SplitDirection::Right,
                TargetRef {
                    generation: original.generation,
                    workspace: original.workspace,
                    pane: 9_876_543_210_128,
                    session: 9_876_543_210_129,
                    occupant_revision: 9_876_543_210_130,
                },
            )
            .expect("preview");
        for rendered in [
            format!("{layout:?}"),
            format!("{:?}", layout.snapshot()),
            format!("{:?}", preview.planned_snapshot()),
            format!("{preview:?}"),
            format!("{:?}", layout.snapshot().panes[0].expect("pane")),
        ] {
            for marker in [
                "9876543210123",
                "9876543210124",
                "9876543210125",
                "9876543210126",
                "9876543210127",
                "9876543210128",
                "9876543210129",
                "9876543210130",
            ] {
                assert!(!rendered.contains(marker), "debug output leaked {marker}");
            }
        }
        let error = LayoutError::StaleRevision;
        assert_eq!(error.to_string(), "layout revision is stale");
    }
}
