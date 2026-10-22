use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::recovery_view::{
    plan_viewport, DisplayPlan, OverviewLabel, RecoverySlot, RecoveryView,
};
use panelink::saved_layout::{LayoutAxis, LayoutMetadata, SavedLayout};

fn target(pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 8,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn saved(axis: LayoutAxis, rows: u16, columns: u16, pane_count: usize) -> SavedLayout {
    let metadata = LayoutMetadata::new(7, 8, 1, 1, 10).unwrap();
    if pane_count == 0 {
        return SavedLayout::empty(metadata);
    }
    let (panes, divider) = match (axis, pane_count) {
        (LayoutAxis::Single, 1) => (
            [
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns,
                    },
                }),
                None,
            ],
            None,
        ),
        (LayoutAxis::SideBySide, 2) => {
            let first = (columns - 1) / 2;
            let second = columns - first - 1;
            (
                [
                    Some(PaneGeometry {
                        target: target(1, 1),
                        rect: Rect {
                            x: 0,
                            y: 0,
                            rows,
                            columns: first,
                        },
                    }),
                    Some(PaneGeometry {
                        target: target(2, 2),
                        rect: Rect {
                            x: first + 1,
                            y: 0,
                            rows,
                            columns: second,
                        },
                    }),
                ],
                Some(Rect {
                    x: first,
                    y: 0,
                    rows,
                    columns: 1,
                }),
            )
        }
        (LayoutAxis::Stacked, 2) => {
            let first = (rows - 1) / 2;
            let second = rows - first - 1;
            (
                [
                    Some(PaneGeometry {
                        target: target(1, 1),
                        rect: Rect {
                            x: 0,
                            y: 0,
                            rows: first,
                            columns,
                        },
                    }),
                    Some(PaneGeometry {
                        target: target(2, 2),
                        rect: Rect {
                            x: 0,
                            y: first + 1,
                            rows: second,
                            columns,
                        },
                    }),
                ],
                Some(Rect {
                    x: 0,
                    y: first,
                    rows: 1,
                    columns,
                }),
            )
        }
        _ => panic!("invalid test layout"),
    };
    SavedLayout::from_snapshot(
        metadata,
        &LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes,
            divider,
        },
    )
    .unwrap()
}

fn view(axis: LayoutAxis, rows: u16, columns: u16, panes: usize) -> RecoveryView {
    RecoveryView::from_layout(saved(axis, rows, columns, panes))
}

#[test]
fn from_layout_preserves_historical_spatial_order_and_empty_tombstone() {
    let split = view(LayoutAxis::SideBySide, 11, 82, 2);
    assert_eq!(split.panes.len(), 2);
    assert_eq!(split.panes[0].historical_target, target(1, 1));
    assert_eq!(split.panes[1].historical_target, target(2, 2));
    assert_eq!(split.axis, LayoutAxis::SideBySide);
    assert_eq!(
        split.clone().in_slot(RecoverySlot::Previous).slot,
        RecoverySlot::Previous
    );

    let empty = view(LayoutAxis::Single, 0, 0, 0);
    assert!(empty.panes.is_empty());
}

#[test]
fn regular_two_pane_plans_keep_divider_order_and_floor_rounding() {
    for (columns, first, second) in [(81, 40, 40), (82, 40, 41)] {
        let plan = plan_viewport(&view(LayoutAxis::SideBySide, 11, 82, 2), 11, columns, 0);
        let DisplayPlan::Panes { panes, divider, .. } = plan else {
            panic!("expected regular side-by-side plan")
        };
        assert_eq!(panes[0].columns, first);
        assert_eq!(panes[1].columns, second);
        assert_eq!(divider.unwrap().columns, 1);
        assert_eq!(panes[1].x, first + 1);
    }

    let plan = plan_viewport(&view(LayoutAxis::Stacked, 11, 82, 2), 11, 82, 0);
    let DisplayPlan::Panes { panes, divider, .. } = plan else {
        panic!("expected regular stacked plan")
    };
    assert_eq!((panes[0].rows, panes[1].rows), (5, 5));
    assert_eq!(panes[1].y, 6);
    assert_eq!(divider.unwrap().rows, 1);
}

#[test]
fn exact_minimum_leaves_fit_and_one_cell_short_uses_overview() {
    let fits = plan_viewport(&view(LayoutAxis::SideBySide, 5, 41, 2), 5, 41, 0);
    assert!(matches!(fits, DisplayPlan::Panes { .. }));

    let narrow = plan_viewport(&view(LayoutAxis::SideBySide, 11, 82, 2), 5, 40, 0);
    assert!(matches!(narrow, DisplayPlan::Overview { .. }));
    let short = plan_viewport(&view(LayoutAxis::Stacked, 11, 82, 2), 10, 82, 0);
    assert!(matches!(short, DisplayPlan::Overview { .. }));
}

#[test]
fn compact_hosts_get_proportional_labeled_overview_not_too_small() {
    for (rows, columns) in [(3, 10), (10, 40), (40, 10)] {
        let plan = plan_viewport(&view(LayoutAxis::SideBySide, 11, 82, 2), rows, columns, 0);
        let DisplayPlan::Overview {
            viewport, boxes, ..
        } = plan
        else {
            panic!("host at or above outline minimum must get overview")
        };
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].label, OverviewLabel::PaneOne);
        assert_eq!(boxes[1].label, OverviewLabel::PaneTwo);
        assert!(boxes.iter().all(|item| {
            item.rect.rows <= viewport.rows && item.rect.columns <= viewport.columns
        }));
    }
}

#[test]
fn tiny_host_and_chrome_underflow_are_bounded() {
    assert_eq!(
        plan_viewport(&view(LayoutAxis::SideBySide, 11, 82, 2), 2, 9, 0),
        DisplayPlan::TooSmall
    );
    assert_eq!(
        plan_viewport(&view(LayoutAxis::Stacked, 11, 82, 2), 0, 0, 0),
        DisplayPlan::TooSmall
    );
    assert_eq!(
        plan_viewport(&view(LayoutAxis::Stacked, 11, 82, 2), 12, 40, 10),
        DisplayPlan::TooSmall
    );
    assert_eq!(
        plan_viewport(&view(LayoutAxis::Single, 8, 24, 1), 1, 20, 2),
        DisplayPlan::TooSmall
    );
}

#[test]
fn chrome_is_subtracted_and_oversized_hosts_are_clamped_to_supported_geometry() {
    let plan = plan_viewport(&view(LayoutAxis::Single, 8, 24, 1), 90, 300, 2);
    let DisplayPlan::Panes {
        viewport, panes, ..
    } = plan
    else {
        panic!("expected one-pane plan")
    };
    assert_eq!(viewport.rows, 80);
    assert_eq!((panes[0].rows, panes[0].columns), (80, 240));

    let split = plan_viewport(&view(LayoutAxis::SideBySide, 11, 82, 2), 90, 300, 0);
    assert!(matches!(split, DisplayPlan::Panes { .. }));
}

#[test]
fn saved_dimensions_and_model_are_not_mutated_by_host_planning() {
    let model = view(LayoutAxis::SideBySide, 11, 82, 2);
    let before = model.clone();
    let _ = plan_viewport(&model, 7, 60, 2);
    assert_eq!(model, before);
    assert_eq!((model.saved_rows, model.saved_columns), (11, 82));
}
