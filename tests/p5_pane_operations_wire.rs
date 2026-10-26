use panelink::control_protocol::TargetRef;
use panelink::handshake_protocol::{negotiate, Hello, CAPABILITY_PANE_OPERATIONS, CAPABILITY_READ};
use panelink::pane_layout::{Rect, MAX_PANES};
use panelink::pane_operations_protocol::{
    decode_request, decode_view, encode_request, encode_view, CodecError, PaneDividerSet,
    PaneOperationsView, PaneSwap, PaneZoomSet, Request, PANE_OPERATIONS_SCHEMA,
    PANE_OPERATIONS_VIEW_BYTES, PANE_SWAP_BYTES, PANE_TARGET_MUTATION_BYTES,
};
use panelink::protocol::OperationKey;
use panelink::ui_protocol::{
    decode_layout_reply, encode_layout_reply, encode_root_resize, LayoutReply, RootResize, UiPane,
    LAYOUT_REPLY_BYTES, LAYOUT_REPLY_SCHEMA, ROOT_RESIZE_BYTES,
};

fn key(sequence: u64) -> OperationKey {
    OperationKey {
        generation: 1,
        lease: 2,
        sequence,
    }
}

fn target(pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 9,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn two_pane_layout() -> LayoutReply {
    LayoutReply {
        revision: 4,
        rows: 24,
        columns: 80,
        panes: [
            Some(UiPane {
                target: target(10, 11),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: 24,
                    columns: 39,
                },
                pending_confirmed: None,
            }),
            Some(UiPane {
                target: target(12, 13),
                rect: Rect {
                    x: 40,
                    y: 0,
                    rows: 24,
                    columns: 40,
                },
                pending_confirmed: None,
            }),
            None,
            None,
        ],
        divider: Some(Rect {
            x: 39,
            y: 0,
            rows: 24,
            columns: 1,
        }),
        dividers: [
            Some(Rect {
                x: 39,
                y: 0,
                rows: 24,
                columns: 1,
            }),
            None,
            None,
        ],
    }
}

fn two_pane_view() -> PaneOperationsView {
    let mut layout = two_pane_layout();
    // The first pane's confirmed size is the full-root zoom size, while its
    // legacy pending_confirmed remains explicitly relative to its old rect.
    layout.panes[0].as_mut().unwrap().pending_confirmed = Some((22, 78));
    PaneOperationsView {
        layout,
        zoomed_target: Some(target(10, 11)),
        confirmed_sizes: [Some((22, 78)), Some((22, 38)), None, None],
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn request_round_trips_and_swap_matches_schema_13_golden() {
    let swap = Request::Swap(PaneSwap {
        key: key(3),
        expected_layout_revision: 4,
        source: target(10, 11),
        destination: target(12, 13),
    });
    let bytes = encode_request(&swap).unwrap();
    assert_eq!(bytes.len(), PANE_SWAP_BYTES);
    assert_eq!(
        hex(&bytes),
        concat!(
            "000d0002",
            "0000000000000001",
            "0000000000000002",
            "0000000000000003",
            "0000000000000004",
            "0000000000000001",
            "0000000000000009",
            "000000000000000a",
            "000000000000000b",
            "0000000000000001",
            "0000000000000001",
            "0000000000000009",
            "000000000000000c",
            "000000000000000d",
            "0000000000000001"
        )
    );
    assert_eq!(decode_request(&bytes), Ok(swap));

    let query = encode_request(&Request::ViewQuery).unwrap();
    assert_eq!(query, [0, 13, 0, 1, 0, 0, 0, 0]);
    assert_eq!(decode_request(&query), Ok(Request::ViewQuery));

    for request in [
        Request::DividerSet(PaneDividerSet {
            key: key(5),
            expected_layout_revision: 4,
            target: target(10, 11),
            divider_index: 0,
            first_cells: 20,
        }),
        Request::ZoomSet(PaneZoomSet {
            key: key(6),
            expected_layout_revision: 4,
            target: target(10, 11),
            zoomed: true,
        }),
    ] {
        let bytes = encode_request(&request).unwrap();
        assert_eq!(bytes.len(), PANE_TARGET_MUTATION_BYTES);
        assert_eq!(decode_request(&bytes), Ok(request));
    }
}

#[test]
fn malformed_requests_fail_closed_for_lengths_headers_identifiers_and_bounds() {
    let swap = Request::Swap(PaneSwap {
        key: key(3),
        expected_layout_revision: 4,
        source: target(10, 11),
        destination: target(12, 13),
    });
    let valid = encode_request(&swap).unwrap();
    assert_eq!(
        decode_request(&valid[..valid.len() - 1]),
        Err(CodecError::Truncated)
    );
    let mut trailing = valid.clone();
    trailing.push(0);
    assert_eq!(decode_request(&trailing), Err(CodecError::TrailingBytes));
    let mut bad_schema = valid.clone();
    bad_schema[1] = 12;
    assert_eq!(
        decode_request(&bad_schema),
        Err(CodecError::UnsupportedSchema)
    );
    let mut bad_kind = valid.clone();
    bad_kind[3] = 9;
    assert_eq!(decode_request(&bad_kind), Err(CodecError::UnknownKind));
    let mut zero_key = valid.clone();
    zero_key[11] = 0;
    assert_eq!(decode_request(&zero_key), Err(CodecError::ZeroKeyComponent));
    let mut zero_revision = valid.clone();
    zero_revision[35] = 0;
    assert_eq!(
        decode_request(&zero_revision),
        Err(CodecError::ZeroRevision)
    );
    let mut zero_target = valid.clone();
    zero_target[59] = 0;
    assert_eq!(decode_request(&zero_target), Err(CodecError::InvalidTarget));
    let mut wrong_generation = valid.clone();
    wrong_generation[79] = 2;
    assert_eq!(
        decode_request(&wrong_generation),
        Err(CodecError::GenerationMismatch)
    );
    let mut same_target = valid.clone();
    same_target[92..116].copy_from_slice(&valid[52..76]);
    assert_eq!(
        decode_request(&same_target),
        Err(CodecError::DuplicateTarget)
    );
    let mut other_workspace = valid.clone();
    other_workspace[91] = 8;
    assert_eq!(
        decode_request(&other_workspace),
        Err(CodecError::WorkspaceMismatch)
    );

    let divider = Request::DividerSet(PaneDividerSet {
        key: key(5),
        expected_layout_revision: 4,
        target: target(10, 11),
        divider_index: 0,
        first_cells: 20,
    });
    let valid_divider = encode_request(&divider).unwrap();
    let mut bad_index = valid_divider.clone();
    bad_index[76] = 3;
    assert_eq!(
        decode_request(&bad_index),
        Err(CodecError::InvalidDividerIndex)
    );
    let mut bad_span = valid_divider.clone();
    bad_span[77..79].copy_from_slice(&240_u16.to_be_bytes());
    assert_eq!(
        decode_request(&bad_span),
        Err(CodecError::InvalidDividerPosition)
    );
    let mut bad_reserved = valid_divider.clone();
    bad_reserved[79] = 1;
    assert_eq!(
        decode_request(&bad_reserved),
        Err(CodecError::NonzeroReserved)
    );

    let zoom = Request::ZoomSet(PaneZoomSet {
        key: key(6),
        expected_layout_revision: 4,
        target: target(10, 11),
        zoomed: true,
    });
    let valid_zoom = encode_request(&zoom).unwrap();
    let mut bad_boolean = valid_zoom.clone();
    bad_boolean[76] = 2;
    assert_eq!(
        decode_request(&bad_boolean),
        Err(CodecError::InvalidBoolean)
    );
    let mut bad_zoom_reserved = valid_zoom;
    bad_zoom_reserved[78] = 1;
    assert_eq!(
        decode_request(&bad_zoom_reserved),
        Err(CodecError::NonzeroReserved)
    );
}

#[test]
fn authoritative_view_round_trips_and_rejects_inconsistent_explicit_state() {
    let view = two_pane_view();
    let bytes = encode_view(&view).unwrap();
    assert_eq!(bytes.len(), PANE_OPERATIONS_VIEW_BYTES);
    assert_eq!(&bytes[0..4], &[0, 13, 0, 5]);
    assert_eq!(&bytes[4..428], &encode_layout_reply(&view.layout).unwrap());
    assert_eq!(decode_view(&bytes), Ok(view));

    let mut bad_presence = bytes;
    bad_presence[428] = 2;
    assert_eq!(decode_view(&bad_presence), Err(CodecError::InvalidBoolean));
    let mut absent_target_not_zero = encode_view(&PaneOperationsView {
        layout: two_pane_layout(),
        zoomed_target: None,
        confirmed_sizes: [Some((22, 37)), Some((22, 38)), None, None],
    })
    .unwrap();
    absent_target_not_zero[432] = 1;
    assert_eq!(
        decode_view(&absent_target_not_zero),
        Err(CodecError::NonzeroReserved)
    );

    let mut missing_size = view;
    missing_size.confirmed_sizes[0] = None;
    assert_eq!(encode_view(&missing_size), Err(CodecError::InvalidView));
    let mut mismatch = view;
    mismatch.confirmed_sizes[0] = Some((22, 37));
    assert_eq!(encode_view(&mismatch), Err(CodecError::InvalidView));
    let mut wrong_zoom_target = view;
    wrong_zoom_target.zoomed_target = Some(target(90, 91));
    assert_eq!(
        encode_view(&wrong_zoom_target),
        Err(CodecError::InvalidView)
    );
}

#[test]
fn legacy_layout_and_root_resize_codecs_keep_their_fixed_records() {
    let layout = two_pane_layout();
    let layout_bytes = encode_layout_reply(&layout).unwrap();
    assert_eq!(layout_bytes.len(), 424);
    assert_eq!(LAYOUT_REPLY_BYTES, 424);
    assert_eq!(
        &layout_bytes[0..4],
        &[0, LAYOUT_REPLY_SCHEMA as u8, 0x80, 1]
    );
    assert_eq!(decode_layout_reply(&layout_bytes), Ok(layout));

    let resize = RootResize {
        key: key(3),
        expected_layout_revision: 4,
        rows: 24,
        columns: 80,
    };
    let resize_bytes = encode_root_resize(&resize).unwrap();
    assert_eq!(resize_bytes.len(), 40);
    assert_eq!(ROOT_RESIZE_BYTES, 40);
    assert_eq!(
        hex(&resize_bytes),
        concat!(
            "00080004",
            "0000000000000001",
            "0000000000000002",
            "0000000000000003",
            "0000000000000004",
            "0018",
            "0050"
        )
    );
}

#[test]
fn pane_operation_capability_is_selected_only_when_offered_and_available() {
    let hello = Hello {
        nonce: 1,
        required_capabilities: CAPABILITY_READ,
        optional_capabilities: CAPABILITY_PANE_OPERATIONS,
    };
    let selected = negotiate(&hello, CAPABILITY_READ | CAPABILITY_PANE_OPERATIONS, 1, 2).unwrap();
    assert_eq!(
        selected.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_PANE_OPERATIONS
    );

    let unoffered = Hello {
        optional_capabilities: 0,
        ..hello
    };
    let not_selected = negotiate(
        &unoffered,
        CAPABILITY_READ | CAPABILITY_PANE_OPERATIONS,
        1,
        2,
    )
    .unwrap();
    assert_eq!(not_selected.selected_capabilities, CAPABILITY_READ);
    assert_eq!(PANE_OPERATIONS_SCHEMA, 13);
    assert_eq!(MAX_PANES, 4);
}

#[test]
fn target_and_operation_identifiers_are_redacted_from_debug_output() {
    let request = Request::Swap(PaneSwap {
        key: key(3),
        expected_layout_revision: 4,
        source: target(10, 11),
        destination: target(12, 13),
    });
    let rendered = format!("{request:?}");
    for secret in [
        "generation",
        "workspace",
        "occupant_revision",
        "lease",
        "sequence",
    ] {
        assert!(!rendered.contains(secret));
    }
    assert!(rendered.contains("<redacted>"));
}
