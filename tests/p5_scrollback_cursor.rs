use vt100::{HistoryCursor, HistoryError, Parser};

fn cursor(parser: &Parser) -> HistoryCursor {
    parser.screen().history_cursor().unwrap()
}

fn texts(parser: &Parser, from: HistoryCursor) -> Vec<(u64, u16, bool, String)> {
    parser
        .screen()
        .history_after(from)
        .unwrap()
        .map(|row| {
            let text = (0..row.width)
                .filter_map(|col| row.cell(col))
                .map(vt100::Cell::contents)
                .collect();
            (row.ordinal, row.width, row.wrapped, text)
        })
        .collect()
}

#[test]
fn full_screen_scrolls_have_distinct_monotonic_ids_and_borrow_original_rows() {
    let mut parser = Parser::new(3, 10, 8);
    let initial = cursor(&parser);
    assert_eq!((initial.epoch, initial.ordinal), (1, 0));
    parser.process(b"one\r\ntwo\r\nthree\r\nfour");
    let records = texts(&parser, initial);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, 1);
    assert!(records[0].3.starts_with("one"));
    let accepted = cursor(&parser);
    assert!(texts(&parser, accepted).is_empty());
    parser.process(b"\x1b[2S");
    let records = texts(&parser, accepted);
    assert_eq!(records.iter().map(|row| row.0).collect::<Vec<_>>(), [2, 3]);
    assert!(records[0].3.starts_with("two"));
    assert!(records[1].3.starts_with("three"));
    let accepted = cursor(&parser);
    parser.process(b"\x1b[?1049hALT\r\nALT\r\nALT\r\nALT");
    assert_eq!(
        cursor(&parser),
        accepted,
        "alternate screen has no normal history"
    );
    assert!(texts(&parser, initial).iter().all(|row| row.1 == 10));
    parser.process(b"\x1b[?1049l");
    assert_eq!(cursor(&parser), accepted);
}

#[test]
fn eviction_reports_gap_and_reset_is_observable_even_with_no_new_rows() {
    let mut parser = Parser::new(2, 10, 2);
    let initial = cursor(&parser);
    parser.process(b"a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(cursor(&parser).ordinal, 3);
    assert!(matches!(
        parser.screen().history_after(initial),
        Err(HistoryError::Gap)
    ));
    let retained = HistoryCursor {
        ordinal: 1,
        ..initial
    };
    assert_eq!(
        texts(&parser, retained)
            .iter()
            .map(|row| row.0)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    let before_reset = cursor(&parser);
    parser.process(b"\x1bc");
    let after = cursor(&parser);
    assert_eq!((after.epoch, after.ordinal), (before_reset.epoch + 1, 0));
    assert!(matches!(
        parser.screen().history_after(before_reset),
        Err(HistoryError::Reset)
    ));
    assert!(matches!(
        parser.screen().history_after(HistoryCursor {
            epoch: after.epoch + 1,
            ordinal: 0
        }),
        Err(HistoryError::InvalidCursor)
    ));
    assert!(texts(&parser, after).is_empty());
    parser.process(b"\x1bc");
    assert_eq!(cursor(&parser).epoch, after.epoch + 1);
}

#[test]
fn partial_region_edits_and_disabled_history_never_fabricate_rows() {
    let mut parser = Parser::new(4, 10, 4);
    let first = cursor(&parser);
    parser.process(b"1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[2S\x1b[2T\x1b[L\x1b[M\x1b[2K");
    assert_eq!(cursor(&parser), first);
    assert!(texts(&parser, first).is_empty());
    assert!(matches!(
        parser.screen().history_after(HistoryCursor {
            epoch: 0,
            ordinal: 0
        }),
        Err(HistoryError::InvalidCursor)
    ));
    assert!(matches!(
        parser.screen().history_after(HistoryCursor {
            ordinal: 1,
            ..first
        }),
        Err(HistoryError::InvalidCursor)
    ));
    let mut disabled = Parser::new(2, 10, 0);
    let start = cursor(&disabled);
    disabled.process(b"1\r\n2\r\n3\r\n4");
    assert_eq!(cursor(&disabled), start);
    assert!(texts(&disabled, start).is_empty());
}

#[test]
fn retained_rows_preserve_width_wrap_and_styled_cells_through_resize() {
    let mut parser = Parser::new(2, 10, 4);
    let initial = cursor(&parser);
    parser.process(b"\x1b[31;1mabcdefghijZklmnopqrstU");
    let rows: Vec<_> = parser.screen().history_after(initial).unwrap().collect();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].wrapped);
    assert_eq!(rows[0].width, 10);
    assert_eq!(rows[0].cell(0).unwrap().contents(), "a");
    assert_eq!(rows[0].cell(0).unwrap().fgcolor(), vt100::Color::Idx(1));
    parser.screen_mut().set_size(3, 12);
    let rows: Vec<_> = parser.screen().history_after(initial).unwrap().collect();
    assert_eq!(rows[0].width, 10);
    assert!(rows[0].wrapped);
}

#[test]
fn normal_history_burst_reports_eviction_and_preserves_bounded_suffix() {
    let mut parser = Parser::new(2, 10, 1024);
    let initial = cursor(&parser);
    for _ in 0..1100 {
        parser.process(b"repeat\r\n");
    }
    let end = cursor(&parser);
    assert!(end.ordinal > 1024);
    assert!(matches!(
        parser.screen().history_after(initial),
        Err(HistoryError::Gap)
    ));
    let suffix = HistoryCursor {
        ordinal: end.ordinal - 1024,
        ..end
    };
    let retained = texts(&parser, suffix);
    assert_eq!(retained.len(), 1024);
    assert_eq!(retained[0].0, suffix.ordinal + 1);
    assert_eq!(retained[1023].0, end.ordinal);
}

#[test]
fn history_debug_never_prints_a_row_or_ordinal() {
    let mut parser = Parser::new(2, 10, 4);
    let initial = cursor(&parser);
    parser.process(b"SECRET-ROW\r\nnext\r\nlast");
    let row = parser
        .screen()
        .history_after(initial)
        .unwrap()
        .next()
        .unwrap();
    for output in [format!("{row:?}"), format!("{:?}", cursor(&parser))] {
        assert!(output.contains("<redacted>"));
        assert!(!output.contains("SECRET-ROW"));
        assert!(!output.contains("ordinal"));
    }
}
