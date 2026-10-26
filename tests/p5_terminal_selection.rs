use panelink::control_protocol::TargetRef;
use panelink::terminal_selection::{
    begin, Cell, Point, Row, SelectionError, SoftWrap, ViewIdentity, MAX_SELECTION_BYTES,
};

fn identity(rows: u16, columns: u16) -> ViewIdentity {
    ViewIdentity {
        target: TargetRef {
            generation: 4,
            workspace: 2,
            pane: 7,
            session: 9,
            occupant_revision: 3,
        },
        view_id: 11,
        epoch: 12,
        rows,
        columns,
        buffer_id: 13,
    }
}

fn cells<'a>(texts: &'a [&'a str]) -> Vec<Cell<'a>> {
    texts
        .iter()
        .map(|text| Cell {
            text,
            wide_lead: false,
            continuation: false,
        })
        .collect()
}

#[test]
fn unicode_and_combining_text_are_preserved() {
    let row_cells = [
        Cell {
            text: "é",
            wide_lead: false,
            continuation: false,
        },
        Cell {
            text: "e\u{301}",
            wide_lead: false,
            continuation: false,
        },
        Cell {
            text: "🙂",
            wide_lead: false,
            continuation: false,
        },
    ];
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(identity(1, 3), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 0, column: 2 });
    assert_eq!(selection.finish(identity(1, 3)).unwrap(), "ée\u{301}🙂");
}

#[test]
fn soft_wrap_joins_rows_and_hard_boundary_adds_newline() {
    let first = cells(&["ab", "c", ""]);
    let second = cells(&["d", "e", ""]);
    let third = cells(&["f", "g", ""]);
    let rows = [
        Row {
            cells: &first,
            wrap_after: SoftWrap::Yes,
        },
        Row {
            cells: &second,
            wrap_after: SoftWrap::No,
        },
        Row {
            cells: &third,
            wrap_after: SoftWrap::Unknown,
        },
    ];
    let mut selection = begin(identity(3, 3), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 1, column: 1 });
    assert_eq!(selection.finish(identity(3, 3)).unwrap(), "abc de");

    let mut hard = begin(identity(3, 3), &rows, Point { row: 1, column: 0 }).unwrap();
    hard.extend(Point { row: 2, column: 1 });
    assert_eq!(hard.finish(identity(3, 3)).unwrap(), "de\nfg");
}

#[test]
fn reverse_drag_is_ordered_and_wide_character_is_copied_once() {
    let row_cells = [
        Cell {
            text: "A",
            wide_lead: false,
            continuation: false,
        },
        Cell {
            text: "界",
            wide_lead: true,
            continuation: false,
        },
        Cell {
            text: "",
            wide_lead: false,
            continuation: true,
        },
        Cell {
            text: "B",
            wide_lead: false,
            continuation: false,
        },
    ];
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut from_continuation = begin(identity(1, 4), &rows, Point { row: 0, column: 2 }).unwrap();
    from_continuation.extend(Point { row: 0, column: 1 });
    assert_eq!(from_continuation.finish(identity(1, 4)).unwrap(), "界");

    let mut reverse = begin(identity(1, 4), &rows, Point { row: 0, column: 3 }).unwrap();
    reverse.extend(Point { row: 0, column: 0 });
    assert_eq!(reverse.finish(identity(1, 4)).unwrap(), "A界B");
}

#[test]
fn click_is_empty_and_coordinates_clip_inside_the_captured_pane() {
    let row_cells = cells(&["a", "b", "c"]);
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(
        identity(1, 3),
        &rows,
        Point {
            row: 99,
            column: 99,
        },
    )
    .unwrap();
    assert_eq!(selection.finish(identity(1, 3)), Err(SelectionError::Empty));
    selection.extend(Point { row: 99, column: 0 });
    assert_eq!(selection.finish(identity(1, 3)).unwrap(), "abc");
}

#[test]
fn unavailable_wrap_metadata_fails_closed() {
    let first = cells(&["a"]);
    let second = cells(&["b"]);
    let rows = [
        Row {
            cells: &first,
            wrap_after: SoftWrap::Unknown,
        },
        Row {
            cells: &second,
            wrap_after: SoftWrap::No,
        },
    ];
    let mut selection = begin(identity(2, 1), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 1, column: 0 });
    assert_eq!(
        selection.finish(identity(2, 1)),
        Err(SelectionError::UnknownWrap)
    );
}

#[test]
fn target_geometry_epoch_and_buffer_changes_cancel_copy() {
    let row_cells = cells(&["a", "b"]);
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(identity(1, 2), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 0, column: 1 });
    let mut changed = identity(1, 2);
    changed.target.pane += 1;
    assert_eq!(selection.finish(changed), Err(SelectionError::StaleView));
    changed = identity(1, 2);
    changed.epoch += 1;
    assert_eq!(selection.finish(changed), Err(SelectionError::StaleView));
}

#[test]
fn every_view_identity_field_mismatch_cancels_copy() {
    let row_cells = cells(&["a", "b"]);
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(identity(1, 2), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 0, column: 1 });

    let mut mismatches = Vec::new();
    let mut changed = identity(1, 2);
    changed.target.generation += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.target.workspace += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.target.pane += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.target.session += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.target.occupant_revision += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.view_id += 1;
    mismatches.push(changed);
    let mut changed = identity(1, 2);
    changed.epoch += 1;
    mismatches.push(changed);
    mismatches.push(identity(2, 2));
    mismatches.push(identity(1, 3));
    let mut changed = identity(1, 2);
    changed.buffer_id += 1;
    mismatches.push(changed);

    for changed in mismatches {
        assert_eq!(selection.finish(changed), Err(SelectionError::StaleView));
    }
}

#[test]
fn frozen_source_survives_mutation_and_drop() {
    let mut selection = {
        let mut source = String::from("before");
        let row_cells = [
            Cell {
                text: &source,
                wide_lead: false,
                continuation: false,
            },
            Cell {
                text: "!",
                wide_lead: false,
                continuation: false,
            },
        ];
        let rows = [Row {
            cells: &row_cells,
            wrap_after: SoftWrap::No,
        }];
        let selection = begin(identity(1, 2), &rows, Point { row: 0, column: 0 }).unwrap();
        source.clear();
        source.push_str("after");
        selection
    };

    selection.extend(Point { row: 0, column: 1 });
    assert_eq!(selection.finish(identity(1, 2)).unwrap(), "before!");
}

#[test]
fn selected_spaces_survive_soft_wrap_but_padding_trims_at_hard_end() {
    let first = cells(&["hello", " ", ""]);
    let second = cells(&["world", "", ""]);
    let rows = [
        Row {
            cells: &first,
            wrap_after: SoftWrap::Yes,
        },
        Row {
            cells: &second,
            wrap_after: SoftWrap::No,
        },
    ];
    let mut selection = begin(identity(2, 3), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 1, column: 0 });
    assert_eq!(selection.finish(identity(2, 3)).unwrap(), "hello  world");

    let hard = cells(&["hello", " ", ""]);
    let rows = [Row {
        cells: &hard,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(identity(1, 3), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 0, column: 2 });
    assert_eq!(selection.finish(identity(1, 3)).unwrap(), "hello");
}

#[test]
fn concatenated_output_cap_counts_inserted_newlines() {
    let large = "x".repeat(MAX_SELECTION_BYTES - 1);
    let first = [Cell {
        text: &large,
        wide_lead: false,
        continuation: false,
    }];
    let second = cells(&["y"]);
    let rows = [
        Row {
            cells: &first,
            wrap_after: SoftWrap::No,
        },
        Row {
            cells: &second,
            wrap_after: SoftWrap::No,
        },
    ];
    let mut selection = begin(identity(2, 1), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 1, column: 0 });
    assert_eq!(
        selection.finish(identity(2, 1)),
        Err(SelectionError::TooLarge)
    );
}

#[test]
fn malformed_grid_controls_and_oversized_text_are_rejected() {
    let short = cells(&["x"]);
    let bad_rows = [Row {
        cells: &short,
        wrap_after: SoftWrap::No,
    }];
    assert!(matches!(
        begin(identity(1, 2), &bad_rows, Point { row: 0, column: 0 }),
        Err(SelectionError::InvalidShape)
    ));

    let control = [Cell {
        text: "x\u{1b}[31m",
        wide_lead: false,
        continuation: false,
    }];
    let control_rows = [Row {
        cells: &control,
        wrap_after: SoftWrap::No,
    }];
    assert!(matches!(
        begin(identity(1, 1), &control_rows, Point { row: 0, column: 0 }),
        Err(SelectionError::InvalidText)
    ));

    let oversized = "x".repeat(MAX_SELECTION_BYTES + 1);
    let huge_cell = [Cell {
        text: &oversized,
        wide_lead: false,
        continuation: false,
    }];
    let huge_rows = [Row {
        cells: &huge_cell,
        wrap_after: SoftWrap::No,
    }];
    assert!(matches!(
        begin(identity(1, 1), &huge_rows, Point { row: 0, column: 0 }),
        Err(SelectionError::TooLarge)
    ));
}

#[test]
fn malformed_wide_cell_topology_is_rejected() {
    let cases = [
        [
            Cell {
                text: "x",
                wide_lead: true,
                continuation: false,
            },
            Cell {
                text: "",
                wide_lead: false,
                continuation: false,
            },
        ],
        [
            Cell {
                text: "",
                wide_lead: false,
                continuation: true,
            },
            Cell {
                text: "x",
                wide_lead: false,
                continuation: false,
            },
        ],
        [
            Cell {
                text: "x",
                wide_lead: true,
                continuation: false,
            },
            Cell {
                text: "y",
                wide_lead: false,
                continuation: false,
            },
        ],
    ];
    for row_cells in cases {
        let rows = [Row {
            cells: &row_cells,
            wrap_after: SoftWrap::No,
        }];
        assert!(matches!(
            begin(identity(1, 2), &rows, Point { row: 0, column: 0 }),
            Err(SelectionError::InvalidShape)
        ));
    }
    let both_flags = [Cell {
        text: "x",
        wide_lead: true,
        continuation: true,
    }];
    let rows = [Row {
        cells: &both_flags,
        wrap_after: SoftWrap::No,
    }];
    assert!(matches!(
        begin(identity(1, 1), &rows, Point { row: 0, column: 0 }),
        Err(SelectionError::InvalidText)
    ));
}

#[test]
fn captured_selection_debug_never_contains_cell_text() {
    let row_cells = cells(&["private selection text", "x"]);
    let rows = [Row {
        cells: &row_cells,
        wrap_after: SoftWrap::No,
    }];
    let mut selection = begin(identity(1, 2), &rows, Point { row: 0, column: 0 }).unwrap();
    selection.extend(Point { row: 0, column: 1 });
    let debug = format!("{selection:?}");
    assert!(!debug.contains("private selection text"));
    assert!(debug.contains("<redacted>"));
}
