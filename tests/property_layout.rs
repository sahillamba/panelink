//! Property-based tests for the pure pane-layout geometry and mutation rules.
//!
//! Packet 2 of `docs/design/TRACING-PROPTEST-PLAN.md`. Strategies generate
//! bounded sequences of split/resize/close/swap/divider/revision operations with
//! both current and stale identities. Every operation is followed by an
//! independent check of the observable snapshot:
//!
//! * root containment, pairwise disjointness, and complete area coverage;
//! * required one-cell dividers, identity uniqueness, minimum pane sizes;
//! * the four-pane limit and the no-op/stale revision rules;
//! * rejected operations leave the entire snapshot unchanged;
//! * `hit_test` and `from_snapshot` agree with the live layout.
//!
//! The harness never assumes an operation must succeed. Shrinking therefore
//! stays valid: a shortened sequence simply takes more rejection branches, and
//! each branch is itself an asserted, explicit outcome.
//!
//! Failures persist to `proptest-regressions/property_layout.txt` (a reviewed,
//! versioned file). Re-running this test target replays every persisted case
//! before generating new ones. `PROPTEST_CASES` may raise the case count for an
//! opt-in local stress run; the committed default stays at 256.

use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{
    content_rect, split_geometry, LayoutError, LayoutSnapshot, PaneGeometry, PaneLayout, Rect,
    SplitDirection, MAX_PANES, MAX_PANE_COLUMNS, MAX_PANE_ROWS, MIN_PANE_COLUMNS, MIN_PANE_ROWS,
};
use proptest::prelude::*;
use proptest::test_runner::{FileFailurePersistence, TestCaseError};

const BASE_GENERATION: u64 = 11;
const BASE_WORKSPACE: u64 = 22;

/// Bounded operation-sequence length for this pilot.
const MAX_SEQUENCE_OPS: usize = 15;

fn config() -> ProptestConfig {
    ProptestConfig {
        // Default is 256; honor PROPTEST_CASES for opt-in larger runs.
        max_shrink_iters: 4_096,
        // Failures persist to a reviewed, versioned file under the crate-root
        // `proptest-regressions/` directory. Replay: re-run the same test target;
        // proptest re-executes every persisted case before new random cases.
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/property_layout.txt",
        ))),
        ..ProptestConfig::default()
    }
}

#[derive(Clone, Debug)]
enum LayoutOp {
    Split {
        pane_sel: u8,
        direction: u8,
        revision_mode: u8,
        identity_mode: u8,
        commit_mode: u8,
    },
    Resize {
        rows: u16,
        columns: u16,
        revision_mode: u8,
    },
    Close {
        pane_sel: u8,
        revision_mode: u8,
    },
    Swap {
        first_sel: u8,
        second_sel: u8,
        revision_mode: u8,
    },
    SetDivider {
        pane_sel: u8,
        divider_index: u8,
        span_mode: u8,
        revision_mode: u8,
    },
    AdvanceRevision {
        revision_mode: u8,
    },
}

fn arb_op() -> impl Strategy<Value = LayoutOp> {
    prop_oneof![
        4 => (0u8..8, 0u8..4, 0u8..3, 0u8..4, 0u8..2).prop_map(
            |(pane_sel, direction, revision_mode, identity_mode, commit_mode)| {
                LayoutOp::Split {
                    pane_sel,
                    direction,
                    revision_mode,
                    identity_mode,
                    commit_mode,
                }
            }
        ),
        2 => (arb_rows(), arb_columns(), 0u8..3)
            .prop_map(|(rows, columns, revision_mode)| LayoutOp::Resize {
                rows,
                columns,
                revision_mode,
            }),
        2 => (0u8..8, 0u8..3).prop_map(|(pane_sel, revision_mode)| LayoutOp::Close {
            pane_sel,
            revision_mode,
        }),
        2 => (0u8..8, 0u8..8, 0u8..3).prop_map(
            |(first_sel, second_sel, revision_mode)| LayoutOp::Swap {
                first_sel,
                second_sel,
                revision_mode,
            }
        ),
        2 => (0u8..8, 0u8..3, 0u8..5, 0u8..3).prop_map(
            |(pane_sel, divider_index, span_mode, revision_mode)| LayoutOp::SetDivider {
                pane_sel,
                divider_index,
                span_mode,
                revision_mode,
            }
        ),
        1 => (0u8..3).prop_map(|revision_mode| LayoutOp::AdvanceRevision { revision_mode }),
    ]
}

/// The first operation is always an attempt to split the single root pane, so
/// every generated sequence exercises at least one split (accepted or rejected).
fn arb_sequence() -> impl Strategy<Value = Vec<LayoutOp>> {
    (
        0u8..4,
        prop::collection::vec(arb_op(), 0..=MAX_SEQUENCE_OPS),
    )
        .prop_map(|(direction, mut ops)| {
            ops.insert(
                0,
                LayoutOp::Split {
                    pane_sel: 0,
                    direction,
                    revision_mode: 0,
                    identity_mode: 0,
                    commit_mode: 0,
                },
            );
            ops
        })
}

fn arb_rows() -> impl Strategy<Value = u16> {
    prop_oneof![
        4 => MIN_PANE_ROWS..=MAX_PANE_ROWS,
        1 => 0u16..MIN_PANE_ROWS,
        1 => (MAX_PANE_ROWS + 1)..=(MAX_PANE_ROWS + 8),
    ]
}

fn arb_columns() -> impl Strategy<Value = u16> {
    prop_oneof![
        // Wide enough for at least one horizontal split.
        3 => 45u16..=MAX_PANE_COLUMNS,
        1 => MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS,
        1 => 0u16..MIN_PANE_COLUMNS,
        1 => (MAX_PANE_COLUMNS + 1)..=(MAX_PANE_COLUMNS + 8),
    ]
}

fn target(pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation: BASE_GENERATION,
        workspace: BASE_WORKSPACE,
        pane,
        session,
        occupant_revision: 500 + pane,
    }
}

fn fresh_target(index: usize) -> TargetRef {
    target(1_000 + index as u64, 2_000 + index as u64)
}

fn stale_target(selector: u8) -> TargetRef {
    target(9_000 + u64::from(selector), 8_000 + u64::from(selector))
}

fn current_panes(snapshot: LayoutSnapshot) -> Vec<PaneGeometry> {
    snapshot.panes.iter().flatten().copied().collect()
}

fn pane_targets(snapshot: LayoutSnapshot) -> Vec<TargetRef> {
    current_panes(snapshot)
        .into_iter()
        .map(|pane| pane.target)
        .collect()
}

fn pane_rects(snapshot: LayoutSnapshot) -> Vec<Rect> {
    current_panes(snapshot)
        .into_iter()
        .map(|pane| pane.rect)
        .collect()
}

/// Selectors below 4 address a current pane by index; higher values are stale.
fn pick_target(snapshot: LayoutSnapshot, selector: u8) -> TargetRef {
    let panes = current_panes(snapshot);
    if selector < 4 {
        panes[usize::from(selector) % panes.len()].target
    } else {
        stale_target(selector)
    }
}

fn pick_direction(value: u8) -> SplitDirection {
    match value % 4 {
        0 => SplitDirection::Left,
        1 => SplitDirection::Right,
        2 => SplitDirection::Up,
        _ => SplitDirection::Down,
    }
}

/// Zero always addresses the current revision, one is a future revision that
/// must never match, and two is the impossible revision zero.
fn pick_revision(snapshot: LayoutSnapshot, mode: u8) -> u64 {
    match mode % 3 {
        0 => snapshot.revision,
        1 => snapshot.revision.wrapping_add(1),
        _ => 0,
    }
}

/// Independent re-derivation of the pane rectangle a split should produce.
fn predicted_split_rect(root: Rect, direction: SplitDirection) -> Rect {
    match direction {
        SplitDirection::Left | SplitDirection::Right => {
            let usable = root.columns - 1;
            let first_columns = usable / 2;
            let second_columns = usable - first_columns;
            let second = Rect {
                x: root.x + first_columns + 1,
                y: root.y,
                rows: root.rows,
                columns: second_columns,
            };
            match direction {
                SplitDirection::Left => Rect {
                    columns: first_columns,
                    ..root
                },
                _ => second,
            }
        }
        SplitDirection::Up | SplitDirection::Down => {
            let usable = root.rows - 1;
            let first_rows = usable / 2;
            let second_rows = usable - first_rows;
            let second = Rect {
                x: root.x,
                y: root.y + first_rows + 1,
                rows: second_rows,
                columns: root.columns,
            };
            match direction {
                SplitDirection::Up => Rect {
                    rows: first_rows,
                    ..root
                },
                _ => second,
            }
        }
    }
}

fn rect_area(rect: Rect) -> u32 {
    u32::from(rect.rows) * u32::from(rect.columns)
}

fn rect_contains(outer: Rect, inner: Rect) -> bool {
    u32::from(inner.x) >= u32::from(outer.x)
        && u32::from(inner.y) >= u32::from(outer.y)
        && u32::from(inner.x) + u32::from(inner.columns)
            <= u32::from(outer.x) + u32::from(outer.columns)
        && u32::from(inner.y) + u32::from(inner.rows) <= u32::from(outer.y) + u32::from(outer.rows)
}

fn rects_intersect(first: Rect, second: Rect) -> bool {
    u32::from(first.x) < u32::from(second.x) + u32::from(second.columns)
        && u32::from(second.x) < u32::from(first.x) + u32::from(first.columns)
        && u32::from(first.y) < u32::from(second.y) + u32::from(second.rows)
        && u32::from(second.y) < u32::from(first.y) + u32::from(first.rows)
}

fn multiset_eq<T: PartialEq>(first: &[T], second: &[T]) -> bool {
    first.len() == second.len()
        && first.iter().all(|item| {
            first.iter().filter(|other| *other == item).count()
                == second.iter().filter(|other| *other == item).count()
        })
}

fn check_snapshot(snapshot: LayoutSnapshot) -> Result<(), String> {
    if !(MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&snapshot.rows)
        || !(MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&snapshot.columns)
    {
        return Err(format!(
            "root {}x{} is outside the supported bounds",
            snapshot.rows, snapshot.columns
        ));
    }
    if snapshot.revision == 0 {
        return Err(String::from("revision must be nonzero"));
    }

    let root = Rect {
        x: 0,
        y: 0,
        rows: snapshot.rows,
        columns: snapshot.columns,
    };
    let panes = current_panes(snapshot);
    if panes.is_empty() || panes.len() > MAX_PANES {
        return Err(format!(
            "pane count {} is outside 1..={}",
            panes.len(),
            MAX_PANES
        ));
    }
    for (index, slot) in snapshot.panes.iter().enumerate() {
        if slot.is_some() != (index < panes.len()) {
            return Err(String::from("pane slots are not densely packed"));
        }
    }

    for (index, pane) in panes.iter().enumerate() {
        let identity = pane.target;
        if [
            identity.generation,
            identity.workspace,
            identity.pane,
            identity.session,
            identity.occupant_revision,
        ]
        .contains(&0)
        {
            return Err(format!("pane {index} has a zero identity field"));
        }
        if identity.generation != BASE_GENERATION {
            return Err(format!("pane {index} changed generation"));
        }
        if !rect_contains(root, pane.rect) {
            return Err(format!("pane {index} is outside the root"));
        }
        if pane.rect.rows > MAX_PANE_ROWS || pane.rect.columns > MAX_PANE_COLUMNS {
            return Err(format!("pane {index} exceeds the maximum pane size"));
        }
        if content_rect(pane.rect).is_none() {
            return Err(format!("pane {index} is below the minimum content size"));
        }
        for (other_index, other) in panes.iter().enumerate().skip(index + 1) {
            if identity.pane == other.target.pane {
                return Err(format!("panes {index} and {other_index} share a pane id"));
            }
            if identity.session == other.target.session {
                return Err(format!(
                    "panes {index} and {other_index} share a session id"
                ));
            }
            if rects_intersect(pane.rect, other.rect) {
                return Err(format!("panes {index} and {other_index} overlap"));
            }
        }
    }

    let expected_dividers = panes.len() - 1;
    for (index, slot) in snapshot.dividers.iter().enumerate() {
        if slot.is_some() != (index < expected_dividers) {
            return Err(String::from("divider slots are not densely packed"));
        }
    }
    if snapshot.divider != snapshot.dividers[0] {
        return Err(String::from(
            "legacy divider field disagrees with dividers[0]",
        ));
    }

    for (index, divider) in snapshot.dividers.iter().flatten().enumerate() {
        let one_cell_thick = (divider.rows == 1 && divider.columns >= 1)
            || (divider.columns == 1 && divider.rows >= 1);
        if !one_cell_thick {
            return Err(format!("divider {index} is not a one-cell strip"));
        }
        if !rect_contains(root, *divider) {
            return Err(format!("divider {index} is outside the root"));
        }
    }

    let mut rects: Vec<Rect> = pane_rects(snapshot);
    rects.extend(snapshot.dividers.iter().flatten().copied());
    for (index, first) in rects.iter().enumerate() {
        for second in rects.iter().skip(index + 1) {
            if rects_intersect(*first, *second) {
                return Err(String::from("pane and divider rectangles overlap"));
            }
        }
    }
    let covered: u32 = rects.iter().map(|rect| rect_area(*rect)).sum();
    if covered != rect_area(root) {
        return Err(format!(
            "rectangles cover {covered} cells of {}",
            rect_area(root)
        ));
    }

    Ok(())
}

fn check_layout(layout: &PaneLayout) -> Result<(), String> {
    let snapshot = layout.snapshot();
    check_snapshot(snapshot)?;

    for pane in current_panes(snapshot) {
        if layout.hit_test(pane.rect.x, pane.rect.y) != Some(pane) {
            return Err(String::from("hit_test disagrees at a pane origin"));
        }
        let last_column = pane.rect.x + pane.rect.columns - 1;
        let last_row = pane.rect.y + pane.rect.rows - 1;
        if layout.hit_test(last_column, last_row) != Some(pane) {
            return Err(String::from("hit_test disagrees at a pane corner"));
        }
    }
    for divider in snapshot.dividers.iter().flatten() {
        if layout.hit_test(divider.x, divider.y).is_some() {
            return Err(String::from(
                "hit_test claims a divider cell belongs to a pane",
            ));
        }
    }
    if layout.hit_test(snapshot.columns, 0).is_some() || layout.hit_test(0, snapshot.rows).is_some()
    {
        return Err(String::from("hit_test claims a cell outside the root"));
    }

    let rebuilt = PaneLayout::from_snapshot(snapshot)
        .map_err(|error| format!("from_snapshot rejected a live layout: {error:?}"))?;
    if rebuilt.snapshot() != snapshot {
        return Err(String::from(
            "from_snapshot did not round-trip the snapshot",
        ));
    }
    Ok(())
}

fn expect_valid(layout: &PaneLayout, context: &str) -> Result<(), TestCaseError> {
    match check_layout(layout) {
        Ok(()) => Ok(()),
        Err(message) => Err(TestCaseError::fail(format!("{context}: {message}"))),
    }
}

/// A rejection must be one of the documented classes and must leave the entire
/// observable snapshot untouched.
fn expect_rejected(
    layout: &PaneLayout,
    before: LayoutSnapshot,
    error: LayoutError,
    allowed: &[LayoutError],
    context: &str,
) -> Result<(), TestCaseError> {
    if !allowed.contains(&error) {
        return Err(TestCaseError::fail(format!(
            "{context}: unexpected rejection class {error:?}"
        )));
    }
    prop_assert_eq!(
        layout.snapshot(),
        before,
        "{}: a rejected operation changed observable state",
        context
    );
    Ok(())
}

const SPLIT_ERRORS: &[LayoutError] = &[
    LayoutError::StaleTarget,
    LayoutError::StaleRevision,
    LayoutError::PaneLimitExceeded,
    LayoutError::InvalidTarget,
    LayoutError::TargetIdentityConflict,
    LayoutError::TargetScopeMismatch,
    LayoutError::LayoutTooSmall,
    LayoutError::LayoutTooLarge,
    LayoutError::RevisionExhausted,
    LayoutError::InvalidLayoutState,
];

const RESIZE_ERRORS: &[LayoutError] = &[
    LayoutError::StaleRevision,
    LayoutError::InvalidDimensions,
    LayoutError::LayoutTooSmall,
    LayoutError::LayoutTooLarge,
    LayoutError::RevisionExhausted,
    LayoutError::InvalidLayoutState,
];

const CLOSE_ERRORS: &[LayoutError] = &[
    LayoutError::StaleTarget,
    LayoutError::StaleRevision,
    LayoutError::InvalidLayoutState,
    LayoutError::RevisionExhausted,
];

const SWAP_ERRORS: &[LayoutError] = &[
    LayoutError::StaleTarget,
    LayoutError::StaleRevision,
    LayoutError::RevisionExhausted,
    LayoutError::InvalidLayoutState,
];

const DIVIDER_ERRORS: &[LayoutError] = &[
    LayoutError::StaleTarget,
    LayoutError::StaleRevision,
    LayoutError::InvalidDividerIndex,
    LayoutError::DividerNotAncestor,
    LayoutError::LayoutTooSmall,
    LayoutError::LayoutTooLarge,
    LayoutError::RevisionExhausted,
    LayoutError::InvalidLayoutState,
];

const REVISION_ERRORS: &[LayoutError] = &[
    LayoutError::StaleRevision,
    LayoutError::RevisionExhausted,
    LayoutError::InvalidLayoutState,
];

fn apply_op(layout: &mut PaneLayout, op: &LayoutOp, index: usize) -> Result<(), TestCaseError> {
    let before = layout.snapshot();
    match op {
        LayoutOp::Split {
            pane_sel,
            direction,
            revision_mode,
            identity_mode,
            commit_mode,
        } => {
            let direction = pick_direction(*direction);
            let target = pick_target(before, *pane_sel);
            let expected_revision = pick_revision(before, *revision_mode);
            let fresh = fresh_target(index);
            let new_target = match identity_mode % 4 {
                0 => fresh,
                1 => {
                    let panes = current_panes(before);
                    TargetRef {
                        pane: panes[usize::from(*pane_sel) % panes.len()].target.pane,
                        ..fresh
                    }
                }
                2 => TargetRef {
                    generation: fresh.generation + 1,
                    ..fresh
                },
                _ => TargetRef { pane: 0, ..fresh },
            };

            let can_split = layout.can_split(target, direction);
            match layout.preview_split(target, expected_revision, direction, new_target) {
                Err(error) => {
                    expect_rejected(layout, before, error, SPLIT_ERRORS, "split preview")?;
                }
                Ok(preview) => {
                    prop_assert_eq!(
                        can_split,
                        Ok(()),
                        "can_split disagreed with a valid preview"
                    );
                    let planned = preview.planned_snapshot();
                    prop_assert_eq!(planned.revision, before.revision + 1);
                    prop_assert_eq!(planned.pane_count(), before.pane_count() + 1);

                    let old_pane = current_panes(before)
                        .into_iter()
                        .find(|pane| pane.target == target)
                        .expect("preview requires a current pane");
                    let expected_rect = predicted_split_rect(old_pane.rect, direction);
                    let planned_new = current_panes(planned)
                        .into_iter()
                        .find(|pane| pane.target == new_target)
                        .expect("planned snapshot contains the new pane");
                    prop_assert_eq!(planned_new.rect, expected_rect);
                    prop_assert_eq!(
                        split_geometry(old_pane.rect, direction),
                        Ok((expected_rect.rows, expected_rect.columns))
                    );
                    let mut planned_targets = pane_targets(planned);
                    let position = planned_targets
                        .iter()
                        .position(|candidate| *candidate == new_target)
                        .expect("planned snapshot contains the new target");
                    planned_targets.remove(position);
                    prop_assert!(multiset_eq(&planned_targets, &pane_targets(before)));

                    if commit_mode % 2 == 0 {
                        let committed =
                            layout.commit_split(preview).expect("fresh preview commits");
                        prop_assert_eq!(committed, planned);
                        let committed_new = current_panes(committed)
                            .into_iter()
                            .find(|pane| pane.target == new_target)
                            .expect("committed snapshot contains the new pane");
                        prop_assert_eq!(committed_new.rect, expected_rect);
                    } else {
                        let disturbed = layout
                            .advance_revision(before.revision)
                            .expect("current revision advances");
                        prop_assert_eq!(disturbed.revision, before.revision + 1);
                        prop_assert_eq!(
                            layout.commit_split(preview),
                            Err(LayoutError::PreviewStale),
                            "a stale preview must not commit"
                        );
                        prop_assert_eq!(layout.snapshot(), disturbed);
                        prop_assert_eq!(pane_targets(layout.snapshot()), pane_targets(before));
                    }
                }
            }
        }
        LayoutOp::Resize {
            rows,
            columns,
            revision_mode,
        } => {
            let expected_revision = pick_revision(before, *revision_mode);
            match layout.resize(expected_revision, *rows, *columns) {
                Err(error) => {
                    expect_rejected(layout, before, error, RESIZE_ERRORS, "resize")?;
                }
                Ok(after) => {
                    prop_assert_eq!(after, layout.snapshot());
                    prop_assert_eq!(pane_targets(after), pane_targets(before));
                    if *rows == before.rows && *columns == before.columns {
                        prop_assert_eq!(after, before, "resize to identical dimensions is a no-op");
                    } else {
                        prop_assert_eq!(after.revision, before.revision + 1);
                        prop_assert_eq!(after.rows, *rows);
                        prop_assert_eq!(after.columns, *columns);
                    }
                }
            }
        }
        LayoutOp::Close {
            pane_sel,
            revision_mode,
        } => {
            let removed = pick_target(before, *pane_sel);
            let expected_revision = pick_revision(before, *revision_mode);
            match layout.collapse(removed, expected_revision) {
                Err(error) => {
                    expect_rejected(layout, before, error, CLOSE_ERRORS, "close")?;
                }
                Ok(after) => {
                    prop_assert_eq!(after, layout.snapshot());
                    prop_assert_eq!(after.revision, before.revision + 1);
                    prop_assert_eq!(after.pane_count(), before.pane_count() - 1);
                    let expected: Vec<_> = pane_targets(before)
                        .into_iter()
                        .filter(|target| *target != removed)
                        .collect();
                    prop_assert_eq!(pane_targets(after), expected);
                }
            }
        }
        LayoutOp::Swap {
            first_sel,
            second_sel,
            revision_mode,
        } => {
            let first = pick_target(before, *first_sel);
            let second = pick_target(before, *second_sel);
            let expected_revision = pick_revision(before, *revision_mode);
            match layout.swap(first, second, expected_revision) {
                Err(error) => {
                    expect_rejected(layout, before, error, SWAP_ERRORS, "swap")?;
                }
                Ok(after) => {
                    prop_assert_eq!(after, layout.snapshot());
                    if first == second {
                        prop_assert_eq!(after, before, "swapping one pane is a no-op");
                    } else {
                        prop_assert_eq!(after.revision, before.revision + 1);
                        prop_assert!(multiset_eq(&pane_targets(after), &pane_targets(before)));
                        prop_assert!(multiset_eq(&pane_rects(after), &pane_rects(before)));
                    }
                }
            }
        }
        LayoutOp::SetDivider {
            pane_sel,
            divider_index,
            span_mode,
            revision_mode,
        } => {
            let target = pick_target(before, *pane_sel);
            let expected_revision = pick_revision(before, *revision_mode);
            let index = divider_index % (MAX_PANES as u8 - 1);
            let info = layout.divider_info(target, expected_revision, index);
            let first_cells = match info {
                Ok(info) => match span_mode % 5 {
                    0 => 0,
                    1 => info.first_span,
                    2 => info.total_cells,
                    3 => info.total_cells / 2,
                    _ => info.first_span.saturating_add(1),
                },
                Err(_) => 0,
            };

            match layout.set_divider(target, expected_revision, index, first_cells) {
                Err(error) => {
                    prop_assert!(
                        info.is_err()
                            || first_cells == 0
                            || first_cells >= info.unwrap().total_cells
                            || error == LayoutError::LayoutTooSmall
                            || error == LayoutError::InvalidLayoutState,
                        "unexpected divider rejection {:?} for span {}",
                        error,
                        first_cells
                    );
                    expect_rejected(layout, before, error, DIVIDER_ERRORS, "set_divider")?;
                }
                Ok(after) => {
                    let info = info.expect("an accepted divider must have been resolvable");
                    prop_assert_eq!(after, layout.snapshot());
                    prop_assert_eq!(pane_targets(after), pane_targets(before));
                    if first_cells == info.first_span {
                        prop_assert_eq!(after, before, "an unchanged span is a no-op");
                    } else {
                        prop_assert_eq!(after.revision, before.revision + 1);
                        let updated = layout
                            .divider_info(target, after.revision, index)
                            .expect("the resized divider stays resolvable");
                        prop_assert_eq!(updated.index, index);
                        prop_assert_eq!(updated.first_span, first_cells);
                        prop_assert_eq!(updated.total_cells, info.total_cells);
                    }
                }
            }
        }
        LayoutOp::AdvanceRevision { revision_mode } => {
            let expected_revision = pick_revision(before, *revision_mode);
            match layout.advance_revision(expected_revision) {
                Err(error) => {
                    expect_rejected(layout, before, error, REVISION_ERRORS, "advance_revision")?;
                }
                Ok(after) => {
                    prop_assert_eq!(after, layout.snapshot());
                    prop_assert_eq!(after.revision, before.revision + 1);
                    prop_assert_eq!(after.panes, before.panes);
                    prop_assert_eq!(after.dividers, before.dividers);
                }
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// Random bounded operation sequences keep every layout invariant, keep
    /// rejected operations atomic, and keep the structural rules exact.
    #[test]
    fn operation_sequences_preserve_layout_invariants(
        rows in MIN_PANE_ROWS..=MAX_PANE_ROWS,
        columns in 45u16..=MAX_PANE_COLUMNS,
        ops in arb_sequence(),
    ) {
        let mut layout = PaneLayout::new(target(41, 42), rows, columns).expect("valid root");
        expect_valid(&layout, "initial layout")?;

        for (index, op) in ops.iter().enumerate() {
            apply_op(&mut layout, op, index)?;
            expect_valid(&layout, &format!("after operation {index}: {op:?}"))?;
        }
    }

    /// The same sequence harness over roots that may be too narrow to split, so
    /// undersized-geometry rejections are exercised deliberately.
    #[test]
    fn narrow_roots_reject_operations_atomically(
        rows in arb_rows(),
        columns in arb_columns(),
        ops in arb_sequence(),
    ) {
        let mut layout = match PaneLayout::new(target(7, 8), rows, columns) {
            Ok(layout) => layout,
            Err(error) => {
                prop_assert_eq!(error, LayoutError::InvalidDimensions);
                return Ok(());
            }
        };
        expect_valid(&layout, "initial layout")?;
        for (index, op) in ops.iter().enumerate() {
            apply_op(&mut layout, op, index)?;
            expect_valid(&layout, &format!("after operation {index}: {op:?}"))?;
        }
    }

    /// Root admission is exactly the documented inclusive range.
    #[test]
    fn root_dimension_admission_is_exact(rows in arb_rows(), columns in arb_columns()) {
        let rows_ok = (MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rows);
        let columns_ok = (MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns);
        match PaneLayout::new(target(1, 2), rows, columns) {
            Ok(layout) => {
                prop_assert!(rows_ok && columns_ok);
                let snapshot = layout.snapshot();
                prop_assert_eq!(snapshot.revision, 1);
                prop_assert_eq!(snapshot.rows, rows);
                prop_assert_eq!(snapshot.columns, columns);
                prop_assert_eq!(snapshot.pane_count(), 1);
                prop_assert_eq!(
                    snapshot.panes[0],
                    Some(PaneGeometry {
                        target: target(1, 2),
                        rect: Rect { x: 0, y: 0, rows, columns },
                    })
                );
                prop_assert_eq!(snapshot.divider, None);
                expect_valid(&layout, "admitted root")?;
            }
            Err(error) => {
                prop_assert!(!(rows_ok && columns_ok), "in-bounds root was rejected");
                prop_assert_eq!(error, LayoutError::InvalidDimensions);
            }
        }
    }

    /// A zero identity field in the initial target is rejected before any state
    /// exists.
    #[test]
    fn zero_target_fields_are_rejected(field in 0usize..5) {
        let mut values = [BASE_GENERATION, BASE_WORKSPACE, 3, 4, 5];
        values[field] = 0;
        let invalid = TargetRef {
            generation: values[0],
            workspace: values[1],
            pane: values[2],
            session: values[3],
            occupant_revision: values[4],
        };
        prop_assert_eq!(
            PaneLayout::new(invalid, MIN_PANE_ROWS, MIN_PANE_COLUMNS).map(|_| ()),
            Err(LayoutError::InvalidTarget)
        );
    }
}

// ---------------------------------------------------------------------------
// Deterministic companions
// ---------------------------------------------------------------------------

fn committed_split(
    layout: &mut PaneLayout,
    target: TargetRef,
    direction: SplitDirection,
    new_target: TargetRef,
) -> LayoutSnapshot {
    let revision = layout.snapshot().revision;
    let preview = layout
        .preview_split(target, revision, direction, new_target)
        .expect("deterministic split preview");
    layout.commit_split(preview).expect("deterministic commit")
}

/// A four-pane split tree is a complete partition, and the fifth split is
/// rejected at capacity without disturbing that partition.
#[test]
fn four_pane_layout_is_complete_and_at_capacity() {
    let first = target(41, 42);
    let second = target(43, 44);
    let third = target(45, 46);
    let fourth = target(47, 48);
    let mut layout = PaneLayout::new(first, 31, 122).expect("valid root");
    committed_split(&mut layout, first, SplitDirection::Right, second);
    committed_split(&mut layout, first, SplitDirection::Down, third);
    committed_split(&mut layout, second, SplitDirection::Down, fourth);

    let snapshot = layout.snapshot();
    assert_eq!(snapshot.pane_count(), 4);
    assert_eq!(snapshot.dividers.iter().flatten().count(), 3);
    assert_eq!(snapshot.revision, 4);
    check_layout(&layout).expect("four-pane layout satisfies every invariant");

    // Every pane occupies at least the minimum content rectangle.
    for pane in current_panes(snapshot) {
        assert!(content_rect(pane.rect).is_some());
    }

    let blocked = fresh_target(99);
    assert_eq!(
        layout.preview_split(first, snapshot.revision, SplitDirection::Right, blocked),
        Err(LayoutError::PaneLimitExceeded)
    );
    assert_eq!(layout.snapshot(), snapshot);
    assert_eq!(
        layout.can_split(first, SplitDirection::Right),
        Err(LayoutError::PaneLimitExceeded)
    );
}

/// Stale revisions and stale targets are rejected without changing any part of
/// the snapshot, including dividers and the legacy divider field.
#[test]
fn stale_mutations_are_rejected_atomically() {
    let mut layout = PaneLayout::new(target(1, 2), 31, 122).expect("valid root");
    committed_split(
        &mut layout,
        target(1, 2),
        SplitDirection::Right,
        target(3, 4),
    );
    let before = layout.snapshot();

    let stale = before.revision + 1;
    assert_eq!(
        layout.resize(stale, 40, 100),
        Err(LayoutError::StaleRevision)
    );
    assert_eq!(layout.snapshot(), before);

    assert_eq!(
        layout.preview_split(target(1, 2), stale, SplitDirection::Down, fresh_target(1)),
        Err(LayoutError::StaleRevision)
    );
    assert_eq!(layout.snapshot(), before);

    assert_eq!(
        layout.collapse(target(1, 2), stale),
        Err(LayoutError::StaleRevision)
    );
    assert_eq!(layout.snapshot(), before);

    assert_eq!(
        layout.swap(target(1, 2), target(3, 4), stale),
        Err(LayoutError::StaleRevision)
    );
    assert_eq!(layout.snapshot(), before);

    assert_eq!(
        layout.set_divider(target(3, 4), stale, 0, 10),
        Err(LayoutError::StaleRevision)
    );
    assert_eq!(layout.snapshot(), before);

    assert_eq!(
        layout.collapse(stale_target(1), before.revision),
        Err(LayoutError::StaleTarget)
    );
    assert_eq!(layout.snapshot(), before);
}

/// The final pane cannot be collapsed, and a no-op swap does not advance the
/// revision while an identity swap does.
#[test]
fn collapse_and_swap_revision_rules_are_exact() {
    let only = target(1, 2);
    let mut layout = PaneLayout::new(only, 31, 122).expect("valid root");
    let before = layout.snapshot();
    assert_eq!(
        layout.collapse(only, before.revision),
        Err(LayoutError::InvalidLayoutState)
    );
    assert_eq!(layout.snapshot(), before);
    assert_eq!(layout.swap(only, only, before.revision), Ok(before));
    assert_eq!(layout.snapshot(), before);

    let other = target(3, 4);
    committed_split(&mut layout, only, SplitDirection::Right, other);
    let split = layout.snapshot();
    let after_swap = layout.swap(only, other, split.revision).expect("swap");
    assert_eq!(after_swap.revision, split.revision + 1);
    assert!(multiset_eq(&pane_targets(after_swap), &pane_targets(split)));
    assert!(multiset_eq(&pane_rects(after_swap), &pane_rects(split)));
    check_layout(&layout).expect("swapped layout satisfies every invariant");

    let after_close = layout
        .collapse(other, after_swap.revision)
        .expect("collapse back to one pane");
    assert_eq!(after_close.pane_count(), 1);
    assert_eq!(after_close.dividers, [None; MAX_PANES - 1]);
    assert_eq!(after_close.divider, None);
    check_layout(&layout).expect("collapsed layout satisfies every invariant");
}

/// Rebuilding a layout from an invalid snapshot fails closed.
#[test]
fn from_snapshot_rejects_invalid_states() {
    let mut layout = PaneLayout::new(target(1, 2), 31, 122).expect("valid root");
    let snapshot = layout.snapshot();

    let zero_revision = LayoutSnapshot {
        revision: 0,
        ..snapshot
    };
    assert_eq!(
        PaneLayout::from_snapshot(zero_revision).map(|_| ()),
        Err(LayoutError::InvalidLayoutState)
    );

    let empty = LayoutSnapshot {
        panes: [None; MAX_PANES],
        ..snapshot
    };
    assert_eq!(
        PaneLayout::from_snapshot(empty).map(|_| ()),
        Err(LayoutError::InvalidLayoutState)
    );

    let bad_dimensions = LayoutSnapshot {
        rows: MIN_PANE_ROWS - 1,
        ..snapshot
    };
    assert_eq!(
        PaneLayout::from_snapshot(bad_dimensions).map(|_| ()),
        Err(LayoutError::InvalidDimensions)
    );

    let displaced = LayoutSnapshot {
        panes: [
            Some(PaneGeometry {
                target: target(1, 2),
                rect: Rect {
                    x: 1,
                    y: 0,
                    rows: 31,
                    columns: 122,
                },
            }),
            None,
            None,
            None,
        ],
        ..snapshot
    };
    assert_eq!(
        PaneLayout::from_snapshot(displaced).map(|_| ()),
        Err(LayoutError::InvalidLayoutState)
    );

    let rebuilt = PaneLayout::from_snapshot(snapshot).expect("live snapshot rebuilds");
    assert_eq!(rebuilt.snapshot(), snapshot);
    assert_eq!(layout.snapshot(), snapshot);
    layout
        .advance_revision(snapshot.revision)
        .expect("unchanged layout still advances");
}

#[test]
fn root_dimension_boundaries_are_deterministic() {
    for rows in [
        MIN_PANE_ROWS - 1,
        MIN_PANE_ROWS,
        MAX_PANE_ROWS,
        MAX_PANE_ROWS + 1,
    ] {
        for columns in [
            MIN_PANE_COLUMNS - 1,
            MIN_PANE_COLUMNS,
            MAX_PANE_COLUMNS,
            MAX_PANE_COLUMNS + 1,
        ] {
            let valid = (MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rows)
                && (MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns);
            assert_eq!(PaneLayout::new(target(1, 2), rows, columns).is_ok(), valid);
        }
    }
}
