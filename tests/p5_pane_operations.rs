use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{content_rect, PaneLayout, Rect};
use panelink::pane_operations_protocol::{decode_view, encode_view, PaneOperationsView};
use panelink::ui_protocol::{LayoutReply, UiPane};

fn target() -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 11,
        pane: 13,
        session: 17,
        occupant_revision: 19,
    }
}

#[test]
fn zoom_view_keeps_the_underlying_partition_and_explicit_actual_size() {
    let target = target();
    let snapshot = PaneLayout::new(target, 24, 80).unwrap().snapshot();
    let content = content_rect(Rect {
        x: 0,
        y: 0,
        rows: snapshot.rows,
        columns: snapshot.columns,
    })
    .unwrap();
    let view = PaneOperationsView {
        layout: LayoutReply {
            revision: snapshot.revision,
            rows: snapshot.rows,
            columns: snapshot.columns,
            panes: [
                Some(UiPane {
                    target,
                    rect: snapshot.panes[0].unwrap().rect,
                    pending_confirmed: None,
                }),
                None,
                None,
                None,
            ],
            divider: snapshot.divider,
            dividers: snapshot.dividers,
        },
        zoomed_target: Some(target),
        confirmed_sizes: [Some((content.rows, content.columns)), None, None, None],
    };
    let encoded = encode_view(&view).unwrap();
    assert_eq!(decode_view(&encoded).unwrap(), view);
}

#[test]
fn zoom_only_revision_preserves_the_entire_tree_and_checks_overflow() {
    let mut layout = PaneLayout::new(target(), 24, 80).unwrap();
    let before = layout.snapshot();
    let zoom_revision = layout.advance_revision(before.revision).unwrap();
    assert_eq!(zoom_revision.revision, before.revision + 1);
    assert_eq!(zoom_revision.rows, before.rows);
    assert_eq!(zoom_revision.columns, before.columns);
    assert_eq!(zoom_revision.panes, before.panes);
    assert_eq!(zoom_revision.dividers, before.dividers);
    assert!(layout.advance_revision(before.revision).is_err());
    assert_eq!(layout.snapshot(), zoom_revision);
}
